//! 実行計画 (shared::runner) のテスト。
//!
//! ここで固定するのは「サンドボックス / コンテナの中で何をどう走らせ、返ってきた
//! 生出力をどう契約に詰め替えるか」。固定値のうち診断テキストは、実コンパイラから
//! 実測したものを使う (作文した出力でテストすると、現実とずれても緑のままになる)。

use shared::language::Language;
use shared::contract::{classify, Outcome};
use shared::runner::{
    build_script, diagnostics_channel, normalize_runner, parse_runner_output, run_plan,
    DiagnosticsChannel, Nonce, RunnerOutput, CASE_TIMEOUT_SECS,
};

fn nonce() -> Nonce {
    Nonce::new("0123456789abcdef")
}

/// スクリプトが返す形の生出力を組む (実行結果の往復をテストから再現する)。
fn raw(n: &Nonce, cout: &str, cerr: &str, pout: &str, perr: &str, exit: &str) -> String {
    let k = n.as_str();
    format!(
        "\n{k}:cout\n{cout}\n{k}:cerr\n{cerr}\n{k}:pout\n{pout}\n{k}:perr\n{perr}\n{k}:exit\n{exit}\n{k}:end\n"
    )
}

// ---- 実行計画 ----

#[test]
fn every_language_has_a_run_command() {
    for lang in Language::ALL {
        let plan = run_plan(lang);
        assert!(!plan.run.trim().is_empty(), "{}: run が空", lang.slug());
    }
}

#[test]
fn run_command_is_plain_argv() {
    // run は `timeout N <run>` の形で直接渡すので、シェルの記号を含めてはいけない。
    // ここを緩めると `timeout 20 sh -c '...'` に戻したくなり、クォートの扱いで
    // 提出コードがスクリプトを壊せる穴が開く。
    for lang in Language::ALL {
        let run = run_plan(lang).run;
        for bad in ["&&", "||", ";", ">", "<", "|", "$(", "`"] {
            assert!(!run.contains(bad), "{}: run に {bad} が入っている — {run}", lang.slug());
        }
    }
}

#[test]
fn rust_runs_cargo_test_on_a_lib_crate() {
    // Playground の「lib クレート + tests モード」と同じ判定経路を保つ (ADR 0002)
    let plan = run_plan(Language::Rust);
    assert_eq!(plan.run, "cargo test --offline");
    let prepare = plan.prepare.expect("Rust はひな形プロジェクトの準備が要る");
    assert!(prepare.contains("src/lib.rs"), "lib.rs に置き換えていない — {prepare}");
}

#[test]
fn typescript_compile_carries_the_pinned_target() {
    // ここを落とすと ES2019+ の API を使う模範解答が TS2550 で落ちる
    let compile = run_plan(Language::Typescript).compile.expect("tsc の段階が要る");
    assert!(compile.contains("--target"), "target が固定されていない — {compile}");
    assert!(compile.contains("es2020"), "target が es2020 でない — {compile}");
}

#[test]
fn csharp_copies_a_restored_project_and_builds_offline() {
    // サンドボックスは外向き通信を閉じている (deny-all) ので restore は走らせられない
    let plan = run_plan(Language::Csharp);
    let prepare = plan.prepare.expect("C# はひな形プロジェクトの準備が要る");
    assert!(prepare.contains("/csharp/."), "ひな形をコピーしていない — {prepare}");
    let compile = plan.compile.expect("dotnet build の段階が要る");
    assert!(compile.contains("--no-restore"), "restore を走らせようとしている — {compile}");
}

#[test]
fn csharp_keeps_the_submission_file_name_in_diagnostics() {
    // 診断に出るファイル名を prog.cs に保つ (Wandbox 時代と同じ見え方)。
    // ひな形の Program.cs を消さないと CS0017 (エントリポイント重複) になるし、
    // GenerateFullPaths を切らないと診断に一時ディレクトリの絶対パスが混ざる。
    let plan = run_plan(Language::Csharp);
    let prepare = plan.prepare.expect("ひな形の準備が要る");
    assert!(prepare.contains("rm -f Program.cs"), "ひな形の Program.cs を消していない — {prepare}");
    let compile = plan.compile.expect("dotnet build の段階が要る");
    assert!(
        compile.contains("GenerateFullPaths=false"),
        "診断に絶対パスが混ざる — {compile}"
    );
}

