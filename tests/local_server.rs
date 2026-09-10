//! ローカル検証サーバーの結合テスト。実際にバイナリを起動して HTTP で叩く。
//!
//! ここで守りたいのは「検証ハーネスが本番のプロキシと同じ判定をすること」。
//! 以前ハーネスを JS で書き直していたとき、本番に入れた TypeScript のフラグ修正が
//! ハーネス側に入らず、問題は正しいのにブラウザ検証だけ落ちた (typescript/i065)。
//! サーバーが `api/execute.rs` の `dispatch` を直接呼ぶようになったことを、
//! ここで実際の HTTP 経由で確かめる。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// テスト用にサーバーを起動し、Drop で確実に止める。
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(port: u16) -> Option<Self> {
        let bin = env!("CARGO_BIN_EXE_local-server");
        let mut child = Command::new(bin)
            .arg("dist")
            .arg(port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // "listening on ..." が出るまで待つ
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if !line.contains("listening") {
            let _ = child.kill();
            return None;
        }
        Some(Server { child, port })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap()
}

/// `dist/` が無いとき (ビルド前) はスキップする。CI での偽の失敗を避ける。
fn dist_ready() -> bool {
    std::path::Path::new("dist/index.html").exists()
}

#[test]
fn serves_static_files_and_answers_head() {
    if !dist_ready() {
        eprintln!("dist/ が無いのでスキップ (先に trunk build を実行)");
        return;
    }
    let Some(server) = Server::start(18801) else {
        eprintln!("サーバーを起動できないのでスキップ");
        return;
    };
    let c = client();

    let r = c.get(server.url("/")).send().unwrap();
    assert_eq!(r.status(), 200);

    // フロントは言語データの存在確認に HEAD を使う。ここが 200 を返さないと
    // 言語セレクタが縮退する
    let r = c
        .head(server.url("/data/problems/rust/beginner.json"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "HEAD が 200 を返していない");

    let r = c
        .head(server.url("/data/problems/nonexistent/x.json"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 404, "存在しないデータが 404 でない");
}

#[test]
fn rejects_path_traversal_over_http() {
    if !dist_ready() {
        return;
    }
    let Some(server) = Server::start(18802) else {
        return;
    };
    let r = client().get(server.url("/../Cargo.toml")).send().unwrap();
    assert_ne!(r.status(), 200, "dist の外を読めてしまっている");
}

#[test]
fn rejects_malformed_execute_request() {
    if !dist_ready() {
        return;
    }
    let Some(server) = Server::start(18803) else {
        return;
    };
    let c = client();

    let r = c.post(server.url("/api/execute")).body("{ broken").send().unwrap();
    assert_eq!(r.status(), 400);

    let r = c
        .post(server.url("/api/execute"))
        .json(&serde_json::json!({"language": "rust", "code": "  "}))
        .send()
        .unwrap();
    assert_eq!(r.status(), 400, "空コードを弾いていない");

    let r = c.get(server.url("/api/execute")).send().unwrap();
    assert_eq!(r.status(), 405);
}

#[test]
#[ignore = "実上流 (Vercel Sandbox) に接続する"]
fn typescript_flags_reach_the_upstream_through_the_server() {
    // 回帰テスト: ハーネスが tsc の target を渡していなかったせいで、
    // ES2019+ の API を使う正解が「ブラウザ検証だけ落ちる」ことがあった。
    // Object.fromEntries は --target es2020 が無いと TS2550 になる。
    if !dist_ready() {
        eprintln!("dist/ が無いのでスキップ");
        return;
    }
    let Some(server) = Server::start(18804) else {
        eprintln!("サーバーを起動できないのでスキップ");
        return;
    };

    let code = "function pairsToObj(ps: [string, number][]): Record<string, number> {\n  return Object.fromEntries(ps);\n}\n\n// ===== 判定用テスト (自動付加) =====\ndeclare const process: { exit(code: number): never };\nlet __f = 0;\nfunction __chk(c: boolean, n: string): void { if (!c) { console.error(\"FAILED: \" + n); __f++; } }\n__chk(pairsToObj([[\"a\", 1]]).a === 1, \"one\");\n__chk(Object.keys(pairsToObj([])).length === 0, \"empty\");\nif (__f > 0) { console.log(\"test result: FAILED\"); process.exit(1); }\nconsole.log(\"test result: ok\");\n";

    let r = client()
        .post(server.url("/api/execute"))
        .json(&serde_json::json!({"language": "typescript", "code": code}))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "HTTP {}", r.status());
    let body: shared::contract::ExecuteResponse = r.json().unwrap();
    assert!(
        !body.compile_failed,
        "ES2020 のフラグが上流に届いていない: {}",
        body.stderr
    );
    assert_eq!(
        shared::contract::classify(&body),
        shared::contract::Outcome::Passed,
        "stdout={:?} stderr={:?}",
        body.stdout,
        body.stderr
    );
}
