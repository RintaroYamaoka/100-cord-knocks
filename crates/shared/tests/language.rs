use shared::language::Language;

#[test]
fn slug_roundtrips() {
    for l in Language::ALL {
        assert_eq!(Language::from_slug(l.slug()), Some(l));
    }
}

#[test]
fn unknown_slug_is_none() {
    assert_eq!(Language::from_slug("ruby"), None);
    assert_eq!(Language::from_slug(""), None);
    // 大文字小文字は区別する (URL / ディレクトリ名に使うため)
    assert_eq!(Language::from_slug("Rust"), None);
}

#[test]
fn slugs_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for l in Language::ALL {
        assert!(seen.insert(l.slug()), "slug 重複: {}", l.slug());
    }
}

#[test]
fn json_representation_matches_slug() {
    // JSON 表現とディレクトリ名がずれると、問題データのパスとフィールドが食い違う
    for l in Language::ALL {
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(json, format!("\"{}\"", l.slug()));
        let back: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
    }
}

#[test]
fn python_uses_hash_comment_others_use_slashes() {
    // Python に `//` を挿入すると SyntaxError になり、その言語の問題が全滅する
    assert_eq!(Language::Python.line_comment(), "#");
    for l in Language::ALL.into_iter().filter(|l| *l != Language::Python) {
        assert_eq!(l.line_comment(), "//", "{} の行コメント", l.slug());
    }
}

#[test]
fn source_file_names_are_distinct_per_language() {
    let mut seen = std::collections::HashSet::new();
    for l in Language::ALL {
        assert!(seen.insert(l.source_file_name()), "{} のファイル名が重複", l.slug());
    }
    // Wandbox のファイル名固定が Java の public class 禁止の根拠 (ADR 0002)
    assert_eq!(Language::Java.source_file_name(), "prog.java");
}

#[test]
fn labels_are_human_readable() {
    assert_eq!(Language::Cpp.label(), "C++");
    assert_eq!(Language::Csharp.label(), "C#");
    for l in Language::ALL {
        assert!(!l.label().is_empty());
    }
}
