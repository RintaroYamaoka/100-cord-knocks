//! 判定契約の見本コード (shared::fixtures) のテスト。
//!
//! 見本は「実行基盤を差し替えたときに、7 言語の判定がまだ成立しているか」を
//! 確かめるための最小セット。ここが欠けると、実行基盤の検証が言語ごとに
//! 書き散らされて食い違う。

use shared::fixtures;
use shared::language::Language;
use shared::contract::{TEST_FAILED_MARKER, TEST_OK_MARKER};

#[test]
fn every_language_has_a_full_set() {
    for lang in Language::ALL {
        let f = fixtures::for_language(lang);
        for (name, code) in [
            ("answer", f.answer),
            ("starter", f.starter),
            ("broken", f.broken),
            ("hidden_tests", f.hidden_tests),
        ] {
            assert!(!code.trim().is_empty(), "{}: {name} が空", lang.slug());
        }
    }
}

#[test]
fn hidden_tests_emit_both_markers() {
    // ADR 0002 の判定契約: 成功は stdout に `test result: ok`、失敗は `test result: FAILED`
    for lang in Language::ALL {
        let tests = fixtures::for_language(lang).hidden_tests;
        if lang == Language::Rust {
            // cargo test が目印を出すので、問題側は #[test] を書くだけ
            assert!(tests.contains("#[test]"), "{}: #[test] が無い", lang.slug());
            continue;
        }
        assert!(tests.contains(TEST_OK_MARKER), "{}: 成功の目印が無い", lang.slug());
        assert!(tests.contains(TEST_FAILED_MARKER), "{}: 失敗の目印が無い", lang.slug());
    }
}

#[test]
fn broken_code_differs_from_the_answer() {
    for lang in Language::ALL {
        let f = fixtures::for_language(lang);
        assert_ne!(f.answer, f.broken, "{}: 壊れた版が模範解答と同じ", lang.slug());
        assert_ne!(f.answer, f.starter, "{}: 初期コードが模範解答と同じ", lang.slug());
    }
}

#[test]
fn compiler_signature_is_language_specific() {
    // 「実物のエラーログを読ませる」ことがアプリの目的なので、診断の形まで固定する
    assert_eq!(fixtures::compiler_error_signature(Language::Rust), "error[E");
    assert_eq!(fixtures::compiler_error_signature(Language::Csharp), "error CS");
    assert_eq!(fixtures::compiler_error_signature(Language::Typescript), "error TS");
    assert_eq!(fixtures::compiler_error_signature(Language::Python), "SyntaxError");
}
