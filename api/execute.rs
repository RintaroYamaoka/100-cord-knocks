//! POST /api/execute — 提出コードを Vercel Sandbox で実行するプロキシ。
//!
//! 7 言語すべてを**自前イメージ (VCR: knocks-runtime) の Sandbox** で動かす (ADR 0003)。
//! 以前は Rust だけ play.rust-lang.org、他 6 言語は wandbox.org に出していたが、
//! 2026-09-07 の Wandbox 全体障害で 6 言語が 4 日以上停止したため実行基盤を自前に寄せた。
//!
//! 公式 SDK は JS / Python のみだが、同じ操作は REST API として公開されている。
//! 認証は本番では OIDC (`VERCEL_OIDC_TOKEN`) が自動で入る。
//!
//! 契約・スクリプト生成・出力の詰め替えは `shared::{runner, sandbox}` に集約し、
//! ここは HTTP glue のみ。Vercel 公式 Rust ランタイム (vercel_runtime 2.x) 上で動く。

use std::time::Duration;

use http_body_util::BodyExt;
use hyper::StatusCode;
use shared::language::Language;
use shared::contract::{validate, ExecuteRequest, ExecuteResponse};
use shared::runner::{
    base64_encode, build_script, normalize_runner, parse_runner_output, Nonce, PRODUCTION_IMAGE,
};
use shared::sandbox::{
    classify_sandbox_failure, parse_command_stream, pick_token, CreateSandboxRequest,
    CreateSandboxResponse, ExecCommandRequest, UpstreamFailure, OIDC_HEADER,
};
use vercel_runtime::{run, service_fn, Error, Request, Response};

const SANDBOX_API: &str = "https://api.vercel.com";

/// 作成が一時失敗したときの再試行回数 (VCR のイメージ準備中・上流 5xx)。
const CREATE_RETRIES: usize = 2;

/// 上流 API への HTTP 待ち。コマンド実行は最大 45 秒待つので、それより長く取る。
const HTTP_TIMEOUT: Duration = Duration::from_secs(75);

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(service_fn(handler)).await
}

fn json_response(status: StatusCode, body: String) -> Result<Response<String>, Error> {
    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(body)?)
}

fn json_error(status: StatusCode, message: &str) -> Result<Response<String>, Error> {
    json_response(status, serde_json::json!({ "error": message }).to_string())
}

fn ok(resp: &ExecuteResponse) -> Result<Response<String>, Error> {
    json_response(StatusCode::OK, serde_json::to_string(resp)?)
}

const BUSY: &str = "実行環境が混雑しています。少し待ってから再試行してください";
const UNREACHABLE: &str = "実行環境に接続できませんでした。しばらくして再試行してください";
const TIMED_OUT: &str = "実行がタイムアウトしました。無限ループがないか確認してください";
const UNPARSEABLE: &str = "実行結果を解釈できませんでした";
const UPSTREAM_FAILED: &str = "実行環境がエラーを返しました";
const NO_CREDENTIALS: &str = "実行環境の認証情報がありません (サーバー設定の問題です)";
/// Hobby の無料枠を使い切ったとき。**再試行しても直らない**ので、そう言う。
/// 2026-09-07 の教訓: 原因が利用者のコードでないなら、文言でそれを伝える。
const QUOTA_EXHAUSTED: &str =
    "実行環境の今月の無料枠を使い切りました。枠がリセットされるまで実行できません (コードの問題ではありません)";

/// 上流 (Vercel Sandbox API) 呼び出しの失敗。HTTP の殻に変換する前の形。
enum SandboxError {
    NoCredentials,
    Busy,
    Quota,
    Unreachable,
    TimedOut,
    /// スクリプトは完走したのに出力が読めない (上流の応答形が変わった等)
    Unparseable,
    Failed,
}

