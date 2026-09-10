//! 公開 API を通した統合スモーク: 問題 → 提出コード合成 → 実行結果分類までの一連の流れ。

use shared::language::Language;
use shared::contract::{classify, ExecuteRequest, Outcome};
use shared::runner::{normalize_runner, RunnerOutput};
use shared::problem::compose_submission;

#[test]
fn rust_submission_flow_composes_and_classifies() {
    let user_code = "pub fn answer() -> i32 { 41 }";
    let hidden_tests = "#[test]\nfn t() { assert_eq!(answer(), 42); }";
    let submission = compose_submission(Language::Rust, user_code, hidden_tests);
    let req = ExecuteRequest::judge(Language::Rust, &submission);
    assert_eq!(req.language, Language::Rust);
    assert!(req.code.contains("pub fn answer"));

    let raw = RunnerOutput {
        program_stdout: "running 1 test\ntest t ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed".into(),
        program_stderr: "error: test failed, to rerun pass `--lib`".into(),
        exit_code: Some(101),
        ..Default::default()
    };
    assert_eq!(classify(&normalize_runner(Language::Rust, &raw)), Outcome::TestsFailed);
}

#[test]
fn python_submission_flow_composes_and_classifies() {
    let user_code = "def answer():\n    return 41";
    let hidden_tests = "import sys\nif answer() != 42:\n    print(\"test result: FAILED\")\n    sys.exit(1)\nprint(\"test result: ok\")";
    let submission = compose_submission(Language::Python, user_code, hidden_tests);
    // 区切りが `#` でないと、この提出コードは実行前に SyntaxError になる
    assert!(submission.contains("# ====="));

    let raw = RunnerOutput {
        program_stdout: "test result: FAILED\n".into(),
        exit_code: Some(1),
        ..Default::default()
    };
    assert_eq!(classify(&normalize_runner(Language::Python, &raw)), Outcome::TestsFailed);
}

#[test]
fn every_language_can_round_trip_a_passing_run() {
    for lang in Language::ALL {
        let raw = RunnerOutput {
            program_stdout: "test result: ok\n".into(),
            exit_code: Some(0),
            ..Default::default()
        };
        let resp = normalize_runner(lang, &raw);
        assert_eq!(classify(&resp), Outcome::Passed, "{}", lang.slug());
    }
}
