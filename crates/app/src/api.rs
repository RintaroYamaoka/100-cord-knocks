//! バックエンド /api/execute と問題データ (静的 JSON) への通信層。

use shared::language::Language;
use shared::contract::{ExecuteRequest, ExecuteResponse};
use shared::problem::{Level, Problem};

/// エラー応答ボディ ({"error": "..."}) から利用者向けメッセージを取り出す。
pub fn error_message_from_body(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_else(|| format!("実行サービスへの接続に失敗しました (HTTP {status})"))
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_problems(language: Language, level: Level) -> Result<Vec<Problem>, String> {
    let url = shared::problem::problems_url(language, level);
    let resp = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("問題データを取得できませんでした: {e}"))?;
    if !resp.ok() {
        return Err(format!("問題データを取得できませんでした (HTTP {})", resp.status()));
    }
    resp.json()
        .await
        .map_err(|e| format!("問題データを解釈できませんでした: {e}"))
}

/// その言語・レベルの問題データが配信されているか。
///
/// セレクタに未完成の言語を出さないための存在確認。中身は要らないので HEAD で問い、
/// 21 回叩いても起動が重くならないようにしている (GET だと数 MB を無駄に読む)。
/// 失敗はすべて「無い」に倒す — 無いものを出すより、あるものを出し損ねるほうが安全。
#[cfg(target_arch = "wasm32")]
pub async fn fetch_language_manifest() -> Option<Vec<shared::language::Language>> {
    // 収録済み言語の一覧。1 リクエストで済むので、起動直後からセレクタが埋まる
    let resp = gloo_net::http::Request::get(shared::problem::MANIFEST_URL)
        .send()
        .await
        .ok()?;
    if !resp.ok() {
        return None;
    }
    let text = resp.text().await.ok()?;
    crate::lang::languages_from_manifest(&text)
}

#[cfg(target_arch = "wasm32")]
pub async fn execute(req: &ExecuteRequest) -> Result<ExecuteResponse, String> {
    let resp = gloo_net::http::Request::post("/api/execute")
        .json(req)
        .map_err(|e| format!("リクエストを構築できませんでした: {e}"))?
        .send()
        .await
        .map_err(|_| "実行サービスに接続できませんでした。ネットワークを確認してください".to_string())?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(error_message_from_body(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| "実行サービスの応答を解釈できませんでした".to_string())
}

// ---- host スタブ (テストビルド用。実行は wasm でのみ成立する) ----

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_problems(_language: Language, _level: Level) -> Result<Vec<Problem>, String> {
    Err("wasm32 専用".into())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_language_manifest() -> Option<Vec<shared::language::Language>> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn execute(_req: &ExecuteRequest) -> Result<ExecuteResponse, String> {
    Err("wasm32 専用".into())
}