impl SandboxError {
    fn into_response(self) -> Result<Response<String>, Error> {
        match self {
            SandboxError::NoCredentials => {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, NO_CREDENTIALS)
            }
            SandboxError::Busy => json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY),
            SandboxError::Quota => json_error(StatusCode::SERVICE_UNAVAILABLE, QUOTA_EXHAUSTED),
            SandboxError::Unreachable => json_error(StatusCode::BAD_GATEWAY, UNREACHABLE),
            SandboxError::TimedOut => json_error(StatusCode::GATEWAY_TIMEOUT, TIMED_OUT),
            SandboxError::Unparseable => json_error(StatusCode::BAD_GATEWAY, UNPARSEABLE),
            SandboxError::Failed => json_error(StatusCode::BAD_GATEWAY, UPSTREAM_FAILED),
        }
    }
}

pub async fn handler(req: Request) -> Result<Response<String>, Error> {
    if req.method() != hyper::Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "POST のみ受け付けます");
    }

    // 本番の認証情報はここにしか無い。body を消費する前に取り出す
    let header_token = req
        .headers()
        .get(OIDC_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "リクエストボディを読めませんでした"),
    };
    let exec_req: ExecuteRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "リクエストボディが不正です"),
    };
    dispatch(exec_req, header_token.as_deref()).await
}

/// 検証して実行する。HTTP の殻を剥がした本体。
///
/// `handler` から切り出してあるのは、`Request` の中身 (`hyper::body::Incoming`) が
/// テストから組み立てられないため。ここを独立させておくと、実際に配信されるコードを
/// そのまま実上流に対して走らせて検証できる (下の `#[ignore]` テストと tools/local_server.rs)。
pub async fn dispatch(
    exec_req: ExecuteRequest,
    header_token: Option<&str>,
) -> Result<Response<String>, Error> {
    if let Err(msg) = validate(&exec_req) {
        return json_error(StatusCode::BAD_REQUEST, &msg);
    }
    match run_in_sandbox(exec_req.language, &exec_req.code, header_token).await {
        Ok(resp) => ok(&resp),
        Err(e) => e.into_response(),
    }
}

fn image() -> String {
    std::env::var("KNOCKS_SANDBOX_IMAGE").unwrap_or_else(|_| PRODUCTION_IMAGE.to_string())
}

/// 上流の認証トークン。
///
/// **本番はリクエストヘッダ `x-vercel-oidc-token` から来る** (Vercel は関数の実行時に
/// 環境変数ではなくヘッダで渡す)。ローカル検証は `vercel env pull` が置く
/// `VERCEL_OIDC_TOKEN`。最後のフォールバックは個人アクセストークンだが、
/// **リポジトリとローカル環境にトークンは置かない** (Vercel 側の環境変数に入れる)。
fn token(header_token: Option<&str>) -> Option<String> {
    let oidc = std::env::var("VERCEL_OIDC_TOKEN").ok();
    let fallback = std::env::var("KNOCKS_VERCEL_TOKEN").ok();
    pick_token(header_token, oidc.as_deref(), fallback.as_deref())
}

/// 推測できない 16 進文字列。セクション区切りの nonce と cmdId に使う。
///
/// nonce が推測できると、提出コードが偽のセクション区切りを印字して
/// 「コンパイルエラーを正解に見せる」経路が開く (パーサ側も後勝ちで守っている)。
fn random_hex() -> String {
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf).is_err() {
        // 乱数が取れない環境では時刻で代替する (起こらないはずだが、落とさない)
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("{nanos:032x}");
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn client() -> Result<reqwest::Client, SandboxError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| SandboxError::Failed)
}