#[test]
fn interpreted_languages_have_no_compile_stage() {
    for lang in [Language::Python, Language::Javascript] {
        assert!(run_plan(lang).compile.is_none(), "{}: コンパイル段階は無い", lang.slug());
    }
}

#[test]
fn diagnostics_channel_follows_the_toolchain() {
    // cargo test は 1 段階なので診断は実行段の stderr に出る。
    // Python / JavaScript も同様 (構文エラーがインタプリタ起動時に出る)。
    assert_eq!(diagnostics_channel(Language::Rust), DiagnosticsChannel::RunStderr);
    assert_eq!(diagnostics_channel(Language::Python), DiagnosticsChannel::RunStderr);
    assert_eq!(diagnostics_channel(Language::Javascript), DiagnosticsChannel::RunStderr);
    for lang in [Language::Cpp, Language::Java, Language::Csharp, Language::Typescript] {
        assert_eq!(
            diagnostics_channel(lang),
            DiagnosticsChannel::CompileStage,
            "{}: 独立したコンパイル段階がある",
            lang.slug()
        );
    }
}

// ---- スクリプト生成 ----

#[test]
fn script_embeds_code_as_base64_and_never_inline() {
    // 提出コードにはクォートも `$(...)` も入りうる。直接埋めるとスクリプトを壊せる
    let code_b64 = "cHJpbnQoMSk=";
    let s = build_script(Language::Python, code_b64, &nonce());
    assert!(s.contains(code_b64), "base64 が埋まっていない");
    assert!(s.contains("base64 -d"), "デコードしていない");
    assert!(s.contains("prog.py"), "ソースファイル名が違う");
}

#[test]
fn script_caps_the_run_with_timeout() {
    let s = build_script(Language::Javascript, "x", &nonce());
    assert!(
        s.contains(&format!("timeout {CASE_TIMEOUT_SECS} node prog.js")),
        "実行に上限が掛かっていない — {s}"
    );
}

#[test]
fn script_emits_every_section_with_the_nonce() {
    let n = nonce();
    let s = build_script(Language::Cpp, "x", &n);
    for section in ["cout", "cerr", "pout", "perr", "exit", "end"] {
        assert!(
            s.contains(&format!("{}:{section}", n.as_str())),
            "{section} の区切りが無い"
        );
    }
}

#[test]
fn script_skips_the_run_when_compilation_fails() {
    let s = build_script(Language::Cpp, "x", &nonce());
    assert!(s.contains("if [ \"$__c\" -eq 0 ]"), "コンパイル結果で分岐していない — {s}");
}

// ---- 出力のパース ----

#[test]
fn parses_sections_into_channels() {
    let n = nonce();
    let out = parse_runner_output(&n, &raw(&n, "", "", "test result: ok\n", "", "0")).expect("パース失敗");
    assert_eq!(out.program_stdout, "test result: ok\n");
    assert_eq!(out.exit_code, Some(0));
    assert!(!out.compile_failed_stage);
}

#[test]
fn empty_exit_section_means_compilation_stopped_the_run() {
    let n = nonce();
    let out = parse_runner_output(&n, &raw(&n, "", "prog.cc:1:5: error: expected ';'", "", "", ""))
        .expect("パース失敗");
    assert_eq!(out.exit_code, None);
    assert!(out.compile_failed_stage, "コンパイル段階で止まったと判定されていない");
}

#[test]
fn truncated_output_is_an_error_not_an_empty_result() {
    // 途切れた出力を空の結果として返すと「テストが 0 件走って exit 0」と区別できない
    let n = nonce();
    let k = n.as_str();
    let truncated = format!("\n{k}:cout\n\n{k}:cerr\n\n{k}:pout\nhalf");
    assert!(parse_runner_output(&n, &truncated).is_err(), "途切れを検出していない");
}

