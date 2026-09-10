//! ローカル検証用サーバー — `dist/` を配信し、`/api/execute` を **`api/execute.rs` の
//! `dispatch` そのもの** で処理する。
//!
//! これがある理由: 以前はブラウザ検証用にプロキシを JS で書き直していたが、
//! 本物 (`api/execute.rs`) に TypeScript のフラグ修正を入れたときハーネス側が取り残され、
//! 「問題は正しいのにブラウザ検証だけ落ちる」という偽の失敗が出た (typescript/i065)。
//! 判定に関わるコードを二重に持たないことで、この種のズレを構造的に消す。
//!
//! 本番 (Vercel) では使わない。開発・検証用:
//!   cargo run --release --bin local-server -- dist 8081

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use shared::contract::ExecuteRequest;
use tokio::net::TcpListener;

// 本番のプロキシをそのまま取り込む。main/handler は Vercel 用なのでここでは使わない
#[path = "../api/execute.rs"]
#[allow(dead_code)]
mod execute;
#[path = "local_server_paths.rs"]
mod paths;

type Body = http_body_util::Full<Bytes>;

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn json(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(http_body_util::Full::new(Bytes::from(body)))
        .unwrap()
}

fn text(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(http_body_util::Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

async fn handle(
    req: Request<hyper::body::Incoming>,
    dist: PathBuf,
) -> Result<Response<Body>, std::convert::Infallible> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    if path == "/api/execute" {
        if method != Method::POST {
            return Ok(json(
                StatusCode::METHOD_NOT_ALLOWED,
                r#"{"error":"POST のみ受け付けます"}"#.into(),
            ));
        }
        // 本番ハンドラと同じ順序: body を消費する前にトークンのヘッダを取る
        let header_token = req
            .headers()
            .get(shared::sandbox::OIDC_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = match req.into_body().collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => {
                return Ok(json(StatusCode::BAD_REQUEST, r#"{"error":"ボディを読めません"}"#.into()))
            }
        };
        let exec_req: ExecuteRequest = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(_) => {
                return Ok(json(
                    StatusCode::BAD_REQUEST,
                    r#"{"error":"リクエストボディが不正です"}"#.into(),
                ))
            }
        };
        // ここが要点: 本番と同じ dispatch を呼ぶ (プロキシを書き直さない)
        return Ok(match execute::dispatch(exec_req, header_token.as_deref()).await {
            Ok(resp) => {
                let status = resp.status();
                json(status, resp.into_body())
            }
            Err(e) => json(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        });
    }

    let Some(file) = paths::resolve(&dist, &path) else {
        return Ok(text(StatusCode::FORBIDDEN, "forbidden"));
    };
    match tokio::fs::read(&file).await {
        Ok(data) => {
            let len = data.len();
            // HEAD にも 200 を返す (フロントが言語データの存在確認に使う)
            let body = if method == Method::HEAD { Bytes::new() } else { Bytes::from(data) };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime_for(&file))
                .header("content-length", len.to_string())
                .body(http_body_util::Full::new(body))
                .unwrap())
        }
        Err(_) => Ok(text(StatusCode::NOT_FOUND, "not found")),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dist = PathBuf::from(args.next().unwrap_or_else(|| "dist".into()));
    let port: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8081);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    // テストがこの行を待つので、変えるならテスト側も直すこと
    println!("listening on http://{addr} (dist={})", dist.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let dist = dist.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req| handle(req, dist.clone()));
            let _ = http1::Builder::new().serve_connection(io, svc).await;
        });
    }
}
