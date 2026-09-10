//! 言語選択の純ロジック。
//!
//! 「未完成の言語をセレクタに出さない」「保存した選択を復元する」「Rust 以外に
//! rustc 専用のリンクを張らない」は、いずれも UI を動かさずに決まる規則なので
//! ここで固定する。

use app::lang::{
    backend_label, console_idle_hint, console_running_hint, initial_language,
    links_error_codes, resolve_selection, selector_languages,
};
use shared::language::Language;

#[test]
fn initial_language_restores_stored_slug() {
    assert_eq!(initial_language(Some("python")), Language::Python);
}

#[test]
fn initial_language_falls_back_to_rust() {
    // 未保存・壊れた値・かつて存在しない slug、いずれも従来の既定へ
    assert_eq!(initial_language(None), Language::Rust);
    assert_eq!(initial_language(Some("")), Language::Rust);
    assert_eq!(initial_language(Some("cobol")), Language::Rust);
}

#[test]
fn selection_moves_off_a_language_that_has_no_data() {
    let available = [Language::Rust, Language::Python];
    assert_eq!(resolve_selection(Language::Java, &available), Language::Rust);
    // 有効ならそのまま
    assert_eq!(
        resolve_selection(Language::Python, &available),
        Language::Python
    );
}

#[test]
fn selection_is_left_alone_when_availability_is_unknown() {
    // 確認が機能しなかった環境で選択を動かすと、動いていた言語まで巻き添えになる
    assert_eq!(resolve_selection(Language::Rust, &[]), Language::Rust);
}

#[test]
fn selector_shows_the_available_languages() {
    let available = [Language::Rust, Language::Cpp];
    assert_eq!(
        selector_languages(&available, Language::Rust, true),
        vec![Language::Rust, Language::Cpp]
    );
}

#[test]
fn selector_falls_back_to_the_loaded_language_when_probing_failed() {
    assert_eq!(
        selector_languages(&[], Language::Rust, true),
        vec![Language::Rust]
    );
    // 何も読めていなくても**空にはしない**。
    // option が 0 個だと、保存された言語のデータが無くなった利用者に
    // 「別の言語に戻す手段」が UI 上から消えてしまう (症状は「一覧が空」だけで
    // 原因が見えない)。必ず存在が保証されている Rust を残す。
    assert_eq!(
        selector_languages(&[], Language::Rust, false),
        vec![Language::Rust]
    );
}

#[test]
fn error_code_links_are_rust_only() {
    assert!(links_error_codes(Language::Rust));
    for lang in Language::ALL {
        if lang != Language::Rust {
            assert!(!links_error_codes(lang), "{} でリンクを張っている", lang.label());
        }
    }
}

#[test]
fn backend_label_names_the_service_that_actually_runs_the_code() {
    // 7 言語とも同じ基盤 (自前イメージの Vercel Sandbox) で動く (ADR 0003)
    for lang in Language::ALL {
        assert_eq!(backend_label(lang), "Vercel Sandbox", "{}", lang.slug());
    }
}

#[test]
fn console_wording_is_not_hardcoded_to_rust() {
    // 待機中の文言はどの言語でも成り立つこと
    assert!(!console_idle_hint().contains("rustc"));
    assert!(!console_idle_hint().contains("Rust"));

    // 実行中の文言は選択中の言語に追従すること
    let py = console_running_hint(Language::Python);
    assert!(py.contains("Python"), "{py}");
    assert!(py.contains("Sandbox"), "{py}");
    assert!(!py.contains("Rust"), "{py}");

    let rs = console_running_hint(Language::Rust);
    assert!(rs.contains("Rust"), "{rs}");
    assert!(rs.contains("Sandbox"), "{rs}");
}

#[test]
fn selector_never_becomes_empty() {
    // option が 0 個になると、保存された言語のデータが無くなった利用者に
    // 「別の言語に戻す手段」が UI 上から消える
    let none: Vec<Language> = vec![];
    let r = selector_languages(&none, Language::Python, false);
    assert!(!r.is_empty(), "セレクタが空になった");
    assert!(r.contains(&Language::Rust), "既定の Rust が残っていない: {r:?}");

    let r = selector_languages(&none, Language::Rust, false);
    assert_eq!(r, vec![Language::Rust]);
}

#[test]
fn selector_prefers_confirmed_languages_when_available() {
    let avail = vec![Language::Rust, Language::Python];
    assert_eq!(
        selector_languages(&avail, Language::Python, true),
        vec![Language::Rust, Language::Python]
    );
}

// ---- マニフェストからの言語一覧 ----

#[test]
fn manifest_lists_languages_with_all_three_levels() {
    use app::lang::languages_from_manifest;
    let json = r#"{"languages":[
        {"slug":"rust","levels":["beginner","intermediate","advanced"]},
        {"slug":"cpp","levels":["beginner","intermediate","advanced"]}
    ]}"#;
    assert_eq!(
        languages_from_manifest(json),
        Some(vec![Language::Rust, Language::Cpp])
    );
}

#[test]
fn manifest_drops_languages_missing_a_level() {
    use app::lang::languages_from_manifest;
    // 3 レベル揃っていない言語は出さない (選んだ瞬間に空の一覧を見せることになる)
    let json = r#"{"languages":[
        {"slug":"rust","levels":["beginner","intermediate","advanced"]},
        {"slug":"java","levels":["beginner"]}
    ]}"#;
    assert_eq!(languages_from_manifest(json), Some(vec![Language::Rust]));
}

#[test]
fn manifest_ignores_unknown_slugs() {
    use app::lang::languages_from_manifest;
    let json = r#"{"languages":[
        {"slug":"cobol","levels":["beginner","intermediate","advanced"]},
        {"slug":"rust","levels":["beginner","intermediate","advanced"]}
    ]}"#;
    assert_eq!(languages_from_manifest(json), Some(vec![Language::Rust]));
}

#[test]
fn manifest_keeps_the_canonical_language_order() {
    use app::lang::languages_from_manifest;
    // 表示順は Language::ALL の順に揃える (マニフェストの並びに引きずられない)
    let json = r#"{"languages":[
        {"slug":"python","levels":["beginner","intermediate","advanced"]},
        {"slug":"rust","levels":["beginner","intermediate","advanced"]},
        {"slug":"cpp","levels":["beginner","intermediate","advanced"]}
    ]}"#;
    assert_eq!(
        languages_from_manifest(json),
        Some(vec![Language::Rust, Language::Cpp, Language::Python])
    );
}

#[test]
fn broken_manifest_yields_none_not_an_empty_list() {
    use app::lang::languages_from_manifest;
    // None = 「判定できなかった」。空 Vec = 「1 言語も無い」と区別する。
    // 混同すると、取得に失敗しただけでセレクタが空になる
    assert_eq!(languages_from_manifest("{壊れ"), None);
    assert_eq!(languages_from_manifest(""), None);
    assert_eq!(languages_from_manifest(r#"{"languages":"not an array"}"#), None);
}

#[test]
fn manifest_with_no_languages_is_an_empty_list() {
    use app::lang::languages_from_manifest;
    assert_eq!(languages_from_manifest(r#"{"languages":[]}"#), Some(vec![]));
}
