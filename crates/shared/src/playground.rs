//! 実行契約 — フロント / プロキシ / verifier が共有する。
//!
//! フロントは `{language, code}` だけを送り、プロキシが言語ごとの上流
//! (Playground / Wandbox) に翻訳して、結果を `ExecuteResponse` に詰め替える。
//! 判定契約とバックエンド選定の正本は ADR 0002。

use serde::{Deserialize, Serialize};

use crate::language::{Backend, Language};

/// 提出コードの上限。上流へのプロキシ時に DoS 的な巨大ペイロードを弾く。
pub const MAX_CODE_BYTES: usize = 64 * 1024;

/// 全テスト成功の目印。stdout に出る (cargo test の出力と同じ位置)。
pub const TEST_OK_MARKER: &str = "test result: ok";
/// テスト失敗の目印。stdout に出る。
pub const TEST_FAILED_MARKER: &str = "test result: FAILED";
/// 「判定テストがともかく最後まで走った」ことを示す接頭辞。
pub const TEST_RESULT_PREFIX: &str = "test result:";

/// 判定テストが実際に走ったか (成否は問わない)。
///
/// これが真なら**コンパイルは成功している**。診断テキストにエラー行があっても
/// コンパイルエラーとして扱ってはいけない。
///
/// この判定が要る理由: `cargo test` はテストが落ちたときに stderr へ
/// `error: test failed, to rerun pass \`--lib\`` を出す。これは rustc の診断ではなく
/// cargo 自身の要約だが、行頭が `error:` なので診断走査に引っかかる。
/// 見落とすと、**Rust のテスト失敗が全件「コンパイルエラー」と表示される**
/// (利用者がいちばん頻繁に見る画面が壊れる)。
pub fn harness_ran(stdout: &str) -> bool {
    stdout.contains(TEST_RESULT_PREFIX)
}

/// `/api/execute` の受信契約。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExecuteRequest {
    pub language: Language,
    pub code: String,
}

impl ExecuteRequest {
    /// 正誤判定: 提出コード (ユーザーコード + hidden_tests) を実行する。
    pub fn judge(language: Language, code: &str) -> Self {
        Self {
            language,
            code: code.to_string(),
        }
    }
}

/// 上流の実行結果を言語非依存の形に均したもの。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExecuteResponse {
    /// プロセスが終了コード 0 で終わったか。
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// コンパイル/型検査の段階で落ちたか。
    ///
    /// プロキシが**コンパイラ診断のテキストだけ**を見て決める。ここを
    /// stderr 全体から推測すると、プログラムの出力に "error" が含まれるだけで
    /// コンパイルエラーと誤判定してしまう。
    #[serde(default)]
    pub compile_failed: bool,
}

