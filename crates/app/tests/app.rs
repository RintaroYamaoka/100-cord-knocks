//! app.rs のうち host で検証できる純ロジック (進捗遷移規則・進捗キーの通し方) のテスト。
//! UI 配線は Playwright スクリーンショットで確認する。

use app::app::code_for;
use app::next_status;
use shared::language::Language;
use shared::contract::Outcome;
use shared::problem::{Level, Problem};
use shared::progress::{progress_key, ProblemStatus, ProgressEntry, ProgressMap};

fn problem(language: Language, id: &str) -> Problem {
    Problem {
        id: id.to_string(),
        language,
        level: Level::Beginner,
        title: "t".into(),
        description_md: "d".into(),
        starter_code: "STARTER".into(),
        hidden_tests: "T".into(),
        answer_code: "A".into(),
        explanation_md: "e".into(),
        hints: vec![],
        tags: vec![],
    }
}

fn draft(code: &str) -> ProgressEntry {
    ProgressEntry {
        status: ProblemStatus::Unanswered,
        saved_code: Some(code.to_string()),
        updated_at_ms: 0.0,
    }
}

#[test]
fn passing_always_marks_passed() {
    assert_eq!(next_status(ProblemStatus::Unanswered, Outcome::Passed), ProblemStatus::Passed);
    assert_eq!(next_status(ProblemStatus::Attempted, Outcome::Passed), ProblemStatus::Passed);
}

#[test]
fn failure_marks_attempted_but_never_downgrades_passed() {
    assert_eq!(
        next_status(ProblemStatus::Unanswered, Outcome::CompileError),
        ProblemStatus::Attempted
    );
    assert_eq!(
        next_status(ProblemStatus::Unanswered, Outcome::TestsFailed),
        ProblemStatus::Attempted
    );
    // 一度正解した問題はやり直して失敗しても「正解済み」のまま
    assert_eq!(next_status(ProblemStatus::Passed, Outcome::TestsFailed), ProblemStatus::Passed);
}

#[test]
fn tests_that_never_ran_do_not_count_as_a_pass() {
    // exit(0) でテストを飛ばした提出を「正解」にしないのが NoTestsRun の存在理由
    assert_eq!(
        next_status(ProblemStatus::Unanswered, Outcome::NoTestsRun),
        ProblemStatus::Attempted
    );
    assert_eq!(
        next_status(ProblemStatus::Passed, Outcome::NoTestsRun),
        ProblemStatus::Passed
    );
}

#[test]
fn draft_is_looked_up_by_language_scoped_key() {
    let p = problem(Language::Python, "b001");
    let mut map = ProgressMap::new();
    map.insert(progress_key(&p), draft("PY_DRAFT"));
    assert_eq!(code_for(&p, &map), "PY_DRAFT");
}

#[test]
fn drafts_do_not_leak_between_languages_sharing_an_id() {
    let rust = problem(Language::Rust, "b001");
    let python = problem(Language::Python, "b001");
    let mut map = ProgressMap::new();
    map.insert(progress_key(&rust), draft("RUST_DRAFT"));

    assert_eq!(code_for(&rust, &map), "RUST_DRAFT");
    // 同じ id でも言語が違えば別問題。starter に戻ること
    assert_eq!(code_for(&python, &map), "STARTER");
}

#[test]
fn a_bare_id_key_is_not_picked_up_as_a_draft() {
    // 旧形式のキーが移行されないまま残っていても、素の id で引き当ててはいけない
    let p = problem(Language::Rust, "b001");
    let mut map = ProgressMap::new();
    map.insert("b001".to_string(), draft("LEGACY"));
    assert_eq!(code_for(&p, &map), "STARTER");
}

#[test]
fn missing_draft_falls_back_to_starter_code() {
    let p = problem(Language::Cpp, "i042");
    assert_eq!(code_for(&p, &ProgressMap::new()), "STARTER");
}
