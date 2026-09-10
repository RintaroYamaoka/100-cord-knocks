//! 使い方:
//!   cargo run -p verifier                                  # 全言語・全難易度
//!   cargo run -p verifier -- --lang cpp                    # 1 言語
//!   cargo run -p verifier -- --lang cpp --level beginner   # 1 ファイル
//!   cargo run -p verifier -- --lang cpp --level beginner --file batch.json --scratch target/s1
//!   cargo run -p verifier -- --expect 2100                 # 検証件数まで含めて検算
//!
//! 終了コード 0 は「検証した全問が期待どおり」かつ「1 問以上検証した」ときだけ。
//! 読めないファイル・タイムアウトは失敗として数える (黙って 0 件成功にしない)。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;

use shared::language::Language;
use shared::problem::{problems_rel_path, Level, Problem};
use verifier::docker::CaseKind;
use verifier::{load_problems_str, run_batch_docker};

struct Job {
    path: PathBuf,
    language: Language,
    level: Level,
    /// 収録ファイル (data/problems/<lang>/<level>.json) か。
    /// バッチファイルの検証 (--file) では件数 100 を要求しない。
    is_release_file: bool,
}

#[derive(Default)]
struct Totals {
    verified: usize,
    failed: usize,
    container_runs: usize,
}

fn verify_job(job: &Job, scratch: &PathBuf, totals: &mut Totals) -> Vec<Problem> {
    let json = match std::fs::read_to_string(&job.path) {
        Ok(s) => s,
        Err(e) => {
            // ここを「スキップ」にすると、パスを 1 文字間違えただけで
            // 「0 問検証 / 問題なし」= 緑になり、未検証データが収録される
            eprintln!("✗ {} を読めません ({e})", job.path.display());
            totals.failed += 1;
            return Vec::new();
        }
    };
    let problems: Vec<Problem> = match load_problems_str(&json) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ {} のパースに失敗: {e}", job.path.display());
            totals.failed += 1;
            return Vec::new();
        }
    };
    println!(
        "== {} : {} {} ({} 問) ==",
        job.path.display(),
        job.language.label(),
        job.level.label_ja(),
        problems.len()
    );

    // 収録ファイル (バッチではなく <難易度>.json) は 100 問という契約。
    // --file 指定 (= バッチ検証) のときは件数を問わない
    let expected_count = job.is_release_file.then_some(verifier::PROBLEMS_PER_FILE);
    for issue in verifier::validate_static_with_expected(&problems, job.language, job.level, expected_count) {
        eprintln!("✗ [{}] {}", issue.id, issue.message);
        totals.failed += 1;
    }

    if problems.is_empty() {
        // 無言 return にすると、空ファイルが「問題なし」で通る
        eprintln!("✗ {} に問題が 1 件も入っていません", job.path.display());
        totals.failed += 1;
        return Vec::new();
    }

    // 7 言語とも本番と同じイメージで検証する (ADR 0003)。
    // 以前は Rust だけローカル cargo だったが、ホストと本番の rustc がずれる
    verify_via_docker(job, &problems, scratch, totals);
    problems
}

fn verify_via_docker(job: &Job, problems: &[Problem], scratch: &PathBuf, totals: &mut Totals) {
    let workdir = scratch.join(format!("{}-{}", job.language.slug(), job.level.slug()));
    let workdir = std::path::absolute(&workdir).unwrap_or(workdir);

    let report = match run_batch_docker(job.language, problems, &workdir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("✗ {} のバッチ実行に失敗: {e}", job.path.display());
            totals.failed += problems.len();
            totals.verified += problems.len();
            return;
        }
    };
    totals.container_runs += report.container_runs;
    // コンテナ自体が壊れたのを「⚠ 表示だけ」にすると、環境障害が問題の欠陥に化ける
    // (starter 側は「正しく落ちた」として数えられてしまう)。失敗として計上する。
    for e in &report.errors {
        eprintln!("✗ {e}");
        totals.failed += 1;
    }

    let answers: HashSet<&str> = report
        .outcomes
        .iter()
        .filter(|o| o.kind == CaseKind::Answer && o.passed)
        .map(|o| o.problem_id.as_str())
        .collect();
    let starters_passing: HashSet<&str> = report
        .outcomes
        .iter()
        .filter(|o| o.kind == CaseKind::Starter && o.passed)
        .map(|o| o.problem_id.as_str())
        .collect();

    for p in problems {
        totals.verified += 1;
        if !answers.contains(p.id.as_str()) {
            let detail = report
                .outcomes
                .iter()
                .find(|o| o.problem_id == p.id && o.kind == CaseKind::Answer)
                .map(|o| tail(&o.output))
                .unwrap_or_else(|| "(実行結果なし)".into());
            eprintln!("✗ [{}] answer_code がテストを通りません:\n{detail}", p.id);
            totals.failed += 1;
            continue;
        }
        if starters_passing.contains(p.id.as_str()) {
            eprintln!("✗ [{}] starter_code のままテストが通ってしまいます", p.id);
            totals.failed += 1;
            continue;
        }
        println!("✓ {}", p.id);
    }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(25);
    lines[start..].join("\n")
}

