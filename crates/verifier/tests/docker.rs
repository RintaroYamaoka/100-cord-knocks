use std::path::Path;

use shared::language::Language;
use shared::runner::LOCAL_IMAGE;
use verifier::docker::{container_script, plan_batch, CaseKind, RunCase};

fn cases(n: usize) -> Vec<RunCase> {
    (1..=n)
        .flat_map(|i| {
            [CaseKind::Answer, CaseKind::Starter].into_iter().map(move |k| RunCase {
                problem_id: format!("b{i:03}"),
                kind: k,
                code: "// code".into(),
            })
        })
        .collect()
}

#[test]
fn a_whole_batch_needs_exactly_one_container() {
    // 1 問ごとにコンテナを起こすと 2100 問 × 2 で起動オーバーヘッドだけで数時間かかる。
    // バッチ 1 ファイル = コンテナ 1 回であることをここで固定する。
    for lang in Language::ALL {
        let plan = plan_batch(lang, &cases(20), Path::new("/tmp/w"));
        assert_eq!(plan.len(), 1, "{} が {} 回コンテナを起こしている", lang.slug(), plan.len());
    }
}

#[test]
fn container_count_does_not_grow_with_problem_count() {
    let small = plan_batch(Language::Cpp, &cases(1), Path::new("/tmp/w")).len();
    let large = plan_batch(Language::Cpp, &cases(100), Path::new("/tmp/w")).len();
    assert_eq!(small, large);
}

#[test]
fn rust_is_verified_in_the_same_image_as_everything_else() {
    // 以前は Rust だけローカル cargo で検証していた (速いので)。本番が 7 言語とも
    // 同じイメージになったので、検証も同じイメージに寄せる。ホストの rustc と
    // イメージの rustc が違うと、Rust だけ「検証と本番の版がずれる」状態に戻る
    let plan = plan_batch(Language::Rust, &cases(20), Path::new("/tmp/w"));
    assert_eq!(plan.len(), 1, "Rust が Docker で検証されていない");
    assert_eq!(plan[0].image, LOCAL_IMAGE);
}

#[test]
fn empty_batch_plans_no_container() {
    assert!(plan_batch(Language::Cpp, &[], Path::new("/tmp/w")).is_empty());
}

#[test]
fn every_language_uses_the_one_unified_image() {
    // 「検証と本番が同じイメージ」が ADR 0003 の核心。言語ごとに別イメージへ
    // 戻すと、TS の target ずれのような「verifier は緑なのに本番は落ちる」差が復活する
    for lang in Language::ALL {
        let plan = plan_batch(lang, &cases(3), Path::new("/tmp/w"));
        assert_eq!(plan[0].image, LOCAL_IMAGE, "{} が別イメージを使っている", lang.slug());
    }
}

#[test]
fn plan_is_network_isolated() {
    // 提出コードは信頼できない。ネットワークを与えない
    // (本番の Sandbox 側も networkPolicy: deny-all で同じ条件にしてある)
    let plan = plan_batch(Language::Python, &cases(3), Path::new("/tmp/w"));
    assert!(plan[0].network_disabled, "コンテナにネットワークが残っている");
}

#[test]
fn every_case_gets_a_per_case_timeout() {
    // 無限ループを書いた 1 問がバッチ全体を永久にブロックしないこと
    for lang in Language::ALL {
        let script = container_script(lang, &cases(2));
        assert!(script.contains("timeout "), "{} のスクリプトに timeout が無い", lang.slug());
    }
}

#[test]
fn script_visits_every_case_directory() {
    let script = container_script(Language::Cpp, &cases(2));
    for id in ["b001", "b002"] {
        for suffix in ["answer", "starter"] {
            assert!(script.contains(&format!("{id}-{suffix}")), "{id}-{suffix} が抜けている:\n{script}");
        }
    }
}

#[test]
fn script_records_exit_code_and_streams_separately() {
    let script = container_script(Language::Python, &cases(1));
    // stdout と stderr を混ぜると「目印が stdout にあるか」を判定できなくなる
    assert!(script.contains("_stdout"));
    assert!(script.contains("_stderr"));
    assert!(script.contains("_exit"));
}

#[test]
fn script_commands_come_from_the_shared_run_plan() {
    // コマンドの定義を verifier 側に持つと、本番 (shared::runner) との二重管理になる。
    // 2026-08-29 の TS target ずれはまさにこれで起きた
    for lang in Language::ALL {
        let plan = shared::runner::run_plan(lang);
        let script = container_script(lang, &cases(1));
        assert!(
            script.contains(&plan.run),
            "{}: run ({}) がスクリプトに入っていない",
            lang.slug(),
            plan.run
        );
        if let Some(compile) = plan.compile {
            assert!(script.contains(&compile), "{}: compile が違う", lang.slug());
        }
    }
}

#[test]
fn csharp_and_rust_copy_the_baked_template_per_case() {
    // ひな形 (restore 済み / cargo プロジェクト) はイメージに焼いてある。
    // ケースごとにコピーするので、ケース間で状態が混ざらない
    for lang in [Language::Csharp, Language::Rust] {
        let script = container_script(lang, &cases(2));
        assert!(
            script.contains(shared::runner::TEMPLATE_ROOT),
            "{}: ひな形を使っていない",
            lang.slug()
        );
    }
    // dotnet new はイメージのビルド時に済んでいる。実行時に走らせてはいけない
    assert!(!container_script(Language::Csharp, &cases(20)).contains("dotnet new"));
}

#[test]
fn scripts_use_the_language_source_file_name() {
    // C# だけ例外: csproj がディレクトリの **/*.cs を拾うので、コマンドは
    // ファイル名を書かない (ファイルは verifier 側が source_file_name() で置く)。
    // 逆に名前を書くと、ひな形の Program.cs と二重管理になる
    for lang in Language::ALL.into_iter().filter(|l| *l != Language::Csharp) {
        let script = container_script(lang, &cases(1));
        assert!(
            script.contains(lang.source_file_name()),
            "{} のスクリプトが {} を使っていない",
            lang.slug(),
            lang.source_file_name()
        );
    }
}
