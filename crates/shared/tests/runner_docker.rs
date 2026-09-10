//! ローカル Docker で実行イメージを検証するスモークテスト。
//!
//! 本番 (Vercel Sandbox) と**同じイメージ・同じスクリプト**を走らせるので、
//! 「本番だけ落ちる」差を出さないための最初の関所になる。既定では走らせない。
//!
//!   cargo test -p shared --test runner_docker -- --ignored --nocapture --test-threads=1
//!
//! 前提: `bash scripts/build-runner-image.sh` でイメージを作ってあること。

use std::io::Write;
use std::process::{Command, Stdio};

use shared::fixtures;
use shared::language::Language;
use shared::contract::{classify, Outcome};
use shared::problem::compose_submission;
use shared::runner::{base64_encode, build_script, normalize_runner, parse_runner_output, Nonce};

use shared::runner::LOCAL_IMAGE as IMAGE;

/// イメージの中で 1 ケース走らせて、本番と同じ詰め替えを通した結果を返す。
fn run(language: Language, user_code: &str) -> shared::contract::ExecuteResponse {
    let code = compose_submission(language, user_code, fixtures::for_language(language).hidden_tests);
    let nonce = Nonce::new("d0ckersm0ke000001");
    let script = build_script(language, &base64_encode(code.as_bytes()), &nonce);

    let mut child = Command::new("docker")
        .args(["run", "--rm", "-i", "--network=none", IMAGE, "sh", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("docker run が起動できない (イメージを作ってあるか確認)");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let raw = String::from_utf8_lossy(&out.stdout);
    let parsed = parse_runner_output(&nonce, &raw)
        .unwrap_or_else(|e| panic!("{}: {e}\n--- stdout ---\n{raw}\n--- stderr ---\n{}", language.slug(), String::from_utf8_lossy(&out.stderr)));
    normalize_runner(language, &parsed)
}

#[test]
#[ignore = "ローカル Docker と knocks-runtime イメージが要る"]
fn every_language_judges_a_correct_answer_as_passed() {
    for lang in Language::ALL {
        let r = run(lang, fixtures::for_language(lang).answer);
        assert_eq!(
            classify(&r),
            Outcome::Passed,
            "{}: stdout={:?} stderr={:?}",
            lang.slug(),
            r.stdout,
            r.stderr
        );
        println!("✓ {:<11} 正解 → Passed", lang.slug());
    }
}

#[test]
#[ignore = "ローカル Docker と knocks-runtime イメージが要る"]
fn every_language_rejects_the_unimplemented_starter() {
    for lang in Language::ALL {
        let r = run(lang, fixtures::for_language(lang).starter);
        let o = classify(&r);
        assert_ne!(o, Outcome::Passed, "{}: 未実装が正解になった", lang.slug());
        println!("✓ {:<11} 未実装 → {o:?}", lang.slug());
    }
}

#[test]
#[ignore = "ローカル Docker と knocks-runtime イメージが要る"]
fn every_language_reports_a_real_compiler_diagnostic() {
    for lang in Language::ALL {
        let r = run(lang, fixtures::for_language(lang).broken);
        let sig = fixtures::compiler_error_signature(lang);
        assert_eq!(classify(&r), Outcome::CompileError, "{}: stderr={:?}", lang.slug(), r.stderr);
        assert!(r.stderr.contains(sig), "{}: 固有の診断が無い — {:?}", lang.slug(), r.stderr);
        let first = r.stderr.lines().find(|l| l.contains(sig)).unwrap_or("");
        println!("✓ {:<11} 壊れたコード → CompileError: {}", lang.slug(), first.trim());
    }
}

#[test]
#[ignore = "ローカル Docker と knocks-runtime イメージが要る"]
fn a_submission_that_exits_before_the_tests_is_not_passed() {
    // 判定テストより先に exit(0) する提出。終了コードだけで正解にすると通ってしまう
    let r = run(Language::Python, "import sys\ndef add(a, b):\n    return 0\nsys.exit(0)");
    assert_eq!(classify(&r), Outcome::NoTestsRun, "stdout={:?}", r.stdout);
    println!("✓ 先に exit(0) する提出 → NoTestsRun");
}

#[test]
#[ignore = "ローカル Docker と knocks-runtime イメージが要る"]
fn an_infinite_loop_is_killed_by_the_timeout() {
    let r = run(Language::Javascript, "function add(a, b) { while (true) {} }");
    assert_ne!(classify(&r), Outcome::Passed, "無限ループが正解になった");
    assert!(!r.success, "無限ループが成功扱い");
    println!("✓ 無限ループ → {:?} (timeout で殺された)", classify(&r));
}
