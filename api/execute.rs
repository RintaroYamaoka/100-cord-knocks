//! POST /api/execute — 言語ごとの実行サービスへのバリデーション付きプロキシ。
//!
//! Rust は play.rust-lang.org、他 6 言語は wandbox.org に振り分ける (ADR 0002)。
//! 契約・検証・応答の詰め替えは shared::playground に集約し、ここは HTTP glue のみ。
//! Vercel 公式 Rust ランタイム (vercel_runtime 2.x) 上で動く。

use std::time::Duration;

use http_body_util::BodyExt;
use hyper::StatusCode;
use shared::language::{Backend, Language};
use shared::playground::{
    normalize_playground, normalize_wandbox, validate, wandbox_http_failure_is_transient,
    wandbox_request, ExecuteRequest, ExecuteResponse, PlaygroundRequest, PlaygroundResponse,
    WandboxResponse,
};
use vercel_runtime::{run, service_fn, Error, Request, Response};

const PLAYGROUND_URL: &str = "https://play.rust-lang.org/execute";
const WANDBOX_URL: &str = "https://wandbox.org/api/compile.json";
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(40);

/// Wandbox は既定の User-Agent を 403 で弾く。明示的に名乗る必要がある。
const USER_AGENT: &str = "rust-100-knocks (+https://github.com/RintaroYamaoka/rust-100-knocks)";

/// Wandbox の一時エラーに対する再試行回数。
const WANDBOX_RETRIES: usize = 2;

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

fn client() -> Result<reqwest::Client, Error> {
    Ok(reqwest::Client::builder()
        .timeout(UPSTREAM_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?)
}

const BUSY: &str = "実行サービスが混雑しています。少し待ってから再試行してください";
const UNREACHABLE: &str = "実行サービスに接続できませんでした。しばらくして再試行してください";
const TIMED_OUT: &str = "実行がタイムアウトしました。無限ループがないか確認してください";
const UNPARSEABLE: &str = "実行サービスの応答を解釈できませんでした";
const UPSTREAM_FAILED: &str = "実行サービスがエラーを返しました";
/// Wandbox 自体が落ちているとき。Rust は別の実行先 (Playground) なので影響しないことを伝える。
const WANDBOX_DOWN: &str =
    "実行サービス (Wandbox) が一時的に停止しています。復旧までお待ちください (Rust は影響ありません)";

pub async fn handler(req: Request) -> Result<Response<String>, Error> {
    if req.method() != hyper::Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "POST のみ受け付けます");
    }

    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "リクエストボディを読めませんでした"),
    };
    let exec_req: ExecuteRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "リクエストボディが不正です"),
    };
    dispatch(exec_req).await
}

/// 検証して上流に振り分ける。HTTP の殻を剥がした本体。
///
/// `handler` から切り出してあるのは、`Request` の中身 (`hyper::body::Incoming`) が
/// テストから組み立てられないため。ここを独立させておくと、実際に配信されるコードを
/// そのまま実上流に対して走らせて検証できる (下の `#[ignore]` テスト)。
pub async fn dispatch(exec_req: ExecuteRequest) -> Result<Response<String>, Error> {
    if let Err(msg) = validate(&exec_req) {
        return json_error(StatusCode::BAD_REQUEST, &msg);
    }

    match exec_req.language.backend() {
        Backend::Playground => run_playground(&exec_req.code).await,
        Backend::Wandbox { .. } => run_wandbox(exec_req.language, &exec_req.code).await,
    }
}

async fn run_playground(code: &str) -> Result<Response<String>, Error> {
    let client = client()?;
    let upstream = match client
        .post(PLAYGROUND_URL)
        .json(&PlaygroundRequest::judge(code))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return json_error(StatusCode::GATEWAY_TIMEOUT, TIMED_OUT),
        Err(_) => return json_error(StatusCode::BAD_GATEWAY, UNREACHABLE),
    };
    if upstream.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return json_error(StatusCode::TOO_MANY_REQUESTS, BUSY);
    }
    if !upstream.status().is_success() {
        return json_error(StatusCode::BAD_GATEWAY, UPSTREAM_FAILED);
    }
    match upstream.json::<PlaygroundResponse>().await {
        Ok(raw) => ok(&normalize_playground(&raw)),
        Err(_) => json_error(StatusCode::BAD_GATEWAY, UNPARSEABLE),
    }
}