/// プロキシが上流へ転送してよいリクエストか。
pub fn validate(req: &ExecuteRequest) -> Result<(), String> {
    if req.code.trim().is_empty() {
        return Err("コードが空です".to_string());
    }
    if req.code.len() > MAX_CODE_BYTES {
        return Err(format!("コードが大きすぎます (上限 {MAX_CODE_BYTES} bytes)"));
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// コンパイルが通り全テスト成功 (= 正解)
    Passed,
    /// コンパイルは通ったがテストが失敗 (= 未正解)
    TestsFailed,
    /// コンパイル / 型検査 / 構文解析で落ちた
    CompileError,
    /// 実行時のパニック・例外
    RuntimeError,
    /// 終了コード 0 なのにテストの目印が無い (= テストが最後まで走らなかった)
    NoTestsRun,
}

/// 実行結果を結果種別に分類する。
///
/// `Passed` の必要条件に「成功の目印がある」を入れているのが要点。終了コードだけを
/// 見ると、ユーザーコードが判定テストより先に `exit(0)` を呼ぶだけで、テストを
/// 1 件も実行しないまま「正解」になってしまう (判定テストをユーザーコードの後ろに
/// 連結する方式の構造的な穴)。
pub fn classify(resp: &ExecuteResponse) -> Outcome {
    let has_ok = resp.stdout.contains(TEST_OK_MARKER);
    let has_failed = resp.stdout.contains(TEST_FAILED_MARKER) || resp.stderr.contains(TEST_FAILED_MARKER);

    if resp.success && has_ok {
        return Outcome::Passed;
    }
    if resp.compile_failed {
        return Outcome::CompileError;
    }
    if has_failed {
        return Outcome::TestsFailed;
    }
    if !resp.success {
        return Outcome::RuntimeError;
    }
    Outcome::NoTestsRun
}

/// コンパイラ診断のテキストに「エラー水準の行」があるか。
///
/// 引数には**診断テキストだけ**を渡すこと (プログラムの出力を混ぜない)。
pub fn has_compile_error(language: Language, diagnostics: &str) -> bool {
    diagnostics.lines().any(|line| is_error_line(language, line))
}

/// その言語に「実行前の独立したコンパイル段階」があるか。
///
/// Python と JavaScript には無く、構文エラーはインタプリタ起動時に
/// **プログラムの stderr** に出る。したがってこの 2 言語だけは、上流の診断欄が
/// 空でもプログラムの stderr を構文エラーとして走査する必要がある。
/// これをしないと、構文エラーが `RuntimeError` に落ちて
/// 「未実装で落ちた」のか「構文が壊れている」のか利用者に伝わらない (実測で判明)。
pub fn has_separate_compile_phase(language: Language) -> bool {
    !matches!(language, Language::Python | Language::Javascript)
}

/// 1 行がその言語のエラー診断かどうか。
///
/// rustc は行頭が `error`、gcc / javac は `file:line:col: error:`、
/// Roslyn / tsc は `file(line,col): error CSnnnn` と形が違う。行頭だけを見ると
/// rustc 以外の診断が全行「ただの文字列」になり、コンソールで色が付かない。
fn is_error_line(language: Language, line: &str) -> bool {
    let t = line.trim_start();
    match language {
        Language::Rust => t.starts_with("error[") || t.starts_with("error:"),
        Language::Cpp | Language::Java => t.starts_with("error:") || line.contains(": error:"),
        Language::Csharp => line.contains("): error ") || t.starts_with("error "),
        Language::Typescript => line.contains("): error "),
        // Python / JavaScript にコンパイル段階は無いが、構文エラーは実行前に出る
        Language::Python => {
            t.starts_with("SyntaxError") || t.starts_with("IndentationError") || t.starts_with("TabError")
        }
        Language::Javascript => t.starts_with("SyntaxError"),
    }
}

/// C# のビルド出力から定型ノイズを落とす。
///
/// `dotnetcore` は成功時も `dotnet new` と MSBuild の進行状況を吐くので、そのまま
/// 見せると「実物の診断を読む」という目的が埋もれる。除去はホワイトリスト方向
/// (`error` / `warning` を含む行は必ず残す) にしてあり、正規表現に合わない本物の
/// 失敗 (`error MSBnnnn` など) を巻き込まない。
pub fn strip_csharp_build_noise(s: &str) -> String {
    const NOISE_PREFIXES: [&str; 11] = [
        "The template ",
        "Processing post-creation actions",
        "Running 'dotnet restore'",
        "Determining projects to restore",
        "Restored ",
        "Restore succeeded",
        "MSBuild version",
        "All projects are up-to-date",
        "Build succeeded",
        "Time Elapsed",
        "You may only use the Microsoft .NET",
    ];

    s.lines()
        .filter(|line| {
            let t = line.trim();
            if t.is_empty() {
                return false;
            }
            // 診断はどんな形でも残す
            if t.contains("error") || t.contains("warning") || t.contains("Error") || t.contains("Warning") {
                // "0 Warning(s)" / "0 Error(s)" はビルド成功の定型なので落とす
                return !(t.ends_with("Warning(s)") || t.ends_with("Error(s)"));
            }
            if NOISE_PREFIXES.iter().any(|p| t.starts_with(p)) {
                return false;
            }
            // "  prog -> /home/wandbox/prog/bin/Debug/net6.0/prog.dll" のような成果物パス行
            if t.contains(" -> ") && t.ends_with(".dll") {
                return false;
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- Wandbox ----

/// wandbox.org/api/compile.json への送信形。
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct WandboxRequest {
    pub compiler: String,
    pub code: String,
    /// コンパイラごとに定義された**選択肢の ID** (例: gcc の "warning,c++17")。生フラグではない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    /// 生のコンパイラフラグ (改行区切り)。選択肢が用意されていないコンパイラは
    /// こちらでしかフラグを渡せない (typescript がそれ)。
    #[serde(rename = "compiler-option-raw", skip_serializing_if = "Option::is_none")]
    pub compiler_option_raw: Option<String>,
    pub save: bool,
}

/// Wandbox の応答形 (実測したフィールドのみ)。
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
pub struct WandboxResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub signal: String,
    #[serde(default)]
    pub compiler_output: String,
    #[serde(default)]
    pub compiler_error: String,
    #[serde(default)]
    pub program_output: String,
    #[serde(default)]
    pub program_error: String,
}

/// Wandbox が HTTP レベルで失敗したとき、それが「上流の一時障害」かどうか。
///
/// 2026-09-07 の障害では、Wandbox が全コンパイラで HTTP 500 と
/// `Error: Failed to get uid: status=exit status: 125, base_dir=/tmp/wandbox/...`
/// (サーバー側のコンテナランタイム起動失敗) を返し続けた。これは利用者のコードとも
/// こちらのリクエストとも無関係なので、5xx は再試行したうえで「停止中」として返す。
/// 4xx (リクエスト不正・403 など) は再試行しても無駄なので一時障害とはみなさない。
pub fn wandbox_http_failure_is_transient(status: u16, body: &str) -> bool {
    (500..600).contains(&status) || body.contains("Failed to get uid")
}

impl WandboxResponse {
    /// 上流が一時的に落ちている (過負荷) ときの応答か。
    /// これをコンパイルエラーとして見せると、正しいコードが赤く出て学習者が混乱する。
    pub fn is_upstream_transient_error(&self) -> bool {
        self.compiler_error.contains("OCI runtime error")
            || self.compiler_error.contains("Resource temporarily unavailable")
    }

    /// シグナルで殺された実行か (SIGKILL によるメモリ超過、SIGSEGV など)。
    /// 上流は status を空文字にすることがあるので、シグナルの有無も見ないと
    /// 「異常終了なのに Passed」の経路が残る。
    pub fn killed_by_signal(&self) -> bool {
        !self.signal.trim().is_empty()
    }
}

/// `Language` に対応する Wandbox リクエストを組む。Rust (Playground) には None。
pub fn wandbox_request(language: Language, code: &str) -> Option<WandboxRequest> {
    match language.backend() {
        Backend::Playground => None,
        Backend::Wandbox { compiler, options } => Some(WandboxRequest {
            compiler: compiler.to_string(),
            code: code.to_string(),
            options: options.map(String::from),
            // typescript には選択肢が無いので生フラグで target を指定する。
            // ここを渡し忘れると ES2019+ の API を使う正解が TS2550 で落ちる
            compiler_option_raw: match language {
                Language::Typescript => Some(crate::language::tsc_flags_wandbox_raw()),
                _ => None,
            },
            save: false,
        }),
    }
}

/// Wandbox の応答を `ExecuteResponse` に詰め替える。
///
/// 診断 (compiler_*) とプログラム出力 (program_*) を分けて扱い、`compile_failed` は
/// **診断テキストだけ**から決める。両者を混ぜてから判定すると、プログラムが
/// "error" を印字しただけでコンパイルエラーになる。
pub fn normalize_wandbox(language: Language, raw: &WandboxResponse) -> ExecuteResponse {
    // TypeScript の型エラーは compiler_output に、gcc/javac は compiler_error に来る
    let mut diagnostics = String::new();
    for part in [&raw.compiler_error, &raw.compiler_output] {
        if !part.trim().is_empty() {
            if !diagnostics.is_empty() {
                diagnostics.push('\n');
            }
            diagnostics.push_str(part.trim_end());
        }
    }
    if language == Language::Csharp {
        diagnostics = strip_csharp_build_noise(&diagnostics);
    }

    // Python / JavaScript は構文エラーがプログラムの stderr に出るので、そこも走査する。
    // コンパイル段階を持つ言語では走査しない (プログラムが "error:" を印字しただけで
    // コンパイルエラーと誤判定してしまうため)。
    // 判定テストが走ったなら、その時点でコンパイルは成功している。
    // 診断にエラー行が残っていても (警告扱いの行やランナーの要約) コンパイルエラーにしない。
    let compile_failed = !harness_ran(&raw.program_output)
        && (has_compile_error(language, &diagnostics)
            || (!has_separate_compile_phase(language)
                && has_compile_error(language, &raw.program_error)));

    let mut stderr = diagnostics;
    if !raw.program_error.trim().is_empty() {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(raw.program_error.trim_end());
    }

    ExecuteResponse {
        // シグナルで殺された実行は成功にしない
        success: raw.status == "0" && !raw.killed_by_signal(),
        stdout: raw.program_output.clone(),
        stderr,
        compile_failed,
    }
}

// ---- Playground (Rust) ----

/// play.rust-lang.org/execute への送信形。
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct PlaygroundRequest {
    pub channel: String,
    pub mode: String,
    pub edition: String,
    #[serde(rename = "crateType")]
    pub crate_type: String,
    pub tests: bool,
    pub code: String,
    pub backtrace: bool,
}

impl PlaygroundRequest {
    /// 正誤判定: ユーザーコード + `#[test]` 群を tests モードで実行する。
    pub fn judge(code: &str) -> Self {
        Self {
            channel: "stable".into(),
            mode: "debug".into(),
            edition: "2024".into(),
            crate_type: "lib".into(),
            tests: true,
            code: code.to_string(),
            backtrace: false,
        }
    }
}

/// Playground の応答形。
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
pub struct PlaygroundResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

/// Playground の応答を `ExecuteResponse` に詰め替える。
pub fn normalize_playground(raw: &PlaygroundResponse) -> ExecuteResponse {
    ExecuteResponse {
        success: raw.success,
        stdout: raw.stdout.clone(),
        stderr: raw.stderr.clone(),
        // cargo test はテスト失敗時に stderr へ `error: test failed` を出す。
        // 判定テストが走った証拠があるならコンパイルは通っている
        compile_failed: !harness_ran(&raw.stdout)
            && has_compile_error(Language::Rust, &raw.stderr),
    }
}

// ---- コンソール表示 ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Error,
    Warning,
    Note,
    Plain,
}

/// コンソール表示用: 出力の 1 行を色分け種別に分類する。
///
/// 言語を跨いで使うので、行頭形式 (rustc) と `file:line: error:` 形式
/// (gcc / javac) と `file(line,col): error CSnnnn` 形式 (Roslyn / tsc) の
/// 3 つをまとめて拾う。ここを rustc 専用のままにすると、他言語の診断が
/// 全行 Plain になって色が付かない。
pub fn classify_line(line: &str) -> LineKind {
    let t = line.trim_start();

    // gcc / javac の `  2 | int main(){...}` のようなソース引用行を先に除く。
    // 引用されたコードに "error" が含まれていても診断行ではない。
    if t.starts_with('|') || t.split_once('|').is_some_and(|(head, _)| {
        !head.is_empty() && head.chars().all(|c| c.is_ascii_digit() || c.is_whitespace())
    }) {
        return LineKind::Plain;
    }

    if t.starts_with("error") || line.contains(": error:") || line.contains("): error ") {
        return LineKind::Error;
    }
    // 実行時例外の 1 行目 (Java / Python / Node)
    if t.starts_with("Exception in thread")
        || t.ends_with("Error")
        || t.starts_with("SyntaxError")
        || t.starts_with("Traceback (most recent call last)")
        || t.starts_with("thread '") && line.contains("panicked")
    {
        return LineKind::Error;
    }
    if t.starts_with("warning") || line.contains(": warning:") || line.contains("): warning ") {
        return LineKind::Warning;
    }
    if t.starts_with("note:") || t.starts_with("help:") || t.starts_with("-->") || t.starts_with("at ") {
        return LineKind::Note;
    }
    LineKind::Plain
}