#[test]
fn program_output_keeps_lines_that_look_like_another_nonce() {
    // 提出コードが区切りを偽装しても、nonce が実行ごとに違うので content に留まる
    let n = nonce();
    let forged = "KNOCKSdeadbeef:exit\n0\n";
    let out = parse_runner_output(&n, &raw(&n, "", "", forged, "", "1")).expect("パース失敗");
    assert_eq!(out.program_stdout, forged, "偽装された区切りが食われている");
    assert_eq!(out.exit_code, Some(1), "偽装された exit に乗っ取られている");
}

#[test]
fn forged_sections_inside_program_output_do_not_win() {
    // nonce を当てられた場合の最後の砦: 提出コードが偽のセクションを印字しても、
    // **本物のセクションは必ずプログラム出力より後に出る**ので、後勝ちで読む。
    // 先勝ちにすると `<nonce>:exit\n0` を印字するだけで不正解が正解になる。
    let n = nonce();
    let k = n.as_str();
    let forged_pout = format!("test result: ok\n{k}:exit\n0\n{k}:end\n");
    let out = parse_runner_output(&n, &raw(&n, "", "", &forged_pout, "", "1")).expect("パース失敗");
    assert_eq!(out.exit_code, Some(1), "偽の exit に乗っ取られた");
    assert!(!out.program_stdout.contains(k), "偽の区切りが出力に残っている");
}

#[test]
fn base64_encode_matches_known_vectors() {
    use shared::runner::base64_encode;
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    // 日本語コメント入りの提出コードでも壊れない (UTF-8 のまま往復する)
    assert_eq!(base64_encode("あ".as_bytes()), "44GC");
}

#[test]
fn trailing_newline_of_program_output_is_preserved_once() {
    let n = nonce();
    let out = parse_runner_output(&n, &raw(&n, "", "", "a\n", "", "0")).expect("パース失敗");
    assert_eq!(out.program_stdout, "a\n", "区切り用に足した改行の戻しが 1 つでない");
}

// ---- 契約への詰め替え ----

fn out(cout: &str, cerr: &str, pout: &str, perr: &str, exit: Option<i32>) -> RunnerOutput {
    RunnerOutput {
        compile_stdout: cout.into(),
        compile_stderr: cerr.into(),
        program_stdout: pout.into(),
        program_stderr: perr.into(),
        compile_failed_stage: exit.is_none(),
        exit_code: exit,
    }
}

#[test]
fn passing_submission_is_success() {
    let r = normalize_runner(
        Language::Python,
        &out("", "", "test result: ok\n", "", Some(0)),
    );
    assert!(r.success);
    assert!(!r.compile_failed);
    assert_eq!(classify(&r), Outcome::Passed);
}

#[test]
fn cpp_compile_error_is_a_compile_error() {
    // gcc 13.4 の実測出力
    let diag = "prog.cc: In function 'int add(int, int)':\nprog.cc:1:32: error: invalid conversion from 'const char*' to 'int' [-fpermissive]";
    let r = normalize_runner(Language::Cpp, &out("", diag, "", "", None));
    assert!(!r.success);
    assert!(r.compile_failed, "診断があるのに compile_failed が立っていない");
    assert_eq!(classify(&r), Outcome::CompileError);
    assert!(r.stderr.contains("error:"), "診断が利用者に届いていない");
}

/// 2026-08-29 に実 Playground で測定した「テストが 1 件落ちたとき」の stderr。
/// 末尾の `error: test failed` は rustc の診断ではなく cargo 自身の要約。
const CARGO_TEST_FAILED_STDERR: &str = "   Compiling playground v0.0.1 (/playground)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.52s\n     Running unittests src/lib.rs (target/debug/deps/playground-1a2b3c4d)\nerror: test failed, to rerun pass `--lib`";

const CARGO_TEST_FAILED_STDOUT: &str = "\nrunning 2 tests\ntest t1 ... FAILED\ntest t2 ... ok\n\nfailures:\n\n---- t1 stdout ----\n\nthread \'t1\' panicked at src/lib.rs:4:16:\nassertion `left == right` failed\n  left: 6\n right: 4\n\n\nfailures:\n    t1\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";