async fn run_in_sandbox(
    language: Language,
    code: &str,
    header_token: Option<&str>,
) -> Result<ExecuteResponse, SandboxError> {
    let token = token(header_token).ok_or(SandboxError::NoCredentials)?;
    let client = client()?;

    let nonce = Nonce::new(&random_hex());
    let script = build_script(language, &base64_encode(code.as_bytes()), &nonce);

    let session = create_session(&client, &token).await?;
    let result = exec_script(&client, &token, &session, &script).await;

    // 停止は best effort。止め忘れてもサンドボックスの timeout で落ちるが、
    // 待つぶんだけ課金対象のメモリ時間が伸びるので必ず投げる。
    stop_session(&client, &token, &session).await;

    let result = result?;
    match parse_runner_output(&nonce, &result.stdout) {
        Ok(parsed) => Ok(normalize_runner(language, &parsed)),
        Err(_) => {
            // 区切りが揃っていない = スクリプトが完走していない。
            // コマンド自体が上限で殺された (SIGKILL) 場合がこれに当たる。
            if result.exit_code != Some(0) {
                Err(SandboxError::TimedOut)
            } else {
                Err(SandboxError::Unparseable)
            }
        }
    }
}

async fn create_session(
    client: &reqwest::Client,
    token: &str,
) -> Result<String, SandboxError> {
    // projectId は送らない: OIDC トークンがプロジェクトに紐づいているため上流が解決する
    // (実測 2026-09-11)。本番の関数環境にプロジェクト ID は渡ってこない。
    let payload = CreateSandboxRequest::for_submission(&image());

    for attempt in 0..=CREATE_RETRIES {
        let resp = match client
            .post(format!("{SANDBOX_API}/v4/sandboxes"))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Err(SandboxError::TimedOut),
            Err(_) => return Err(SandboxError::Unreachable),
        };

        if resp.status().is_success() {
            return match resp.json::<CreateSandboxResponse>().await {
                Ok(parsed) => Ok(parsed.session.id),
                Err(_) => Err(SandboxError::Failed),
            };
        }

        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        match classify_sandbox_failure(status, &body) {
            UpstreamFailure::QuotaExhausted => return Err(SandboxError::Quota),
            UpstreamFailure::Transient if attempt < CREATE_RETRIES => {
                tokio::time::sleep(Duration::from_millis(600 * (attempt as u64 + 1))).await;
                continue;
            }
            UpstreamFailure::Transient => return Err(SandboxError::Busy),
            UpstreamFailure::Other => return Err(SandboxError::Failed),
        }
    }
    Err(SandboxError::Busy)
}

async fn exec_script(
    client: &reqwest::Client,
    token: &str,
    session: &str,
    script: &str,
) -> Result<shared::sandbox::CommandResult, SandboxError> {
    // cmdId はクエリ引数で、呼び出し側が採番する
    let cmd_id = format!("c{}", random_hex());
    let resp = match client
        .post(format!("{SANDBOX_API}/v2/sandboxes/sessions/{session}/cmd?cmdId={cmd_id}"))
        .bearer_auth(token)
        .json(&ExecCommandRequest::shell_script(script))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(SandboxError::TimedOut),
        Err(_) => return Err(SandboxError::Unreachable),
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(match classify_sandbox_failure(status, &body) {
            UpstreamFailure::QuotaExhausted => SandboxError::Quota,
            UpstreamFailure::Transient => SandboxError::Busy,
            UpstreamFailure::Other => SandboxError::Failed,
        });
    }

    let body = resp.text().await.map_err(|_| SandboxError::Failed)?;
    Ok(parse_command_stream(&body))
}