async fn run_wandbox(language: Language, code: &str) -> Result<Response<String>, Error> {
    let Some(payload) = wandbox_request(language, code) else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "この言語の実行先が設定されていません");
    };
    let client = client()?;

    // Wandbox は過負荷時に OCI ランタイムの一時エラーを返す。
    // これをコンパイルエラーとして見せると、正しいコードが赤く出て学習者が混乱するので、
    // 数回だけ再試行してから上流一時障害として返す。
    for attempt in 0..=WANDBOX_RETRIES {
        let upstream = match client.post(WANDBOX_URL).json(&payload).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return json_error(StatusCode::GATEWAY_TIMEOUT, TIMED_OUT),
            Err(_) => return json_error(StatusCode::BAD_GATEWAY, UNREACHABLE),
        };
        if upstream.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return json_error(StatusCode::TOO_MANY_REQUESTS, BUSY);
        }
        if !upstream.status().is_success() {
            // 2026-09-07: Wandbox 全体が 500 ("Failed to get uid") を返し続ける障害があった。
            // 利用者のコードとは無関係なので、再試行のうえ「停止中」として返す。
            let status = upstream.status().as_u16();
            let body = upstream.text().await.unwrap_or_default();
            if wandbox_http_failure_is_transient(status, &body) {
                if attempt < WANDBOX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(700 * (attempt as u64 + 1))).await;
                    continue;
                }
                return json_error(StatusCode::SERVICE_UNAVAILABLE, WANDBOX_DOWN);
            }
            return json_error(StatusCode::BAD_GATEWAY, UPSTREAM_FAILED);
        }
        let raw: WandboxResponse = match upstream.json().await {
            Ok(r) => r,
            Err(_) => return json_error(StatusCode::BAD_GATEWAY, UNPARSEABLE),
        };
        if raw.is_upstream_transient_error() {
            if attempt < WANDBOX_RETRIES {
                tokio::time::sleep(Duration::from_millis(700 * (attempt as u64 + 1))).await;
                continue;
            }
            return json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY);
        }
        return ok(&normalize_wandbox(language, &raw));
    }
    json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY)
}

// ---- 実上流に対する疎通テスト ----
//
// 既定では走らせない (ネットワークと外部サービスに依存するため)。
// 実行するときは:
//   cargo test -p rust-100-knocks-api -- --ignored --nocapture --test-threads=1
//
// ここで検証するのは「配信される実物のプロキシコードが、7 言語それぞれで
// 本物のコンパイラ診断を返すか」(WO-0001 の D2)。判定の分類そのものは
// shared 側の単体テストが実測値で固定している。
#[cfg(test)]
mod upstream_tests {
    use super::*;
    use shared::playground::{classify, Outcome};
    use shared::problem::compose_submission;

    /// 各言語の「模範解答」「未実装」「わざと壊した」コードと判定テスト。
    /// docs/problem-authoring.md のテンプレートに従う。
    fn fixtures(lang: Language) -> (&'static str, &'static str, &'static str, &'static str) {
        match lang {
            Language::Rust => (
                "pub fn add(a: i32, b: i32) -> i32 { a + b }",
                "pub fn add(a: i32, b: i32) -> i32 { todo!() }",
                "pub fn add(a: i32, b: i32) -> i32 { let x: i32 = \"nope\"; x }",
                "#[test]\nfn pos() { assert_eq!(add(1, 2), 3); }\n#[test]\nfn zero() { assert_eq!(add(0, 0), 0); }",
            ),
            Language::Cpp => (
                "int add(int a, int b) { return a + b; }",
                "int add(int a, int b) { return 0; }",
                "int add(int a, int b) { int x = \"nope\"; return x; }",
                "#include <iostream>\nstatic int f = 0;\nstatic void chk(bool c, const char* n) { if (!c) { std::cerr << \"FAILED: \" << n << \"\\n\"; f++; } }\nint main() { chk(add(1,2)==3, \"pos\"); chk(add(0,0)==0, \"zero\"); if (f) { std::cout << \"test result: FAILED\\n\"; return 1; } std::cout << \"test result: ok\\n\"; return 0; }",
            ),
            Language::Csharp => (
                "using System;\nclass Solution { public static int Add(int a, int b) { return a + b; } }",
                "using System;\nclass Solution { public static int Add(int a, int b) { return 0; } }",
                "using System;\nclass Solution { public static int Add(int a, int b) { string s = 1; return 0; } }",
                "class KnockTests {\n  static int f = 0;\n  static void Chk(bool c, string n) { if (!c) { System.Console.Error.WriteLine(\"FAILED: \" + n); f++; } }\n  static int Main() { Chk(Solution.Add(1,2)==3, \"pos\"); Chk(Solution.Add(0,0)==0, \"zero\"); if (f > 0) { System.Console.WriteLine(\"test result: FAILED\"); return 1; } System.Console.WriteLine(\"test result: ok\"); return 0; }\n}",
            ),
            Language::Java => (
                "class Solution { static int add(int a, int b) { return a + b; } }",
                "class Solution { static int add(int a, int b) { return 0; } }",
                "class Solution { static int add(int a, int b) { String s = 1; return 0; } }",
                "class Main {\n  static int f = 0;\n  static void chk(boolean c, String n) { if (!c) { System.err.println(\"FAILED: \" + n); f++; } }\n  public static void main(String[] a) { chk(Solution.add(1,2)==3, \"pos\"); chk(Solution.add(0,0)==0, \"zero\"); if (f > 0) { System.out.println(\"test result: FAILED\"); System.exit(1); } System.out.println(\"test result: ok\"); }\n}",
            ),
            Language::Python => (
                "def add(a, b):\n    return a + b",
                "def add(a, b):\n    raise NotImplementedError",
                "def add(a, b)\n    return a + b",
                "import sys\n_f = 0\ndef _chk(c, n):\n    global _f\n    if not c:\n        print(\"FAILED: \" + n, file=sys.stderr); _f += 1\n_chk(add(1, 2) == 3, \"pos\")\n_chk(add(0, 0) == 0, \"zero\")\nif _f > 0:\n    print(\"test result: FAILED\"); sys.exit(1)\nprint(\"test result: ok\")",
            ),
            Language::Typescript => (
                "function add(a: number, b: number): number { return a + b; }",
                "function add(a: number, b: number): number { throw new Error(\"TODO\"); }",
                "function add(a: number, b: number): number { const s: string = a + b; return s; }",
                "declare const process: { exit(code: number): never };\nlet __f = 0;\nfunction __chk(c: boolean, n: string): void { if (!c) { console.error(\"FAILED: \" + n); __f++; } }\n__chk(add(1, 2) === 3, \"pos\");\n__chk(add(0, 0) === 0, \"zero\");\nif (__f > 0) { console.log(\"test result: FAILED\"); process.exit(1); }\nconsole.log(\"test result: ok\");",
            ),
            Language::Javascript => (
                "function add(a, b) { return a + b; }",
                "function add(a, b) { }",
                "function add(a, b) { return a + ; }",
                "let __f = 0;\nfunction __chk(c, n) { if (!c) { console.error(\"FAILED: \" + n); __f++; } }\n__chk(add(1, 2) === 3, \"pos\");\n__chk(add(0, 0) === 0, \"zero\");\nif (__f > 0) { console.log(\"test result: FAILED\"); process.exit(1); }\nconsole.log(\"test result: ok\");",
            ),
        }
    }

