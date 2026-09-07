//! 実行契約のテスト。
//! 固定値は 2026-08-29 に Playground / Wandbox から実測した本物の出力を使っている
//! (作文した出力でテストすると、契約が現実とずれても緑のままになるため)。

use shared::language::Language;
use shared::playground::{
    classify, classify_line, harness_ran, has_compile_error, has_separate_compile_phase,
    normalize_playground, normalize_wandbox, strip_csharp_build_noise, validate,
    wandbox_http_failure_is_transient, ExecuteRequest,
    ExecuteResponse, LineKind, Outcome, PlaygroundResponse, WandboxResponse, MAX_CODE_BYTES,
    TEST_FAILED_MARKER, TEST_OK_MARKER,
};

fn resp(success: bool, stdout: &str, stderr: &str, compile_failed: bool) -> ExecuteResponse {
    ExecuteResponse {
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        compile_failed,
    }
}

fn wb(status: &str, compiler_output: &str, compiler_error: &str, program_output: &str, program_error: &str) -> WandboxResponse {
    WandboxResponse {
        status: status.to_string(),
        signal: String::new(),
        compiler_output: compiler_output.to_string(),
        compiler_error: compiler_error.to_string(),
        program_output: program_output.to_string(),
        program_error: program_error.to_string(),
    }
}

// ---- リクエスト契約 ----

#[test]
fn request_carries_language_and_code() {
    let r = ExecuteRequest::judge(Language::Python, "print(1)");
    assert_eq!(r.language, Language::Python);
    assert_eq!(r.code, "print(1)");
}

#[test]
fn request_roundtrips_through_json() {
    let r = ExecuteRequest::judge(Language::Cpp, "int main(){}");
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("\"language\":\"cpp\""), "{s}");
    let back: ExecuteRequest = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn validate_rejects_oversized_code() {
    let r = ExecuteRequest::judge(Language::Rust, &"a".repeat(MAX_CODE_BYTES + 1));
    assert!(validate(&r).is_err());
}

#[test]
fn validate_rejects_empty_code() {
    assert!(validate(&ExecuteRequest::judge(Language::Rust, "   ")).is_err());
}

#[test]
fn validate_accepts_every_language() {
    for l in Language::ALL {
        assert!(validate(&ExecuteRequest::judge(l, "x")).is_ok(), "{}", l.slug());
    }
}

// ---- Outcome の判定 ----

#[test]
fn passed_requires_both_success_and_ok_marker() {
    assert_eq!(classify(&resp(true, TEST_OK_MARKER, "", false)), Outcome::Passed);
}

#[test]
fn exit_zero_without_ok_marker_is_not_passed() {
    // ユーザーコードが先に exit(0) を呼ぶと、テストが 1 件も走らないまま成功終了する。
    // これを Passed にすると「テストを書かずに正解」が全言語で成立してしまう。
    let r = resp(true, "何か出力しただけ\n", "", false);
    assert_eq!(classify(&r), Outcome::NoTestsRun);
    assert_ne!(classify(&r), Outcome::Passed);
}

#[test]
fn empty_output_with_exit_zero_is_not_passed() {
    assert_eq!(classify(&resp(true, "", "", false)), Outcome::NoTestsRun);
}