/// 停止。失敗しても利用者の結果には影響させない (サンドボックスは timeout で必ず消える)。
async fn stop_session(client: &reqwest::Client, token: &str, session: &str) {
    let _ = client
        .post(format!("{SANDBOX_API}/v2/sandboxes/sessions/{session}/stop"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await;
}

// ---- 実上流に対する疎通テスト ----
//
// 既定では走らせない (ネットワークと Vercel の認証に依存するため)。
// 実行するときは、個人アカウントの OIDC トークンを取ってから:
//   vercel link --yes --project 100-cord-knocks && vercel env pull
//   set -a; . ./.env.local; set +a
//   cargo test -p rust-100-knocks-api -- --ignored --nocapture --test-threads=1
//
// ここで検証するのは「配信される実物のプロキシコードが、7 言語それぞれで
// 本物のコンパイラ診断と判定を返すか」。見本は shared::fixtures (verifier と共有)。
#[cfg(test)]
mod upstream_tests {
    use super::*;
    use shared::fixtures;
    use shared::contract::{classify, Outcome};
    use shared::problem::compose_submission;

    async fn run(lang: Language, user_code: &str) -> (StatusCode, ExecuteResponse) {
        let code = compose_submission(lang, user_code, fixtures::for_language(lang).hidden_tests);
        let resp = dispatch(ExecuteRequest::judge(lang, &code), None).await.expect("dispatch が失敗");
        let status = resp.status();
        let body = resp.into_body();
        if status != StatusCode::OK {
            return (
                status,
                ExecuteResponse { success: false, stdout: String::new(), stderr: body, compile_failed: false },
            );
        }
        (status, serde_json::from_str(&body).expect("応答が ExecuteResponse として読めない"))
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn every_language_judges_a_correct_answer_as_passed() {
        for lang in Language::ALL {
            let started = std::time::Instant::now();
            let (status, r) = run(lang, fixtures::for_language(lang).answer).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status} — {}", lang.slug(), r.stderr);
            assert_eq!(
                classify(&r),
                Outcome::Passed,
                "{}: stdout={:?} stderr={:?}",
                lang.slug(),
                r.stdout,
                r.stderr
            );
            println!("✓ {:<11} 正解 → Passed ({:.2}s)", lang.slug(), started.elapsed().as_secs_f64());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn every_language_rejects_the_unimplemented_starter() {
        for lang in Language::ALL {
            let (status, r) = run(lang, fixtures::for_language(lang).starter).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status}", lang.slug());
            let o = classify(&r);
            assert_ne!(o, Outcome::Passed, "{}: 未実装が正解になった", lang.slug());
            println!("✓ {:<11} 未実装 → {o:?}", lang.slug());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn every_language_reports_a_real_compiler_diagnostic() {
        // 「実物のエラーログを読ませる」というアプリの目的そのもの。
        // 診断がその言語のコンパイラ固有の形をしていることまで見る。
        for lang in Language::ALL {
            let (status, r) = run(lang, fixtures::for_language(lang).broken).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status}", lang.slug());
            let sig = fixtures::compiler_error_signature(lang);
            assert_eq!(classify(&r), Outcome::CompileError, "{}: stderr={:?}", lang.slug(), r.stderr);
            assert!(r.stderr.contains(sig), "{}: 固有の診断が無い — {:?}", lang.slug(), r.stderr);
            let first = r.stderr.lines().find(|l| l.contains(sig)).unwrap_or("");
            println!("✓ {:<11} 壊れたコード → CompileError: {}", lang.slug(), first.trim());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn csharp_response_carries_no_build_noise() {
        let (_, r) = run(Language::Csharp, fixtures::for_language(Language::Csharp).answer).await;
        for noise in ["MSBuild version", "Restore succeeded", "Determining projects", "Build succeeded", ".csproj]"] {
            assert!(!r.stderr.contains(noise), "ビルドノイズが残っている ({noise}): {:?}", r.stderr);
        }
        println!("✓ C# のビルドノイズは除去されている (stderr={:?})", r.stderr);
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn a_submission_that_exits_before_the_tests_is_not_passed() {
        // 終了コードだけで正解にすると通ってしまう経路 (ADR 0002 の判定順序 5)
        let (_, r) = run(Language::Python, "import sys\ndef add(a, b):\n    return 0\nsys.exit(0)").await;
        assert_eq!(classify(&r), Outcome::NoTestsRun, "stdout={:?}", r.stdout);
        println!("✓ 先に exit(0) する提出 → NoTestsRun");
    }
}