    async fn run(lang: Language, user_code: &str, tests: &str) -> (StatusCode, ExecuteResponse) {
        let code = compose_submission(lang, user_code, tests);
        let resp = dispatch(ExecuteRequest::judge(lang, &code)).await.expect("dispatch が失敗");
        let status = resp.status();
        let body = resp.into_body();
        if status != StatusCode::OK {
            return (status, ExecuteResponse { success: false, stdout: String::new(), stderr: body, compile_failed: false });
        }
        (status, serde_json::from_str(&body).expect("応答が ExecuteResponse として読めない"))
    }

    #[tokio::test]
    #[ignore = "実上流 (Playground / Wandbox) に接続する"]
    async fn every_language_judges_a_correct_answer_as_passed() {
        for lang in Language::ALL {
            let (answer, _, _, tests) = fixtures(lang);
            let (status, r) = run(lang, answer, tests).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status} — {}", lang.slug(), r.stderr);
            assert_eq!(classify(&r), Outcome::Passed, "{}: stdout={:?} stderr={:?}", lang.slug(), r.stdout, r.stderr);
            println!("✓ {:<11} 正解 → Passed", lang.slug());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Playground / Wandbox) に接続する"]
    async fn every_language_rejects_the_unimplemented_starter() {
        for lang in Language::ALL {
            let (_, starter, _, tests) = fixtures(lang);
            let (status, r) = run(lang, starter, tests).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status}", lang.slug());
            let o = classify(&r);
            assert_ne!(o, Outcome::Passed, "{}: 未実装が正解になった", lang.slug());
            println!("✓ {:<11} 未実装 → {o:?}", lang.slug());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Playground / Wandbox) に接続する"]
    async fn every_language_reports_a_real_compiler_diagnostic() {
        // 「実物のエラーログを読ませる」というアプリの目的そのもの。
        // 診断がその言語のコンパイラ固有の形をしていることまで見る。
        let signature = |lang: Language| -> &'static str {
            match lang {
                Language::Rust => "error[E",
                Language::Cpp => "error:",
                Language::Csharp => "error CS",
                Language::Java => "error:",
                Language::Python => "SyntaxError",
                Language::Typescript => "error TS",
                Language::Javascript => "SyntaxError",
            }
        };
        for lang in Language::ALL {
            let (_, _, broken, tests) = fixtures(lang);
            let (status, r) = run(lang, broken, tests).await;
            assert_eq!(status, StatusCode::OK, "{}: HTTP {status}", lang.slug());
            assert_eq!(classify(&r), Outcome::CompileError, "{}: stderr={:?}", lang.slug(), r.stderr);
            assert!(r.stderr.contains(signature(lang)), "{}: 固有の診断が無い — {:?}", lang.slug(), r.stderr);
            let first = r.stderr.lines().find(|l| l.contains(signature(lang))).unwrap_or("");
            println!("✓ {:<11} 壊れたコード → CompileError: {}", lang.slug(), first.trim());
        }
    }

    #[tokio::test]
    #[ignore = "実上流 (Wandbox) に接続する"]
    async fn csharp_response_carries_no_msbuild_noise() {
        let (answer, _, _, tests) = fixtures(Language::Csharp);
        let (_, r) = run(Language::Csharp, answer, tests).await;
        for noise in ["MSBuild version", "Restore succeeded", "Determining projects", "Build succeeded"] {
            assert!(!r.stderr.contains(noise), "ビルドノイズが残っている ({noise}): {:?}", r.stderr);
        }
        println!("✓ C# のビルドノイズは除去されている (stderr={:?})", r.stderr);
    }
}
