//! デプロイ済みの本番に対する実測 — 7 言語の疎通と、提出 1 回の所要時間。
//!
//! 既定では走らせない (ネットワークと本番のデプロイに依存するため)。
//!
//!   cargo test -p rust-100-knocks-api --test production -- --ignored --nocapture
//!
//! 使う見本は `shared::fixtures` で、ローカルスモークとも実 Sandbox テストとも同じもの。
//! 宛先は `KNOCKS_PROD_URL` で差し替えられる (プレビューのデプロイを測るとき)。
//!
//! **これがある理由**: レイテンシは「変えた側」でしか測れない。デプロイの前後で
//! 同じ手順を回して比べられないと、改善したつもりが体感で確かめられない
//! (ADR 0003 の残課題「レイテンシ改善」)。

use std::time::{Duration, Instant};

use shared::contract::{classify, ExecuteRequest, ExecuteResponse, Outcome};
use shared::fixtures;
use shared::language::Language;
use shared::problem::compose_submission;

const DEFAULT_URL: &str = "https://100-cord-knocks.vercel.app/api/execute";

/// 1 提出にこれ以上かかったら、改善どころか壊れている。
const SANITY_LIMIT: Duration = Duration::from_secs(30);

fn endpoint() -> String {
    std::env::var("KNOCKS_PROD_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

fn submit(client: &reqwest::blocking::Client, lang: Language, user_code: &str) -> (u16, ExecuteResponse, Duration) {
    let code = compose_submission(lang, user_code, fixtures::for_language(lang).hidden_tests);
    let body = serde_json::to_string(&ExecuteRequest::judge(lang, &code)).unwrap();

    let started = Instant::now();
    let resp = client
        .post(endpoint())
        .header("content-type", "application/json")
        .body(body)
        .send()
        .expect("本番に到達できない");
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    let elapsed = started.elapsed();

    let parsed = serde_json::from_str::<ExecuteResponse>(&text).unwrap_or(ExecuteResponse {
        success: false,
        stdout: String::new(),
        stderr: text,
        compile_failed: false,
    });
    (status, parsed, elapsed)
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .unwrap()
}

#[test]
#[ignore = "デプロイ済みの本番に接続する"]
fn every_language_passes_in_production_and_how_long_it_takes() {
    let client = client();
    println!("宛先: {}", endpoint());
    println!("{:<12} {:>8}  判定", "言語", "秒");

    let mut timings: Vec<(Language, Duration)> = Vec::new();
    for lang in Language::ALL {
        let (status, r, elapsed) = submit(&client, lang, fixtures::for_language(lang).answer);
        assert_eq!(status, 200, "{}: HTTP {status} — {}", lang.slug(), r.stderr);
        assert_eq!(classify(&r), Outcome::Passed, "{}: stdout={:?} stderr={:?}", lang.slug(), r.stdout, r.stderr);
        assert!(elapsed < SANITY_LIMIT, "{}: {elapsed:?} かかった", lang.slug());
        println!("{:<12} {:>8.2}  Passed", lang.slug(), elapsed.as_secs_f64());
        timings.push((lang, elapsed));
    }

    let secs: Vec<f64> = timings.iter().map(|(_, d)| d.as_secs_f64()).collect();
    let min = secs.iter().cloned().fold(f64::MAX, f64::min);
    let max = secs.iter().cloned().fold(0.0, f64::max);
    let avg = secs.iter().sum::<f64>() / secs.len() as f64;
    println!("\n7 言語: 最短 {min:.2}s / 平均 {avg:.2}s / 最長 {max:.2}s");
}

#[test]
#[ignore = "デプロイ済みの本番に接続する"]
fn a_broken_submission_still_reports_a_real_compiler_diagnostic() {
    // 速くする変更で診断が落ちていないこと (停止を待たなくしても結果の詰め替えは変わらない)
    let client = client();
    for lang in Language::ALL {
        let (status, r, _) = submit(&client, lang, fixtures::for_language(lang).broken);
        assert_eq!(status, 200, "{}: HTTP {status} — {}", lang.slug(), r.stderr);
        let sig = fixtures::compiler_error_signature(lang);
        assert_eq!(classify(&r), Outcome::CompileError, "{}: stderr={:?}", lang.slug(), r.stderr);
        assert!(r.stderr.contains(sig), "{}: 固有の診断が無い — {:?}", lang.slug(), r.stderr);
        println!("✓ {:<11} 壊れたコード → CompileError ({sig})", lang.slug());
    }
}
