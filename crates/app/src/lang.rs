//! 言語選択まわりの純ロジック。
//!
//! 「どの言語をセレクタに出すか」「起動時に何を選ぶか」「どの文言を出すか」を
//! UI 配線とネットワークから切り離し、host でテストできる形に置く。

use std::collections::HashSet;

use shared::language::Language;
use shared::problem::Level;

/// 選択中の言語を保存する localStorage キー。進捗キーと同じ名前空間に置く。
pub const LANGUAGE_STORAGE_KEY: &str = "rust100knocks.language.v1";

/// 収録済み言語のマニフェスト (`/data/problems/index.json`) を解釈する。
///
/// マニフェストはビルド時に実ファイルから生成する。以前は起動時に 21 本の HEAD を
/// **逐次**投げて確かめていたが、それだと揃うまで数秒かかり、その間セレクタには
/// Rust しか出ない。回線が遅いほど長く、1 本でも瞬断すればその言語は消える。
/// リクエスト 1 本にすれば、この待ち時間も取りこぼしも構造的に無くなる。
///
/// 戻り値の `None` は「判定できなかった」で、`Some(vec![])` の「1 言語も無い」とは別。
/// 混同すると、取得に失敗しただけでセレクタが空になる。
pub fn languages_from_manifest(json: &str) -> Option<Vec<Language>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let entries = v.get("languages")?.as_array()?;

    let mut complete = HashSet::new();
    for e in entries {
        let Some(slug) = e.get("slug").and_then(|s| s.as_str()) else { continue };
        let Some(lang) = Language::from_slug(slug) else { continue };
        let levels: HashSet<&str> = e
            .get("levels")
            .and_then(|l| l.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        if Level::ALL.iter().all(|lv| levels.contains(lv.slug())) {
            complete.insert(lang);
        }
    }
    // 表示順は Language::ALL に揃える (マニフェストの並びに引きずられない)
    Some(Language::ALL.into_iter().filter(|l| complete.contains(l)).collect())
}


/// 起動時に選ぶ言語。まだ存在確認が済んでいない段階で呼ばれるので、
/// 保存値が読めればそれ、駄目なら Rust (従来からの既定) に落とす。
pub fn initial_language(stored: Option<&str>) -> Language {
    stored
        .and_then(Language::from_slug)
        .unwrap_or(Language::Rust)
}

/// 存在確認が出揃ったあと、選択を有効な言語へ寄せる。
///
/// 一覧が空のときは動かさない。空になるのは「確認そのものが機能しなかった」場合
/// (HEAD が通らない配信環境など) で、そこで選択を触ると今まで動いていた Rust まで
/// 巻き添えで壊れる。
pub fn resolve_selection(current: Language, available: &[Language]) -> Language {
    if available.is_empty() || available.contains(&current) {
        current
    } else {
        available[0]
    }
}

/// 言語セレクタに出す一覧。
///
/// 確認が効かなかった環境 (`available` が空) では、実際にデータが読めている
/// 選択中の言語だけを出す。空の一覧を出して「言語が 1 つも無い」ように
/// 見せるより、確実に動く 1 つを出すほうが実態に近い。
pub fn selector_languages(
    available: &[Language],
    current: Language,
    current_loaded: bool,
) -> Vec<Language> {
    if !available.is_empty() {
        return available.to_vec();
    }
    if current_loaded {
        return vec![current];
    }
    // ここに来るのは「どの言語の存在も確認できず、現在の言語も読めていない」とき。
    // 空の Vec を返すと option が 0 個になり、**別の言語に戻す手段が UI から消える**
    // (保存された言語のデータが無くなった利用者が詰む)。必ず既定の Rust を残す。
    if current == Language::Rust {
        vec![Language::Rust]
    } else {
        vec![Language::Rust, current]
    }
}

/// 実行基盤の表示名 (コンソールの文言用)。
///
/// 7 言語すべて同じ基盤 (Vercel Sandbox の自前イメージ) で動くので言語に依らない。
/// 引数を残してあるのは、将来また言語ごとに分かれたときに呼び出し側を変えずに済むため。
pub fn backend_label(_language: Language) -> &'static str {
    "Vercel Sandbox"
}

/// 実行前のコンソール placeholder。言語に依らない文言にする。
pub fn console_idle_hint() -> &'static str {
    "コードを書いて「実行して判定」(Ctrl+Enter) を押すと、ここに実物の診断出力と判定結果が表示されます。"
}

/// 実行中のコンソール placeholder。どこで動いているかを言語ごとに正しく出す。
pub fn console_running_hint(language: Language) -> String {
    format!(
        "{} で {} のコードを実行しています…",
        backend_label(language),
        language.label()
    )
}

/// エラーコードを公式ドキュメントへリンクしてよい言語か。
///
/// rustc の `doc.rust-lang.org/error_codes/` に相当する「コード別の公式ページ」は
/// 他の 6 言語には無い。リンクにすると 404 に飛ばすことになるので素のテキストで出す。
pub fn links_error_codes(language: Language) -> bool {
    language == Language::Rust
}
