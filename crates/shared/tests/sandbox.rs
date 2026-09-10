//! Vercel Sandbox REST API の契約テスト。
//!
//! 固定値は 2026-09-11 に実 API (`POST /v4/sandboxes`, `POST /v2/.../cmd`) から
//! 実測した応答をそのまま使っている。作文した応答でテストすると、上流の形が
//! 変わっても緑のままになる。

use shared::sandbox::{
    classify_sandbox_failure, parse_command_stream, pick_token, CreateSandboxRequest,
    CreateSandboxResponse, UpstreamFailure, OIDC_HEADER,
};

/// 実測した作成応答 (一部のフィールドのみ残した)。
const REAL_CREATE_RESPONSE: &str = r#"{"sandbox":{"name":"rose-pregnant-stork-ZGdLCH","currentSessionId":"sbx_StVzLvtSWnsC2ANrxcBYFo36hMTE","status":"running","persistent":false,"region":"iad1","vcpus":1,"memory":2048,"timeout":120000},"session":{"id":"sbx_StVzLvtSWnsC2ANrxcBYFo36hMTE","runtime":"node22","status":"running","vcpus":1,"memory":2048,"timeout":120000,"region":"iad1","cwd":"/vercel","projectId":"prj_xxx"},"routes":[]}"#;

/// 実測したコマンド実行のストリーム (wait=true, logs=true)。
const REAL_CMD_STREAM: &str = r#"{"command":{"id":"cmd_40f0ec2e166e4c10accf2590ecc0","name":"sh","args":["-lc","echo hello"],"cwd":"/vercel","sessionId":"sbx_StVzLvtSWnsC2ANrxcBYFo36hMTE","startedAt":1789056577952,"exitCode":null}}
{"data":"hello-stderr\n","stream":"stderr"}
{"data":"hello-stdout\n","stream":"stdout"}
{"command":{"id":"cmd_40f0ec2e166e4c10accf2590ecc0","name":"sh","args":["-lc","echo hello"],"cwd":"/vercel","sessionId":"sbx_StVzLvtSWnsC2ANrxcBYFo36hMTE","startedAt":1789056577952,"exitCode":7,"durationMs":41}}"#;

#[test]
fn create_response_carries_the_session_id() {
    let parsed: CreateSandboxResponse = serde_json::from_str(REAL_CREATE_RESPONSE).expect("実測応答が読めない");
    assert_eq!(parsed.session.id, "sbx_StVzLvtSWnsC2ANrxcBYFo36hMTE");
}

#[test]
fn create_request_sends_camel_case_fields() {
    let body = serde_json::to_string(&CreateSandboxRequest::for_submission("knocks-runtime:x")).unwrap();
    // 上流は camelCase。snake_case で送ると黙って既定値になる
    assert!(body.contains("\"networkPolicy\""), "networkPolicy が無い — {body}");
    assert!(body.contains("\"persistent\":false"), "persistent:false が無い — {body}");
    assert!(body.contains("\"deny-all\""), "外向き通信を閉じていない — {body}");
    assert!(body.contains("\"image\":\"knocks-runtime:x\""), "image が無い — {body}");
}

#[test]
fn create_request_never_persists_snapshots() {
    // 既定は persistent:true で、停止のたびに自動スナップショットが作られる。
    // Hobby のスナップショット保存は生涯 15GB なので、数回で枯れる
    let req = CreateSandboxRequest::for_submission("img");
    assert!(!req.persistent, "persistent が true");
}

#[test]
fn command_stream_splits_stdout_and_stderr_and_takes_the_exit_code() {
    let r = parse_command_stream(REAL_CMD_STREAM);
    assert_eq!(r.stdout, "hello-stdout\n");
    assert_eq!(r.stderr, "hello-stderr\n");
    assert_eq!(r.exit_code, Some(7), "最後の command イベントから終了コードを取っていない");
}

#[test]
fn command_stream_without_a_final_event_has_no_exit_code() {
    // 途中で切れたストリーム。exit_code が None なら呼び出し側が失敗として扱える
    let partial = "{\"command\":{\"id\":\"c1\",\"exitCode\":null}}\n{\"data\":\"half\",\"stream\":\"stdout\"}";
    let r = parse_command_stream(partial);
    assert_eq!(r.exit_code, None);
    assert_eq!(r.stdout, "half");
}

#[test]
fn command_stream_keeps_chunk_order() {
    let s = "{\"data\":\"a\",\"stream\":\"stdout\"}\n{\"data\":\"b\",\"stream\":\"stdout\"}\n{\"data\":\"c\",\"stream\":\"stdout\"}";
    assert_eq!(parse_command_stream(s).stdout, "abc");
}

