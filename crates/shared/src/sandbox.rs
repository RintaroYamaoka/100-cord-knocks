//! Vercel Sandbox REST API の契約 (純データ部分)。
//!
//! HTTP を投げるのは `api/execute.rs` の責務で、ここには**形と判断**だけを置く。
//! こうしているのは、上流の応答形と失敗の分類をテストで固定できるようにするため
//! (実測した応答を `crates/shared/tests/sandbox.rs` の固定値にしている)。
//!
//! 公式 SDK は JS / Python しか無いが、同じ操作は REST API として公開されている
//! (OpenAPI に掲載)。Rust の関数からは REST を直接叩く。
//!
//!   POST /v4/sandboxes                           作成
//!   POST /v2/sandboxes/sessions/{id}/cmd         コマンド実行 (wait=true で同期)
//!   POST /v2/sandboxes/sessions/{id}/stop        停止

use serde::{Deserialize, Serialize};

use crate::runner::CASE_TIMEOUT_SECS;

/// サンドボックスの寿命 (ミリ秒)。1 提出ぶんしか使わないので短く取る。
/// 実行自体はスクリプト内の `timeout` でも縛っているので、これは保険。
pub const SANDBOX_TIMEOUT_MS: u64 = (CASE_TIMEOUT_SECS + 40) * 1000;

/// コマンド 1 本の上限 (ミリ秒)。スクリプト内の `timeout` より少し長く取る。
pub const COMMAND_TIMEOUT_MS: u64 = (CASE_TIMEOUT_SECS + 25) * 1000;

/// 提出 1 回に割り当てる vCPU 数。1 vCPU = メモリ 2GB。
/// Hobby の枠 (Active CPU 5 時間 / 月) を食い潰さないよう最小にしてある。
pub const VCPUS: u32 = 1;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    pub vcpus: u32,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct NetworkPolicy {
    pub mode: &'static str,
}

/// `POST /v4/sandboxes` の送信形。
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxRequest {
    pub image: String,
    pub resources: Resources,
    /// ミリ秒
    pub timeout: u64,
    /// **必ず false。** 既定 (true) だと停止のたびに自動スナップショットが作られ、
    /// Hobby の生涯 15GB のスナップショット枠を数回で使い切る。
    pub persistent: bool,
    /// 他人のコードを動かすので外向き通信を閉じる。
    /// ひな形プロジェクトを restore 済みでイメージに焼いてあるのはこのため。
    pub network_policy: NetworkPolicy,
}

impl CreateSandboxRequest {
    /// 提出 1 回ぶんのサンドボックス。
    pub fn for_submission(image: &str) -> Self {
        Self {
            image: image.to_string(),
            resources: Resources { vcpus: VCPUS },
            timeout: SANDBOX_TIMEOUT_MS,
            persistent: false,
            network_policy: NetworkPolicy { mode: "deny-all" },
        }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub id: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CreateSandboxResponse {
    pub session: SessionRef,
}

/// `POST /v2/sandboxes/sessions/{id}/cmd` の送信形。
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ExecCommandRequest {
    pub command: String,
    pub args: Vec<String>,
    /// true で ND-JSON を返し、完了まで待つ
    pub wait: bool,
    /// true で stdout / stderr をストリームに混ぜて返す
    pub logs: bool,
    /// ミリ秒
    pub timeout: u64,
}

impl ExecCommandRequest {
    /// スクリプトを `sh -s` の標準入力ではなく引数で渡す。
    /// (REST では標準入力を繋げないため、スクリプト全体を -c の引数にする)
    pub fn shell_script(script: &str) -> Self {
        Self {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            wait: true,
            logs: true,
            timeout: COMMAND_TIMEOUT_MS,
        }
    }
}

/// コマンド実行の結果 (ND-JSON ストリームを畳んだもの)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    /// 最後の command イベントが持つ終了コード。
    /// ストリームが途中で切れた場合は None (= 呼び出し側は失敗として扱う)。
    pub exit_code: Option<i32>,
}

#[derive(Deserialize)]
struct StreamLine {
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    command: Option<StreamCommand>,
}

#[derive(Deserialize)]
struct StreamCommand {
    #[serde(default)]
    #[serde(rename = "exitCode")]
    exit_code: Option<i32>,
}

/// ND-JSON のストリームを畳んで stdout / stderr / 終了コードにする。
///
/// 解釈できない行は捨てる (上流がイベント種別を増やしても落ちないように)。
pub fn parse_command_stream(ndjson: &str) -> CommandResult {
    let mut result = CommandResult::default();
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<StreamLine>(line) else {
            continue;
        };
        if let (Some(data), Some(stream)) = (parsed.data.as_ref(), parsed.stream.as_ref()) {
            match stream.as_str() {
                "stdout" => result.stdout.push_str(data),
                "stderr" => result.stderr.push_str(data),
                _ => {}
            }
        }
        if let Some(cmd) = parsed.command {
            if let Some(code) = cmd.exit_code {
                result.exit_code = Some(code);
            }
        }
    }
    result
}

/// Vercel が関数実行時に OIDC トークンを載せてくるリクエストヘッダ。
///
/// **環境変数ではない。** ビルド時とローカル (`vercel env pull`) は
/// `VERCEL_OIDC_TOKEN` だが、関数の実行時はリクエストヘッダで渡される
/// (https://vercel.com/docs/oidc の "In Vercel Functions")。
/// ここを env だけ見ていたため、2026-09-11 の初回デプロイが全言語
/// 500「認証情報がありません」になった。
pub const OIDC_HEADER: &str = "x-vercel-oidc-token";

/// 使うトークンを決める。優先順は リクエストヘッダ → `VERCEL_OIDC_TOKEN` → フォールバック。
///
/// ヘッダを最優先にするのは、本番ではこれだけが毎回更新される有効なトークンだから。
/// 空白だけの値は「無い」として扱う (無効なトークンで 401 を踏むより、
/// 認証情報が無いと分かる方が切り分けが速い)。
pub fn pick_token(
    header: Option<&str>,
    env_oidc: Option<&str>,
    env_fallback: Option<&str>,
) -> Option<String> {
    [header, env_oidc, env_fallback]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

/// 上流が失敗したときの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamFailure {
    /// 再試行する価値がある (5xx / 429 / イメージ準備中)
    Transient,
    /// Hobby の枠切れ。**再試行しても直らず、次の請求サイクルまで作成が止まる。**
    /// 一般エラーに混ぜると 2026-09-07 の Wandbox 障害と同じ「原因不明の 502」に戻るので分ける。
    QuotaExhausted,
    /// それ以外 (リクエスト不正など)。再試行しない
    Other,
}

/// HTTP の状態と本文から失敗の種別を決める。
///
/// 枠切れの応答本文は実測できていない (枠を使い切るまで出ない) ため、
/// 状態 402 と、本文に現れる `quota` / `limit` / `exceeded` の語で判定している。
/// 外れた場合は `Other` に落ちて一般メッセージになる (黙って再試行はしない)。
pub fn classify_sandbox_failure(status: u16, body: &str) -> UpstreamFailure {
    let lower = body.to_ascii_lowercase();

    // VCR がイメージの最適化を終えるまでは作成できない (ドキュメント記載の再試行対象)
    if lower.contains("image_not_ready") {
        return UpstreamFailure::Transient;
    }
    if status == 402
        || lower.contains("quota")
        || lower.contains("resource_limit")
        || lower.contains("exceeded your")
    {
        return UpstreamFailure::QuotaExhausted;
    }
    if status == 429 || (500..600).contains(&status) {
        return UpstreamFailure::Transient;
    }
    UpstreamFailure::Other
}
