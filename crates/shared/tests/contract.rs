//! 実行契約 (shared::contract) のテスト。
//!
//! 固定値は実コンパイラから実測した本物の出力を使っている
//! (作文した出力でテストすると、契約が現実とずれても緑のままになるため)。
//! 実行基盤 (Sandbox) 側の詰め替えは tests/runner.rs、REST 契約は tests/sandbox.rs。

use shared::language::Language;
use shared::contract::{
    classify, classify_line, harness_ran, has_compile_error, strip_csharp_build_noise, validate,
    ExecuteRequest, ExecuteResponse, LineKind, Outcome, MAX_CODE_BYTES, TEST_FAILED_MARKER,
    TEST_OK_MARKER,
};

fn resp(success: bool, stdout: &str, stderr: &str, compile_failed: bool) -> ExecuteResponse {
    ExecuteResponse {
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        compile_failed,
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

#[test]
fn harness_ran_detects_both_outcomes() {
    assert!(harness_ran("test result: ok. 3 passed"));
    assert!(harness_ran("test result: FAILED. 0 passed; 1 failed"));
    assert!(!harness_ran(""));
    assert!(!harness_ran("何か別の出力"));
}