fn usage() -> ExitCode {
    eprintln!("usage: verifier [--lang <slug>] [--level <slug>] [--file <path>] [--scratch <dir>] [--expect <N>]");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut language: Option<Language> = None;
    let mut level: Option<Level> = None;
    let mut file: Option<PathBuf> = None;
    let mut expect: Option<usize> = None;
    let mut scratch = PathBuf::from("target/verifier-scratch");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lang" => match args.next().as_deref().and_then(Language::from_slug) {
                Some(l) => language = Some(l),
                None => {
                    eprintln!("--lang には rust/cpp/csharp/java/python/typescript/javascript を指定します");
                    return ExitCode::FAILURE;
                }
            },
            "--level" => match args.next().as_deref().and_then(Level::from_slug) {
                Some(l) => level = Some(l),
                None => {
                    eprintln!("--level には beginner/intermediate/advanced を指定します");
                    return ExitCode::FAILURE;
                }
            },
            "--file" => match args.next() {
                Some(p) => file = Some(PathBuf::from(p)),
                None => return usage(),
            },
            "--scratch" => match args.next() {
                Some(p) => scratch = PathBuf::from(p),
                None => return usage(),
            },
            "--expect" => match args.next().and_then(|s| s.parse().ok()) {
                Some(n) => expect = Some(n),
                None => return usage(),
            },
            other => {
                eprintln!("不明な引数: {other}");
                return usage();
            }
        }
    }

    let single_file = file.is_some();
    let jobs: Vec<Job> = if let Some(path) = file {
        match (language, level) {
            (Some(language), Some(level)) => vec![Job { path, language, level, is_release_file: false }],
            _ => {
                eprintln!("--file を使うときは --lang と --level も指定してください");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let langs: Vec<Language> = language.map_or_else(|| Language::ALL.to_vec(), |l| vec![l]);
        let levels: Vec<Level> = level.map_or_else(|| Level::ALL.to_vec(), |l| vec![l]);
        langs
            .iter()
            .flat_map(|&language| {
                levels.iter().map(move |&level| Job {
                    path: PathBuf::from(problems_rel_path(language, level)),
                    language,
                    level,
                    is_release_file: true,
                })
            })
            .collect()
    };

    // 実行環境の前提を着手時に 1 回だけ検査する。
    // 揃わないまま進むと「検証したつもりの未検証データ」が積み上がる
    if let Err(e) = verifier::docker::preflight() {
        eprintln!("✗ 実行環境の前提が満たされていません:
  {e}");
        return ExitCode::FAILURE;
    }

    let mut totals = Totals::default();
    // 言語ごとに全難易度を集めて、難易度をまたぐ重複を検査する。
    // ファイル単位の検査だけだと「初級と中級に同じ問題」が素通りする
    let mut per_language: std::collections::HashMap<Language, Vec<Problem>> =
        std::collections::HashMap::new();
    for job in &jobs {
        let problems = verify_job(job, &scratch, &mut totals);
        per_language.entry(job.language).or_default().extend(problems);
    }

    // 3 難易度すべてを見たときだけ意味がある検査なので、揃っている言語に限る
    let full_language_run = level.is_none() && !single_file;
    if full_language_run {
        for (language, problems) in &per_language {
            for issue in verifier::validate_across_levels(problems) {
                eprintln!("✗ [{}/{}] {}", language.slug(), issue.id, issue.message);
                totals.failed += 1;
            }
        }
    }

    println!(
        "---\n検証 {} 問 / 問題あり {} 件 / コンテナ起動 {} 回",
        totals.verified, totals.failed, totals.container_runs
    );

    if totals.verified == 0 {
        eprintln!("✗ 1 問も検証していません (パスの指定を確認してください)");
        return ExitCode::FAILURE;
    }
    if let Some(n) = expect {
        if totals.verified != n {
            eprintln!("✗ 検証件数が期待と一致しません (期待 {n} / 実際 {})", totals.verified);
            return ExitCode::FAILURE;
        }
    }
    if totals.failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
