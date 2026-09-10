use app::console::{outcome_banner_parts, outcome_note, split_error_codes, ConsoleSegment};
use shared::contract::Outcome;

#[test]
fn splits_line_around_error_codes() {
    let segs = split_error_codes("error[E0308]: mismatched types");
    assert_eq!(
        segs,
        vec![
            ConsoleSegment::Text("error[".into()),
            ConsoleSegment::ErrorCode("E0308".into()),
            ConsoleSegment::Text("]: mismatched types".into()),
        ]
    );
}

#[test]
fn plain_line_is_single_text_segment() {
    let segs = split_error_codes("   Compiling playground v0.0.1");
    assert_eq!(segs, vec![ConsoleSegment::Text("   Compiling playground v0.0.1".into())]);
}

#[test]
fn multiple_codes_in_one_line() {
    let segs = split_error_codes("see error[E0308] and error[E0502]");
    let codes: Vec<_> = segs
        .iter()
        .filter_map(|s| match s {
            ConsoleSegment::ErrorCode(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(codes, vec!["E0308", "E0502"]);
}

#[test]
fn bracket_without_valid_code_stays_text() {
    let segs = split_error_codes("error[X123]: not a code");
    assert!(segs.iter().all(|s| matches!(s, ConsoleSegment::Text(_))));
}

#[test]
fn every_outcome_has_a_banner() {
    // Outcome が増えたらここで落ちる (表示の付け忘れをコンパイルではなくテストで拾う)
    for outcome in [
        Outcome::Passed,
        Outcome::TestsFailed,
        Outcome::CompileError,
        Outcome::RuntimeError,
        Outcome::NoTestsRun,
    ] {
        let (class, label) = outcome_banner_parts(outcome);
        assert!(class.starts_with("outcome-banner"), "{class}");
        assert!(!label.is_empty());
    }
}

#[test]
fn no_tests_run_is_not_shown_as_a_pass() {
    let (class, label) = outcome_banner_parts(Outcome::NoTestsRun);
    let (passed_class, passed_label) = outcome_banner_parts(Outcome::Passed);
    assert_ne!(class, passed_class);
    assert_ne!(label, passed_label);
    assert!(label.contains("テスト"), "{label}");
    assert!(!label.contains("正解"), "{label}");
}

#[test]
fn no_tests_run_explains_why_nothing_ran() {
    let note = outcome_note(Outcome::NoTestsRun).expect("NoTestsRun には理由の説明が要る");
    // 「途中で終了したからテストに到達していない」ことが読み取れること
    assert!(note.contains("実行されませんでした"), "{note}");
    assert!(note.contains("終了"), "{note}");
}

#[test]
fn other_outcomes_have_no_extra_note() {
    for outcome in [
        Outcome::Passed,
        Outcome::TestsFailed,
        Outcome::CompileError,
        Outcome::RuntimeError,
    ] {
        assert!(outcome_note(outcome).is_none(), "{outcome:?}");
    }
}
