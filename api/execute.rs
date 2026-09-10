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

// 利用者に見える文言。**実行先 (Vercel Sandbox) を名乗り、利用者のコードの問題か
// どうかを言い切る。** 2026-09-07 の Wandbox 障害では「実行サービスがエラーを返しました」
// としか出ておらず、利用者が自分のコードを疑い続けた。名前の正本は
// `shared::runner::BACKEND_LABEL` (フロントのコンソール表示と同じものを使う)。
const BUSY: &str = "Vercel Sandbox が混雑しています。少し待ってから再試行してください";
const UNREACHABLE: &str =
    "Vercel Sandbox に接続できませんでした。しばらくして再試行してください (コードの問題ではありません)";
/// これだけは**利用者のコード側の問題**なので、実行先の名前を出さない。
const TIMED_OUT: &str = "実行がタイムアウトしました。無限ループがないか確認してください";
const UNPARSEABLE: &str =
    "Vercel Sandbox の応答を解釈できませんでした (コードの問題ではありません)";
const UPSTREAM_FAILED: &str = "Vercel Sandbox がエラーを返しました (コードの問題ではありません)";
const NO_CREDENTIALS: &str =
    "Vercel Sandbox の認証情報がありません (サーバー設定の問題です。コードの問題ではありません)";
/// トークンが無効 / 権限不足。利用者には直せないので、設定の問題だと言い切る。
const BAD_CREDENTIALS: &str =
    "Vercel Sandbox の認証が拒否されました (サーバー設定の問題です。コードの問題ではありません)";
/// Hobby の無料枠を使い切ったとき。**再試行しても直らない**ので、そう言う。
const QUOTA_EXHAUSTED: &str =
    "Vercel Sandbox の今月の無料枠を使い切りました。枠がリセットされるまで実行できません (コードの問題ではありません)";

/// 上流 (Vercel Sandbox API) 呼び出しの失敗。HTTP の殻に変換する前の形。
#[derive(Debug)]
enum SandboxError {
    NoCredentials,
    BadCredentials,
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
            SandboxError::BadCredentials => {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, BAD_CREDENTIALS)
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
    let upstream = match Upstream::new(header_token) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match run_in_sandbox(exec_req.language, &exec_req.code, &upstream).await {
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

/// 上流への HTTP クライアント。**プロセスで 1 個だけ作って使い回す。**
///
/// 毎回 `Client::builder()` から作ると接続プールも作り直しになり、提出のたびに
/// api.vercel.com への TCP + TLS 握手が要る。Fluid Compute は関数インスタンスを
/// 跨いで再利用するので、静的に持てば 2 回目以降の提出は温まった接続に乗る。
fn client() -> Result<reqwest::Client, SandboxError> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().timeout(HTTP_TIMEOUT).build().ok())
        .clone()
        .ok_or(SandboxError::Failed)
}

/// 上流 (Vercel Sandbox API) への接続一式。
///
/// ベース URL を値で持つのは、**偽の上流を立てて挙動を検証できるようにするため**
/// (下の `mod stop_is_not_awaited`)。本番は常に `SANDBOX_API`。
#[derive(Clone)]
struct Upstream {
    base: String,
    client: reqwest::Client,
    token: String,
}

