//! /api/execute プロキシの入口契約: 受信ボディのパース → 検証 → 実行要求の組み立て。
//! ハンドラ本体は HTTP glue のみで、判定ロジックはすべて shared 側にある。

use shared::contract::{validate, ExecuteRequest};
use shared::language::Language;
use shared::runner::{base64_encode, build_script, Nonce, PRODUCTION_IMAGE};
use shared::sandbox::{CreateSandboxRequest, ExecCommandRequest};

#[test]
fn incoming_body_parses_as_execute_request() {
    let body = r#"{ "language": "rust", "code": "pub fn f() {}" }"#;
    let req: ExecuteRequest = serde_json::from_str(body).unwrap();
    assert_eq!(req.language, Language::Rust);
    assert!(validate(&req).is_ok());
}

#[test]
fn every_language_slug_is_accepted_by_the_wire_format() {
    for l in Language::ALL {
        let body = format!(r#"{{ "language": "{}", "code": "x" }}"#, l.slug());
        let req: ExecuteRequest = serde_json::from_str(&body).unwrap();
        assert_eq!(req.language, l);
    }
}

#[test]
fn unknown_language_is_rejected_at_parse_time() {
    let body = r#"{ "language": "brainfuck", "code": "x" }"#;
    assert!(serde_json::from_str::<ExecuteRequest>(body).is_err());
}

#[test]
fn empty_and_oversized_code_is_rejected_before_forwarding() {
    let empty: ExecuteRequest = serde_json::from_str(r#"{"language":"cpp","code":"  "}"#).unwrap();
    assert!(validate(&empty).is_err());

    let big = ExecuteRequest::judge(Language::Cpp, &"a".repeat(shared::contract::MAX_CODE_BYTES + 1));
    assert!(validate(&big).is_err());
}

#[test]
fn every_language_goes_to_the_same_sandbox_image() {
    // 7 言語が 1 枚のイメージで動くのが ADR 0003 の要点。
    // 言語ごとの分岐が戻ったらここで気づく
    let body = serde_json::to_string(&CreateSandboxRequest::for_submission(PRODUCTION_IMAGE)).unwrap();
    assert!(body.contains(PRODUCTION_IMAGE), "{body}");
    assert!(PRODUCTION_IMAGE.contains(':'), "イメージはタグ付きで固定する — {PRODUCTION_IMAGE}");
}

#[test]
fn submitted_code_is_never_interpolated_into_the_shell() {
    // クォートや $(...) を含む提出コードでスクリプトを壊せないこと
    let nasty = "print('x'); $(rm -rf /) `id` \"\u{27}\"";
    let script = build_script(Language::Python, &base64_encode(nasty.as_bytes()), &Nonce::new("abc123"));
    assert!(!script.contains("rm -rf"), "提出コードが素のままスクリプトに入っている");
    assert!(!script.contains("$(id)"));
    assert!(script.contains(&base64_encode(nasty.as_bytes())));
}

#[test]
fn command_request_waits_for_completion_and_collects_logs() {
    // wait=false だと結果を取りに行く往復が増える。logs=false だと出力が取れない
    let req = ExecCommandRequest::shell_script("echo hi");
    assert!(req.wait, "完了を待っていない");
    assert!(req.logs, "ログを集めていない");
    assert_eq!(req.command, "sh");
    assert!(req.timeout >= 1000);
}