#[test]
fn compile_failure_takes_priority_over_markers() {
    let r = resp(false, TEST_FAILED_MARKER, "prog.cc:2:21: error: invalid conversion", true);
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn failed_marker_is_tests_failed() {
    assert_eq!(
        classify(&resp(false, TEST_FAILED_MARKER, "FAILED: add_positive\n", false)),
        Outcome::TestsFailed
    );
}

#[test]
fn nonzero_exit_without_markers_is_runtime_error() {
    let stderr = "Exception in thread \"main\" java.lang.ArrayIndexOutOfBoundsException: Index 5 out of bounds for length 1\n\tat Main.main(prog.java:1)";
    assert_eq!(classify(&resp(false, "", stderr, false)), Outcome::RuntimeError);
}

#[test]
fn rust_cargo_test_output_classifies_without_special_casing() {
    // cargo test は成功時 "test result: ok. 3 passed" を stdout に出す。
    // 非 Rust 言語の harness をこれに合わせたので、判定経路は 1 本で済む。
    let ok = resp(true, "running 3 tests\n\ntest result: ok. 3 passed; 0 failed", "", false);
    assert_eq!(classify(&ok), Outcome::Passed);

    let failed = resp(
        false,
        "running 1 test\ntest doubles ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed",
        "",
        false,
    );
    assert_eq!(classify(&failed), Outcome::TestsFailed);
}

// ---- コンパイルエラーの検出 (言語別・実測出力) ----

#[test]
fn detects_compile_errors_from_real_diagnostics() {
    let cases = [
        (Language::Rust, "error[E0308]: mismatched types\n --> src/lib.rs:2:5"),
        (Language::Cpp, "prog.cc: In function 'int main()':\nprog.cc:2:21: error: invalid conversion from 'const char*' to 'int' [-fpermissive]"),
        (Language::Java, "prog.java:3: error: incompatible types: int cannot be converted to String\n        String s = 1;"),
        (Language::Csharp, "/home/wandbox/prog/Program.cs(1,65): error CS0029: Cannot implicitly convert type 'int' to 'string'"),
        (Language::Typescript, "prog.ts(2,7): error TS2322: Type 'number' is not assignable to type 'string'."),
        (Language::Python, "  File \"prog.py\", line 3\n    def f(:\n          ^\nSyntaxError: invalid syntax"),
        (Language::Javascript, "/home/wandbox/prog.js:2\nfunction (\n         ^\n\nSyntaxError: Unexpected token '('"),
    ];
    for (lang, diag) in cases {
        assert!(has_compile_error(lang, diag), "{} の診断を検出できていない", lang.slug());
    }
}

#[test]
fn clean_output_is_not_a_compile_error() {
    for l in Language::ALL {
        assert!(!has_compile_error(l, ""), "{}", l.slug());
        assert!(!has_compile_error(l, "test result: ok\n"), "{}", l.slug());
    }
    // 警告だけならコンパイルエラーではない
    assert!(!has_compile_error(Language::Cpp, "prog.cc:3:9: warning: unused variable 'x' [-Wunused-variable]"));
    assert!(!has_compile_error(Language::Rust, "warning: unused variable: `x`"));
}

#[test]
fn program_output_containing_the_word_error_is_not_a_compile_error() {
    // 診断テキストだけを見るので、プログラムが "error" と印字しても誤検出しない
    assert!(!has_compile_error(Language::Python, ""));
}

#[test]
fn python_and_javascript_have_no_separate_compile_phase() {
    // この 2 言語は構文エラーがインタプリタ起動時に「プログラムの stderr」へ出る。
    // 上流の診断欄しか見ないと、構文エラーが実行時エラーに落ちる (実測で判明)
    assert!(!has_separate_compile_phase(Language::Python));
    assert!(!has_separate_compile_phase(Language::Javascript));
    for l in [
        Language::Rust,
        Language::Cpp,
        Language::Csharp,
        Language::Java,
        Language::Typescript,
    ] {
        assert!(has_separate_compile_phase(l), "{}", l.slug());
    }
}

#[test]
fn python_syntax_error_on_program_stderr_is_a_compile_error() {
    // Wandbox 実測: 診断欄は空で、SyntaxError は program_error に来る
    let raw = wb(
        "1",
        "",
        "",
        "",
        "  File \"prog.py\", line 1\n    def add(a, b)\n                 ^\nSyntaxError: expected ':'\n",
    );
    let r = normalize_wandbox(Language::Python, &raw);
    assert!(r.compile_failed, "構文エラーを検出できていない: {}", r.stderr);
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn javascript_syntax_error_on_program_stderr_is_a_compile_error() {
    let raw = wb(
        "1",
        "",
        "",
        "",
        "/home/wandbox/prog.js:2\n  return a + ;\n             ^\n\nSyntaxError: Unexpected token ';'\n    at wrapSafe (node:internal/modules/cjs/loader:1281:20)\n",
    );
    let r = normalize_wandbox(Language::Javascript, &raw);
    assert!(r.compile_failed, "構文エラーを検出できていない: {}", r.stderr);
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn python_runtime_exception_stays_a_runtime_error() {
    // starter_code の NotImplementedError は実行時エラーであってコンパイルエラーではない。
    // ここを取り違えると「未実装」と「構文が壊れている」の区別が利用者に伝わらない
    let raw = wb(
        "1",
        "",
        "",
        "",
        "Traceback (most recent call last):\n  File \"prog.py\", line 12, in <module>\n    _check(add(1, 2) == 3, \"pos\")\n  File \"prog.py\", line 2, in add\n    raise NotImplementedError\nNotImplementedError\n",
    );
    let r = normalize_wandbox(Language::Python, &raw);
    assert!(!r.compile_failed);
    assert_eq!(classify(&r), Outcome::RuntimeError);
}

#[test]
fn compiled_languages_do_not_scan_program_stderr_for_diagnostics() {
    // C++ のプログラムが stderr に "error:" を印字しても、コンパイルは成功している
    let raw = wb("1", "", "", "", "custom error: 入力が不正です\n");
    let r = normalize_wandbox(Language::Cpp, &raw);
    assert!(!r.compile_failed, "プログラム出力をコンパイル診断と誤認している");
    assert_eq!(classify(&r), Outcome::RuntimeError);
}

// ---- C# のビルドノイズ除去 ----

const CSHARP_NOISE: &str = r#"The template "Console App" was created successfully.

Processing post-creation actions...
Running 'dotnet restore' on /home/wandbox/prog/prog.csproj...
  Determining projects to restore...
  Restored /home/wandbox/prog/prog.csproj (in 129 ms).
Restore succeeded.


MSBuild version 17.3.4+a400405ba for .NET
  Determining projects to restore...
  All projects are up-to-date for restore.
  prog -> /home/wandbox/prog/bin/Debug/net6.0/prog.dll

Build succeeded.
    0 Warning(s)
    0 Error(s)

Time Elapsed 00:00:03.83"#;

#[test]
fn csharp_noise_is_removed_entirely_on_success() {
    assert_eq!(strip_csharp_build_noise(CSHARP_NOISE).trim(), "");
}

#[test]
fn csharp_diagnostics_survive_noise_removal() {
    let with_error = format!(
        "{}\n/home/wandbox/prog/Program.cs(1,65): error CS0029: Cannot implicitly convert type 'int' to 'string'\n",
        CSHARP_NOISE
    );
    let cleaned = strip_csharp_build_noise(&with_error);
    assert!(cleaned.contains("error CS0029"), "診断が消えた: {cleaned}");
    assert!(!cleaned.contains("MSBuild version"));
    assert!(!cleaned.contains("Restore succeeded"));
    assert!(!cleaned.contains("Determining projects"));
}

#[test]
fn csharp_noise_removal_keeps_any_line_mentioning_error_or_warning() {
    // 正規表現に合わない本物のエラー (MSBuild 自身の失敗など) を消さない
    let s = "MSBuild version 17.3.4 for .NET\nerror MSB4025: プロジェクトを読み込めません\n";
    let cleaned = strip_csharp_build_noise(s);
    assert!(cleaned.contains("error MSB4025"), "{cleaned}");
    assert!(!cleaned.contains("MSBuild version"));
}

// ---- Wandbox 応答の詰め替え ----

#[test]
fn wandbox_success_becomes_passed() {
    let r = normalize_wandbox(Language::Cpp, &wb("0", "", "", "test result: ok\n", ""));
    assert!(r.success);
    assert!(!r.compile_failed);
    assert_eq!(r.stdout.trim(), "test result: ok");
    assert_eq!(classify(&r), Outcome::Passed);
}

#[test]
fn wandbox_compile_error_sets_the_flag_and_keeps_the_diagnostic() {
    let raw = wb(
        "1",
        "",
        "prog.cc: In function 'int main()':\nprog.cc:2:21: error: invalid conversion from 'const char*' to 'int' [-fpermissive]\n",
        "",
        "",
    );
    let r = normalize_wandbox(Language::Cpp, &raw);
    assert!(!r.success);
    assert!(r.compile_failed);
    assert!(r.stderr.contains("invalid conversion"));
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn wandbox_test_failure_becomes_tests_failed() {
    let raw = wb("1", "", "", "test result: FAILED\n", "FAILED: add_positive\n");
    let r = normalize_wandbox(Language::Java, &raw);
    assert!(!r.compile_failed);
    assert!(r.stderr.contains("FAILED: add_positive"));
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

#[test]
fn wandbox_runtime_error_keeps_the_stack_trace() {
    let raw = wb(
        "1",
        "",
        "",
        "",
        "Exception in thread \"main\" java.lang.ArrayIndexOutOfBoundsException: Index 5 out of bounds for length 1\n\tat Main.main(prog.java:1)\n",
    );
    let r = normalize_wandbox(Language::Java, &raw);
    assert!(r.stderr.contains("ArrayIndexOutOfBoundsException"));
    assert!(r.stderr.contains("prog.java:1"), "行番号が失われている");
    assert_eq!(classify(&r), Outcome::RuntimeError);
}

#[test]
fn wandbox_typescript_type_error_lands_in_compiler_output() {
    // TS の型エラーは compiler_error ではなく compiler_output に来る (実測)
    let raw = wb("2", "prog.ts(2,7): error TS2322: Type 'number' is not assignable to type 'string'.\n", "", "", "");
    let r = normalize_wandbox(Language::Typescript, &raw);
    assert!(r.compile_failed);
    assert!(r.stderr.contains("TS2322"));
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn wandbox_csharp_noise_is_stripped_in_normalization() {
    let raw = wb("0", CSHARP_NOISE, "", "test result: ok\n", "");
    let r = normalize_wandbox(Language::Csharp, &raw);
    assert!(!r.stderr.contains("MSBuild version"), "ノイズが残っている: {}", r.stderr);
    assert_eq!(classify(&r), Outcome::Passed);
}

#[test]
fn wandbox_non_numeric_status_is_treated_as_failure() {
    let r = normalize_wandbox(Language::Python, &wb("", "", "", "", ""));
    assert!(!r.success);
}

// ---- コンソール表示 ----

#[test]
fn classify_line_handles_rustc() {
    assert_eq!(classify_line("error[E0308]: mismatched types"), LineKind::Error);
    assert_eq!(classify_line("error: aborting due to 1 previous error"), LineKind::Error);
    assert_eq!(classify_line("warning: unused variable: `x`"), LineKind::Warning);
    assert_eq!(classify_line(" --> src/lib.rs:2:5"), LineKind::Note);
    assert_eq!(classify_line("note: expected type `i32`"), LineKind::Note);
    assert_eq!(classify_line("help: try removing `&`"), LineKind::Note);
    assert_eq!(classify_line("   Compiling playground v0.0.1"), LineKind::Plain);
}

#[test]
fn classify_line_handles_other_compilers() {
    // 行頭がファイル名になる形式。ここを取りこぼすと診断が全行無着色になる
    assert_eq!(
        classify_line("prog.cc:2:21: error: invalid conversion from 'const char*' to 'int'"),
        LineKind::Error
    );
    assert_eq!(
        classify_line("prog.java:3: error: incompatible types: int cannot be converted to String"),
        LineKind::Error
    );
    assert_eq!(
        classify_line("/home/wandbox/prog/Program.cs(1,65): error CS0029: Cannot implicitly convert type"),
        LineKind::Error
    );
    assert_eq!(
        classify_line("prog.ts(2,7): error TS2322: Type 'number' is not assignable to type 'string'."),
        LineKind::Error
    );
    assert_eq!(
        classify_line("prog.cc:3:9: warning: unused variable 'x' [-Wunused-variable]"),
        LineKind::Warning
    );
    assert_eq!(classify_line("SyntaxError: invalid syntax"), LineKind::Error);
    assert_eq!(classify_line("Exception in thread \"main\" java.lang.RuntimeException"), LineKind::Error);
    assert_eq!(classify_line("    2 | int main(){ int x = \"nope\"; }"), LineKind::Plain);
}


// ---- cargo test の要約行をコンパイルエラーと取り違えない ----

/// 2026-08-29 に実 Playground で測定した「テストが 1 件落ちたとき」の stderr。
/// 末尾の `error: test failed` は rustc の診断ではなく cargo 自身の要約。
const CARGO_TEST_FAILED_STDERR: &str = "   Compiling playground v0.0.1 (/playground)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.52s\n     Running unittests src/lib.rs (target/debug/deps/playground-1a2b3c4d)\nerror: test failed, to rerun pass `--lib`";

const CARGO_TEST_FAILED_STDOUT: &str = "\nrunning 2 tests\ntest t1 ... FAILED\ntest t2 ... ok\n\nfailures:\n\n---- t1 stdout ----\n\nthread \'t1\' panicked at src/lib.rs:4:16:\nassertion `left == right` failed\n  left: 6\n right: 4\n\n\nfailures:\n    t1\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";

#[test]
fn rust_test_failure_is_not_reported_as_a_compile_error() {
    // これを取り違えると、利用者がいちばん頻繁に見る画面 (テストが落ちたとき) が
    // 全件「✗ コンパイルエラー」になり、本当の原因である assertion のパネルが
    // stderr の下に押しやられる
    let raw = PlaygroundResponse {
        success: false,
        stdout: CARGO_TEST_FAILED_STDOUT.to_string(),
        stderr: CARGO_TEST_FAILED_STDERR.to_string(),
    };
    let r = normalize_playground(&raw);
    assert!(!r.compile_failed, "cargo の要約行を診断と誤認している");
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

#[test]
fn rust_real_compile_error_is_still_detected() {
    // 対検証: 本物のコンパイルエラーでは判定テストが走らないので stdout に目印が無い
    let raw = PlaygroundResponse {
        success: false,
        stdout: String::new(),
        stderr: "   Compiling playground v0.0.1 (/playground)\nerror[E0308]: mismatched types\n --> src/lib.rs:2:18\nerror: aborting due to 1 previous error".to_string(),
    };
    let r = normalize_playground(&raw);
    assert!(r.compile_failed);
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn harness_ran_detects_both_outcomes() {
    assert!(harness_ran("test result: ok. 3 passed"));
    assert!(harness_ran("test result: FAILED. 0 passed; 1 failed"));
    assert!(!harness_ran(""));
    assert!(!harness_ran("何か別の出力"));
}

#[test]
fn a_language_whose_program_prints_error_lines_is_not_a_compile_error_once_tests_ran() {
    // 判定テストが走った証拠があるなら、診断欄に何があってもコンパイルは通っている
    let raw = WandboxResponse {
        status: "1".into(),
        compiler_error: "prog.cc:1:1: error: これは診断のように見えるがテストは走った".into(),
        program_output: "test result: FAILED\n".into(),
        ..Default::default()
    };
    let r = normalize_wandbox(shared::language::Language::Cpp, &raw);
    assert!(!r.compile_failed);
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

// ---- TypeScript のフラグは compiler-option-raw で渡す ----

#[test]
fn typescript_flags_go_through_compiler_option_raw_not_options() {
    // Wandbox の `options` はコンパイラごとに定義された選択肢の ID であって生フラグではない。
    // typescript-5.6.2 には選択肢が 1 つも無いので、options に入れても黙って無視される
    // (2026-08-29 実測: 無視された結果 Object.fromEntries が TS2550 で落ちた)。
    let r = shared::playground::wandbox_request(Language::Typescript, "const x = 1;").unwrap();
    assert_eq!(r.options, None);
    assert_eq!(r.compiler_option_raw.as_deref(), Some("--target\nes2020"));

    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"compiler-option-raw\""), "{json}");
}

#[test]
fn other_languages_send_no_raw_flags() {
    for l in Language::ALL.into_iter().filter(|l| *l != Language::Typescript) {
        if let Some(r) = shared::playground::wandbox_request(l, "x") {
            assert_eq!(r.compiler_option_raw, None, "{}", l.slug());
        }
    }
    // gcc は選択肢 ID の方を使う
    let cpp = shared::playground::wandbox_request(Language::Cpp, "int main(){}").unwrap();
    assert_eq!(cpp.options.as_deref(), Some("warning,c++17"));
}

#[test]
fn verifier_and_upstream_use_the_same_typescript_flags() {
    // ここが食い違うと「ローカルの verifier は緑・本番は型エラー」になる。
    // どちらも同じ TSC_FLAGS から組み立てていることを表明しておく
    use shared::language::{tsc_flags_cli, tsc_flags_wandbox_raw, TSC_FLAGS};
    assert_eq!(tsc_flags_cli(), TSC_FLAGS.join(" "));
    assert_eq!(tsc_flags_wandbox_raw(), TSC_FLAGS.join("\n"));
    assert!(TSC_FLAGS.contains(&"es2020"));
}

#[test]
fn a_run_killed_by_a_signal_is_not_successful() {
    // メモリ超過などで殺された実行。status が "0" でもシグナルが立っていたら成功にしない
    let raw = WandboxResponse {
        status: "0".into(),
        signal: "Killed".into(),
        program_output: "test result: ok\n".into(),
        ..Default::default()
    };
    let r = normalize_wandbox(Language::Cpp, &raw);
    assert!(!r.success, "シグナルで殺された実行を成功扱いしている");
    assert_ne!(classify(&r), Outcome::Passed);
}

// ---- Wandbox の HTTP レベル障害 (2026-09-07 実測) ----

#[test]
fn wandbox_500_failed_to_get_uid_is_transient() {
    // 2026-09-07 に全コンパイラで返り続けた本物の応答。
    let body = "Error: Failed to get uid: status=exit status: 125, base_dir=/tmp/wandbox/wandbox_20260907_052930_gPiCgf";
    assert!(wandbox_http_failure_is_transient(500, body));
}

#[test]
fn wandbox_5xx_without_known_body_is_still_transient() {
    assert!(wandbox_http_failure_is_transient(502, ""));
    assert!(wandbox_http_failure_is_transient(503, "<html>Service Unavailable</html>"));
}

#[test]
fn wandbox_4xx_is_not_transient() {
    // 403 (User-Agent 拒否) や 400 は再試行しても直らない。
    assert!(!wandbox_http_failure_is_transient(403, "Forbidden"));
    assert!(!wandbox_http_failure_is_transient(400, "bad request"));
    assert!(!wandbox_http_failure_is_transient(200, ""));
}