impl Upstream {
    fn new(header_token: Option<&str>) -> Result<Self, SandboxError> {
        Ok(Self {
            base: SANDBOX_API.to_string(),
            client: client()?,
            token: token(header_token).ok_or(SandboxError::NoCredentials)?,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

/// 応答を返した後も走り続けている停止要求の数。
///
/// 停止を待たなくなった (L1) ので、**「投げたが届いていない」区間が存在する**。
/// 本番ではそれで構わない (サンドボックスは `timeout` で必ず消える) が、
/// 検証では「確かに届いた」ことを確かめたいので、数だけ数えておく。
mod pending_stops {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

    pub fn begin() {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    }

    pub fn end() {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn in_flight() -> usize {
        IN_FLIGHT.load(Ordering::SeqCst)
    }

    /// 走り続けている停止要求が捌けるまで待つ。**検証専用。**
    /// 捌けたら true。上限まで待っても残っていたら false (呼び出し側が失敗にできる)。
    #[allow(dead_code)]
    pub async fn drain(limit: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while in_flight() > 0 {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        true
    }
}

async fn run_in_sandbox(
    language: Language,
    code: &str,
    up: &Upstream,
) -> Result<ExecuteResponse, SandboxError> {
    let nonce = Nonce::new(&random_hex());
    let script = build_script(language, &base64_encode(code.as_bytes()), &nonce);

    let session = create_session(up).await?;
    let result = exec_script(up, &session, &script).await;

    // **停止を待たない。** 実測で 1.6〜2.7 秒あり、提出 1 回の体感時間の 3〜4 割を
    // 占めていた。結果の詰め替えに停止は要らないので、投げるだけ投げて先へ進む。
    //
    // 待たなくて安全な理由:
    //   - 停止は元から best effort (失敗しても利用者の結果には影響させない)
    //   - サンドボックスは `SANDBOX_TIMEOUT_MS` で必ず消える。届かなくても残骸は残らない
    //   - `persistent: false` なので停止時のスナップショットも作られない
    //   - 課金は provisioned memory の 1 分最低課金なので、数十秒早く止めても変わらない
    // Fluid Compute は応答後も関数インスタンスを生かすので、実際はほぼ届く。
    spawn_stop(up.clone(), session);

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

async fn create_session(up: &Upstream) -> Result<String, SandboxError> {
    // projectId は送らない: OIDC トークンがプロジェクトに紐づいているため上流が解決する
    // (実測 2026-09-11)。本番の関数環境にプロジェクト ID は渡ってこない。
    let payload = CreateSandboxRequest::for_submission(&image());

    for attempt in 0..=CREATE_RETRIES {
        let resp = match up
            .client
            .post(up.url("/v4/sandboxes"))
            .bearer_auth(&up.token)
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
            UpstreamFailure::Unauthorized => return Err(SandboxError::BadCredentials),
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
    up: &Upstream,
    session: &str,
    script: &str,
) -> Result<shared::sandbox::CommandResult, SandboxError> {
    // cmdId はクエリ引数で、呼び出し側が採番する
    let cmd_id = format!("c{}", random_hex());
    let resp = match up
        .client
        .post(up.url(&format!("/v2/sandboxes/sessions/{session}/cmd?cmdId={cmd_id}")))
        .bearer_auth(&up.token)
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
            UpstreamFailure::Unauthorized => SandboxError::BadCredentials,
            UpstreamFailure::Transient => SandboxError::Busy,
            UpstreamFailure::Other => SandboxError::Failed,
        });
    }

    let body = resp.text().await.map_err(|_| SandboxError::Failed)?;
    Ok(parse_command_stream(&body))
}

/// 停止を投げて**待たずに戻る**。応答を返した後も走り続ける。
///
/// 失敗しても利用者の結果には影響させない (サンドボックスは timeout で必ず消える)。
fn spawn_stop(up: Upstream, session: String) {
    pending_stops::begin();
    tokio::spawn(async move {
        stop_session(&up, &session).await;
        pending_stops::end();
    });
}

/// 停止そのもの。Content-Type が無いと上流は 415 を返す (実測 2026-09-11)。
async fn stop_session(up: &Upstream, session: &str) {
    let _ = up
        .client
        .post(up.url(&format!("/v2/sandboxes/sessions/{session}/stop")))
        .bearer_auth(&up.token)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await;
}

// ---- 利用者に見える文言 ----
//
// 実行先を替えたら文言も替える。ここが曖昧語のままだと、上流障害のときに
// 利用者が自分のコードを疑い続ける (2026-09-07 の Wandbox 障害の実例)。
#[cfg(test)]
mod messages {
    use super::*;
    use shared::runner::BACKEND_LABEL;

    /// 上流側の不調を伝える文言 (= 利用者のコードの問題ではないもの)。
    const UPSTREAM_MESSAGES: [&str; 5] =
        [BUSY, UNREACHABLE, UNPARSEABLE, UPSTREAM_FAILED, QUOTA_EXHAUSTED];

    /// 設定側の問題を伝える文言。
    const CONFIG_MESSAGES: [&str; 2] = [NO_CREDENTIALS, BAD_CREDENTIALS];

    #[test]
    fn upstream_failures_name_the_backend() {
        for m in UPSTREAM_MESSAGES {
            assert!(m.contains(BACKEND_LABEL), "実行先を名乗っていない: {m}");
        }
        for m in CONFIG_MESSAGES {
            assert!(m.contains(BACKEND_LABEL), "実行先を名乗っていない: {m}");
            assert!(m.contains("サーバー設定の問題"), "設定の問題だと言っていない: {m}");
        }
    }

    #[test]
    fn upstream_failures_say_it_is_not_the_users_code() {
        // 「混雑しています・待ってから再試行」は原因がこちら側だと分かるので除く
        for m in [UNREACHABLE, UNPARSEABLE, UPSTREAM_FAILED, QUOTA_EXHAUSTED] {
            assert!(m.contains("コードの問題ではありません"), "原因の所在を言っていない: {m}");
        }
    }

    #[test]
    fn a_user_side_failure_does_not_blame_the_backend() {
        // タイムアウトは利用者のコード側 (無限ループ) の問題。実行先を出すと誤誘導になる
        assert!(!TIMED_OUT.contains(BACKEND_LABEL), "{TIMED_OUT}");
        assert!(TIMED_OUT.contains("無限ループ"));
    }

    #[test]
    fn no_message_mentions_a_retired_backend() {
        for m in UPSTREAM_MESSAGES.iter().chain(CONFIG_MESSAGES.iter()).chain([TIMED_OUT].iter()) {
            for stale in ["Wandbox", "Playground", "実行サービス"] {
                assert!(!m.contains(stale), "古い実行先が残っている ({stale}): {m}");
            }
        }
    }
}

// ---- 停止を待たないことの検証 (L1) ----
//
// 偽の上流を立てて「応答は停止の完了を待たない」ことを実測する。実上流に対しては
// 停止が速いか遅いかを選べないので、ここだけ上流を差し替えられるようにしてある
// (`Upstream::base`)。本番の経路は `run_in_sandbox` そのままを通る。
#[cfg(test)]
mod stop_is_not_awaited {
    use super::*;
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use shared::contract::{classify, Outcome};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// 偽の上流が停止要求を握る時間。実測 (1.6〜2.7 秒) と同じ桁にしてある。
    const STOP_DELAY: Duration = Duration::from_secs(2);

    #[derive(Default)]
    struct Fake {
        stop_finished: AtomicBool,
    }

    fn json_body(body: String) -> hyper::Response<Full<Bytes>> {
        hyper::Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap()
    }

    /// スクリプトから nonce を拾う (実行ごとに変わるので、偽の上流も本物と同じく
    /// 受け取ったスクリプトから読むしかない)。
    fn nonce_in(script: &str) -> String {
        let start = script.find("KNOCKS").expect("スクリプトに区切りが無い");
        script[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect()
    }

    async fn route(
        fake: Arc<Fake>,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
        let path = req.uri().path().to_string();
        let body = req.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();

        if path == "/v4/sandboxes" {
            return Ok(json_body(r#"{"session":{"id":"s1"}}"#.to_string()));
        }
        if path.ends_with("/cmd") {
            // 本物のスクリプトが出すのと同じ形の stdout を組み立てて返す
            let n = nonce_in(&String::from_utf8_lossy(&body));
            let mut out = String::new();
            for (section, value) in
                [("cout", ""), ("cerr", ""), ("pout", "test result: ok\n"), ("perr", ""), ("exit", "0\n")]
            {
                out.push_str(&format!("\n{n}:{section}\n{value}"));
            }
            out.push_str(&format!("\n{n}:end\n"));
            let stream = serde_json::json!({ "stream": "stdout", "data": out }).to_string();
            let done = serde_json::json!({ "command": { "exitCode": 0 } }).to_string();
            return Ok(json_body(format!("{stream}\n{done}\n")));
        }
        if path.ends_with("/stop") {
            tokio::time::sleep(STOP_DELAY).await;
            fake.stop_finished.store(true, Ordering::SeqCst);
            return Ok(json_body("{}".to_string()));
        }
        Ok(json_body("{}".to_string()))
    }

    async fn fake_upstream(fake: Arc<Fake>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind できない");
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let fake = fake.clone();
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(stream),
                            service_fn(move |req| route(fake.clone(), req)),
                        )
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_response_does_not_wait_for_the_sandbox_to_stop() {
        let fake = Arc::new(Fake::default());
        let up = Upstream {
            base: fake_upstream(fake.clone()).await,
            client: client().expect("クライアントが作れない"),
            token: "test-token".to_string(),
        };

        let started = std::time::Instant::now();
        let resp = run_in_sandbox(Language::Python, "def add(a, b):\n    return a + b\n", &up)
            .await
            .expect("実行に失敗");
        let elapsed = started.elapsed();

        // 結果は普段どおり返る
        assert_eq!(classify(&resp), Outcome::Passed, "{resp:?}");
        // 停止 (2 秒) を待っていない
        assert!(elapsed < STOP_DELAY / 2, "停止の完了を待っている: {elapsed:?}");
        assert!(!fake.stop_finished.load(Ordering::SeqCst), "応答時点で停止が完了している");

        // 待たないだけで、投げっぱなしにはしない: 応答の後に確かに届く
        assert!(pending_stops::drain(STOP_DELAY * 3).await, "停止要求が捌けなかった");
        assert!(fake.stop_finished.load(Ordering::SeqCst), "停止が上流に届いていない");
    }

    #[tokio::test]
    async fn a_sandbox_outlives_a_lost_stop_by_at_most_its_own_timeout() {
        // 停止が届かなくても残骸が残らない根拠 = サンドボックス自身の timeout。
        // コマンドの上限より長く、かつ「忘れられても数分で消える」範囲に収まっていること。
        let t = shared::sandbox::SANDBOX_TIMEOUT_MS;
        assert!(t > shared::sandbox::COMMAND_TIMEOUT_MS, "コマンドより先にサンドボックスが死ぬ");
        assert!(t <= 5 * 60 * 1000, "停止が届かないと {t}ms 生き残る (長すぎる)");
    }
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

    /// テストのランタイムが終わると、まだ届いていない停止要求は捨てられる。
    /// 実サンドボックスを置き去りにしない (Hobby は同時 10 が上限) ために、
    /// 各テストの最後で捌けるまで待つ。**本番はここを待たない** (それが L1)。
    async fn stop_all() {
        assert!(pending_stops::drain(Duration::from_secs(15)).await, "停止要求が残った");
    }

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
        stop_all().await;
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
        stop_all().await;
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
        stop_all().await;
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn csharp_response_carries_no_build_noise() {
        let (_, r) = run(Language::Csharp, fixtures::for_language(Language::Csharp).answer).await;
        for noise in ["MSBuild version", "Restore succeeded", "Determining projects", "Build succeeded", ".csproj]"] {
            assert!(!r.stderr.contains(noise), "ビルドノイズが残っている ({noise}): {:?}", r.stderr);
        }
        println!("✓ C# のビルドノイズは除去されている (stderr={:?})", r.stderr);
        stop_all().await;
    }

    #[tokio::test]
    #[ignore = "実上流 (Vercel Sandbox) に接続する"]
    async fn a_submission_that_exits_before_the_tests_is_not_passed() {
        // 終了コードだけで正解にすると通ってしまう経路 (ADR 0002 の判定順序 5)
        let (_, r) = run(Language::Python, "import sys\ndef add(a, b):\n    return 0\nsys.exit(0)").await;
        assert_eq!(classify(&r), Outcome::NoTestsRun, "stdout={:?}", r.stdout);
        println!("✓ 先に exit(0) する提出 → NoTestsRun");
        stop_all().await;
    }
}