#[test]
fn command_stream_ignores_unparseable_lines() {
    // 上流が未知のイベントを増やしても落ちない
    let s = "not json\n{\"data\":\"x\",\"stream\":\"stdout\"}\n{\"somethingNew\":1}";
    assert_eq!(parse_command_stream(s).stdout, "x");
}

#[test]
fn server_errors_and_rate_limits_are_transient() {
    assert_eq!(classify_sandbox_failure(500, ""), UpstreamFailure::Transient);
    assert_eq!(classify_sandbox_failure(503, ""), UpstreamFailure::Transient);
    assert_eq!(classify_sandbox_failure(429, ""), UpstreamFailure::Transient);
}

#[test]
fn quota_exhaustion_is_its_own_failure() {
    // Hobby は枠を超えると課金ではなく「サンドボックス作成の停止」になる。
    // これを一般エラーに混ぜると、9/7 の Wandbox 障害と同じ「原因が分からない 502」に戻る
    assert_eq!(
        classify_sandbox_failure(402, "{\"error\":{\"code\":\"quota_exceeded\"}}"),
        UpstreamFailure::QuotaExhausted
    );
    assert_eq!(
        classify_sandbox_failure(403, "{\"error\":{\"code\":\"resource_limit_exceeded\",\"message\":\"You have exceeded your Sandbox quota\"}}"),
        UpstreamFailure::QuotaExhausted
    );
}

#[test]
fn unauthorized_is_a_configuration_problem_not_a_generic_failure() {
    // 401/403 はトークンが無効・権限不足 = サーバー設定の問題。一般エラーに混ぜると、
    // 2026-09-11 の「OIDC を env から読んでいた」級の設定ミスが
    // 「実行環境がエラーを返しました」に埋もれて切り分けに戻れない
    assert_eq!(classify_sandbox_failure(401, "Unauthorized"), UpstreamFailure::Unauthorized);
    assert_eq!(
        classify_sandbox_failure(403, "{\"error\":{\"code\":\"forbidden\"}}"),
        UpstreamFailure::Unauthorized
    );
    // 枠切れの 403 は枠切れのまま (そちらが先に判定される)
    assert_eq!(
        classify_sandbox_failure(403, "You have exceeded your Sandbox quota"),
        UpstreamFailure::QuotaExhausted
    );
}

#[test]
fn client_errors_are_not_retried() {
    // 400 台 (枠切れ以外) は再試行しても直らない
    assert_eq!(classify_sandbox_failure(400, "{\"error\":\"bad request\"}"), UpstreamFailure::Other);
    assert_eq!(classify_sandbox_failure(404, ""), UpstreamFailure::Other);
}

#[test]
fn image_not_ready_is_transient() {
    // VCR が linux/amd64 の最適化を終えるまで image_not_ready が返る (ドキュメント記載)
    assert_eq!(
        classify_sandbox_failure(400, "{\"error\":{\"code\":\"image_not_ready\"}}"),
        UpstreamFailure::Transient
    );
}

// ---- 認証トークンの取り方 (2026-09-11 の本番障害) ----

#[test]
fn the_request_header_is_the_production_source_of_the_token() {
    // Vercel は**関数には環境変数ではなくリクエストヘッダで** OIDC トークンを渡す
    // (docs/oidc の "In Vercel Functions")。env だけを見ていたため、初回の本番
    // デプロイが全言語 500「認証情報がありません」になった
    assert_eq!(OIDC_HEADER, "x-vercel-oidc-token");
    assert_eq!(pick_token(Some("from-header"), None, None).as_deref(), Some("from-header"));
}

#[test]
fn header_wins_over_environment() {
    // 本番ではヘッダのトークンが毎回更新される。env に古いものが残っていても従わない
    assert_eq!(
        pick_token(Some("fresh"), Some("stale"), Some("pat")).as_deref(),
        Some("fresh")
    );
}

#[test]
fn environment_is_the_local_development_fallback() {
    // ローカルは `vercel env pull` が VERCEL_OIDC_TOKEN を置く
    assert_eq!(pick_token(None, Some("local-oidc"), None).as_deref(), Some("local-oidc"));
    // OIDC が使えない環境向けの個人アクセストークン
    assert_eq!(pick_token(None, None, Some("pat")).as_deref(), Some("pat"));
}

#[test]
fn blank_values_are_not_tokens() {
    // 空文字のヘッダ / 環境変数を「ある」と扱うと、401 を認証情報の不備として表示できない
    assert_eq!(pick_token(Some("  "), Some(""), None), None);
    assert_eq!(pick_token(None, None, None), None);
}