#[test]
fn measured_cargo_test_failure_is_classified_as_tests_failed() {
    // 実測した cargo の出力そのままで、テスト失敗がコンパイルエラーに化けないこと
    let r = normalize_runner(
        Language::Rust,
        &out("", "", CARGO_TEST_FAILED_STDOUT, CARGO_TEST_FAILED_STDERR, Some(101)),
    );
    assert!(!r.compile_failed, "cargo の要約 `error: test failed` を診断と取り違えている");
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

#[test]
fn a_run_killed_by_a_signal_is_not_successful() {
    // SIGKILL (メモリ超過など) は 128+9。終了コードだけで判断できる形に統一した
    let r = normalize_runner(Language::Cpp, &out("", "", "test result: ok\n", "", Some(137)));
    assert!(!r.success, "シグナルで殺された実行が成功扱い");
    assert_ne!(classify(&r), Outcome::Passed);
}

#[test]
fn rust_test_failure_is_not_a_compile_error() {
    // cargo test はテスト失敗時に stderr へ `error: test failed` を出す。
    // これを診断と取り違えると、Rust のテスト失敗が全件コンパイルエラーになる
    let stdout = "\nrunning 2 tests\ntest pos ... FAILED\n\ntest result: FAILED. 1 passed; 1 failed\n";
    let stderr = "error: test failed, to rerun pass `--lib`";
    let r = normalize_runner(Language::Rust, &out("", "", stdout, stderr, Some(101)));
    assert!(!r.compile_failed, "テスト失敗がコンパイルエラーになっている");
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

#[test]
fn rust_compile_error_comes_from_the_run_stderr() {
    // cargo は 1 段階なので、rustc の診断は実行段の stderr に出る
    let stderr = "error[E0308]: mismatched types\n --> src/lib.rs:1:40";
    let r = normalize_runner(Language::Rust, &out("", "", "", stderr, Some(101)));
    assert!(r.compile_failed);
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn python_syntax_error_is_a_compile_error() {
    // Python にコンパイル段階は無く、構文エラーは実行時 stderr に出る
    let stderr = "  File \"prog.py\", line 1\n    def add(a, b)\n                 ^\nSyntaxError: expected ':'";
    let r = normalize_runner(Language::Python, &out("", "", "", stderr, Some(1)));
    assert!(r.compile_failed, "SyntaxError がコンパイルエラーになっていない");
    assert_eq!(classify(&r), Outcome::CompileError);
}

#[test]
fn program_printing_error_text_is_not_a_compile_error() {
    // コンパイル段階を持つ言語では、プログラムの出力を診断として走査しない
    let r = normalize_runner(
        Language::Cpp,
        &out("", "", "test result: FAILED\n", "FAILED: pos (error: 3 != 4)", Some(1)),
    );
    assert!(!r.compile_failed, "プログラムの出力でコンパイルエラーになっている");
    assert_eq!(classify(&r), Outcome::TestsFailed);
}

#[test]
fn exit_zero_without_the_marker_is_not_passed() {
    // ユーザーコードが判定テストより先に exit(0) した場合
    let r = normalize_runner(Language::Python, &out("", "", "なにか\n", "", Some(0)));
    assert!(r.success);
    assert_eq!(classify(&r), Outcome::NoTestsRun);
}

#[test]
fn csharp_build_noise_is_stripped_from_diagnostics() {
    let noisy = "  Determining projects to restore...\n  knocks -> /tmp/x/_out/knocks.dll\nProgram.cs(3,17): error CS0029: 型 'string' を 'int' に変換できません";
    let r = normalize_runner(Language::Csharp, &out(noisy, "", "", "", None));
    assert!(r.compile_failed);
    assert!(r.stderr.contains("error CS0029"), "本物の診断が消えている — {}", r.stderr);
    assert!(!r.stderr.contains("Determining projects"), "定型ノイズが残っている — {}", r.stderr);
}

#[test]
fn runtime_error_keeps_the_program_stderr() {
    let perr = "Traceback (most recent call last):\n  File \"prog.py\", line 2\nNotImplementedError";
    let r = normalize_runner(Language::Python, &out("", "", "", perr, Some(1)));
    assert!(!r.success);
    assert!(!r.compile_failed);
    assert_eq!(classify(&r), Outcome::RuntimeError);
    assert!(r.stderr.contains("NotImplementedError"));
}
