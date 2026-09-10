//! 問題コンテンツの品質検証ハーネス。
//!
//! 上流に負荷をかけず、ローカルで
//! `answer_code` / `starter_code` を実際にコンパイル・実行して「収録して良い問題か」を
//! 機械判定する。Rust はローカル cargo、他 6 言語は版を固定した Docker イメージ
//! (ADR 0002 の表が正本)。

pub mod docker;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use shared::language::Language;
use shared::contract::{TEST_FAILED_MARKER, TEST_OK_MARKER};
use shared::problem::{compose_submission, Level, Problem};

use crate::docker::{plan_batch, CaseKind, RunCase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemIssue {
    pub id: String,
    pub message: String,
}

impl ProblemIssue {
    pub fn new(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            message: message.into(),
        }
    }
}

pub fn load_problems_str(json: &str) -> Result<Vec<Problem>, serde_json::Error> {
    serde_json::from_str(json)
}

/// `description_md` の最小文字数。これを下回るものは説明として成立していない。
pub const MIN_DESCRIPTION_CHARS: usize = 80;

/// 実行なしで判定できる整合性チェック。
///
/// `expected_language` / `expected_level` はファイルのパスから来る (パスが正本で、
/// `Problem` のフィールドはその冗長コピー)。
pub fn validate_static(
    problems: &[Problem],
    expected_language: Language,
    expected_level: Level,
) -> Vec<ProblemIssue> {
    validate_static_with_expected(problems, expected_language, expected_level, None)
}

/// 収録ファイル 1 枚は 100 問という契約 (docs/problem-authoring.md §1)。
pub const PROBLEMS_PER_FILE: usize = 100;

/// `expected_count` を渡すと件数も検査する。
/// 件数を見ないと、空ファイルや 50 問しかないファイルが「問題なし」で通ってしまう。
pub fn validate_static_with_expected(
    problems: &[Problem],
    expected_language: Language,
    expected_level: Level,
    expected_count: Option<usize>,
) -> Vec<ProblemIssue> {
    let mut issues = Vec::new();

    if let Some(n) = expected_count {
        if problems.len() != n {
            issues.push(ProblemIssue::new(
                "-",
                format!("問題数が {} 件です (期待 {n} 件)", problems.len()),
            ));
        }
    }
    let mut seen_ids = std::collections::HashSet::new();
    let mut titles: HashMap<&str, &str> = HashMap::new();
    let mut answers: HashMap<&str, &str> = HashMap::new();

    for p in problems {
        if !seen_ids.insert(p.id.as_str()) {
            issues.push(ProblemIssue::new(&p.id, "id が重複しています"));
        }
        let prefix_ok = p.id.starts_with(expected_level.id_prefix())
            && p.id.len() == 4
            && p.id[1..].chars().all(|c| c.is_ascii_digit());
        if !prefix_ok {
            issues.push(ProblemIssue::new(
                &p.id,
                format!("id の形式が不正です (期待: {}NNN)", expected_level.id_prefix()),
            ));
        }
        if p.language != expected_language {
            issues.push(ProblemIssue::new(
                &p.id,
                format!(
                    "language ({}) がファイルの言語 ({}) と一致しません",
                    p.language.slug(),
                    expected_language.slug()
                ),
            ));
        }
        if p.level != expected_level {
            issues.push(ProblemIssue::new(&p.id, "level がファイルの難易度と一致しません"));
        }

        for (name, value) in [
            ("title", &p.title),
            ("description_md", &p.description_md),
            ("starter_code", &p.starter_code),
            ("hidden_tests", &p.hidden_tests),
            ("answer_code", &p.answer_code),
            ("explanation_md", &p.explanation_md),
        ] {
            if value.trim().is_empty() {
                issues.push(ProblemIssue::new(&p.id, format!("{name} が空です")));
            }
        }

        if p.starter_code == p.answer_code {
            issues.push(ProblemIssue::new(&p.id, "starter_code と answer_code が同一です"));
        }

        if p.description_md.chars().count() < MIN_DESCRIPTION_CHARS {
            issues.push(ProblemIssue::new(
                &p.id,
                format!("description_md が短すぎます ({} 文字 / 最低 {MIN_DESCRIPTION_CHARS})", p.description_md.chars().count()),
            ));
        }

        issues.extend(validate_hidden_tests(p));

        // 使い回しの検出。1 問をコピーした 100 問は他の検査を全部通ってしまう
        if let Some(other) = titles.insert(p.title.as_str(), p.id.as_str()) {
            issues.push(ProblemIssue::new(
                &p.id,
                format!("title が {other} と重複しています: 「{}」", p.title),
            ));
        }
        if let Some(other) = answers.insert(p.answer_code.as_str(), p.id.as_str()) {
            issues.push(ProblemIssue::new(
                &p.id,
                format!("answer_code が {other} と完全に同一です"),
            ));
        }
    }
    issues
}

/// 難易度をまたいだ重複を検査する。
///
/// `validate_static` はファイル単位 (= 難易度ごと) に呼ばれるので、初級と中級に
/// 同じ問題を置いても素通りしてしまう。実際、既存 Rust 300 問には
/// 「文字列を反転する」(b077 / i058) と「単語の出現回数を数える」(b083 / i062) が
/// **題名も模範解答も同一**のまま両方に収録されていた。利用者は同じ問題を 2 回解かされる。
pub fn validate_across_levels(problems: &[Problem]) -> Vec<ProblemIssue> {
    let mut issues = Vec::new();
    let mut titles: HashMap<&str, &Problem> = HashMap::new();
    let mut answers: HashMap<&str, &Problem> = HashMap::new();

    for p in problems {
        if let Some(prev) = titles.insert(p.title.as_str(), p) {
            if prev.level != p.level {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!(
                        "title が {} ({}) と重複しています: 「{}」",
                        prev.id,
                        prev.level.label_ja(),
                        p.title
                    ),
                ));
            }
        }
        if let Some(prev) = answers.insert(p.answer_code.as_str(), p) {
            if prev.level != p.level {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!(
                        "answer_code が {} ({}) と完全に同一です",
                        prev.id,
                        prev.level.label_ja()
                    ),
                ));
            }
        }
    }
    issues
}

/// `hidden_tests` に現れる「検査名らしい文字列リテラル」の種類数。
///
/// どのテンプレートも `check(<条件>, "名前")` の形なので、判定の目印そのもの
/// (`test result: ...` / `FAILED: `) を除いたリテラルの種類数が、おおよそ検査の件数になる。
/// ヘルパ関数の名前に依存しないので、生成側が命名を変えても数えられる。
pub fn distinct_check_names(hidden_tests: &str) -> usize {
    let mut names = std::collections::HashSet::new();
    for quote in ['"', '\''] {
        let mut rest = hidden_tests;
        while let Some(start) = rest.find(quote) {
            rest = &rest[start + 1..];
            let Some(end) = rest.find(quote) else { break };
            let lit = &rest[..end];
            rest = &rest[end + 1..];
            let is_marker = lit.contains("test result") || lit.starts_with("FAILED");
            if !is_marker && !lit.is_empty() {
                names.insert(lit.to_string());
            }
        }
    }
    names.len()
}

/// `hidden_tests` が判定契約を満たしているか (言語別)。
///
/// Rust は `#[test]` 形式を維持する (既存 300 問がこれに依存している)。
/// 他言語は成功/失敗の目印を自分で出す必要がある。
fn validate_hidden_tests(p: &Problem) -> Vec<ProblemIssue> {
    let mut issues = Vec::new();
    match p.language {
        Language::Rust => {
            let n = p.hidden_tests.matches("#[test]").count();
            if n < 2 {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!("hidden_tests の #[test] が {n} 個です (最低 2 個)"),
                ));
            }
        }
        _ => {
            if !p.hidden_tests.contains(TEST_OK_MARKER) {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!("hidden_tests が成功の目印 \"{TEST_OK_MARKER}\" を出力していません"),
                ));
            }
            if !p.hidden_tests.contains(TEST_FAILED_MARKER) {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!("hidden_tests が失敗の目印 \"{TEST_FAILED_MARKER}\" を出力していません"),
                ));
            }
            // 「検査は最低 2 件」を数える。
            //
            // `"FAILED: "` の出現回数は使えない — テンプレートではヘルパ関数の中に
            // 1 回だけ現れる定数なので、検査を何件書いても常に 1 になる
            // (= 検査していない検査だった)。代わりに、検査名として渡される
            // 文字列リテラルの種類数を数える。
            let names = distinct_check_names(&p.hidden_tests);
            if names < 2 {
                issues.push(ProblemIssue::new(
                    &p.id,
                    format!("hidden_tests の検査が {names} 件です (最低 2 件、うち 1 件は境界値)"),
                ));
            }
        }
    }
    issues
}

// ---- 実行検証 ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    pub problem_id: String,
    pub kind: CaseKind,
    /// exit 0 かつ stdout に成功の目印があるか
    pub passed: bool,
    pub output: String,
}

#[derive(Debug, Default)]
pub struct BatchReport {
    pub outcomes: Vec<CaseOutcome>,
    pub container_runs: usize,
    pub errors: Vec<String>,
}

/// 提出コードが「テストを最後まで走らせて全部通った」かどうか。
/// 終了コードだけでは、テストが 1 件も走らないまま exit 0 したケースと区別できない。
fn case_passed(exit_code: i32, stdout: &str) -> bool {
    exit_code == 0 && stdout.contains(TEST_OK_MARKER)
}

/// 1 バッチ (= 1 ファイル分の問題群) をコンテナ 1 回で検証する。
pub fn run_batch_docker(
    language: Language,
    problems: &[Problem],
    workdir: &Path,
) -> std::io::Result<BatchReport> {
    let mut report = BatchReport::default();
    if problems.is_empty() {
        return Ok(report);
    }

    let cases: Vec<RunCase> = problems
        .iter()
        .flat_map(|p| {
            [
                (CaseKind::Answer, &p.answer_code),
                (CaseKind::Starter, &p.starter_code),
            ]
            .into_iter()
            .map(|(kind, code)| RunCase {
                problem_id: p.id.clone(),
                kind,
                code: compose_submission(language, code, &p.hidden_tests),
            })
        })
        .collect();

    // 作業ディレクトリを組む
    let _ = fs::remove_dir_all(workdir);
    fs::create_dir_all(workdir.join("cases"))?;
    for c in &cases {
        let dir = workdir.join("cases").join(c.dir_name());
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(language.source_file_name()), &c.code)?;
    }

    let plan = plan_batch(language, &cases, workdir);
    for run in &plan {
        fs::write(workdir.join("run.sh"), &run.script)?;
        let out = docker::execute(run)?;
        report.container_runs += 1;
        if !out.status.success() {
            report.errors.push(format!(
                "コンテナが異常終了しました (image={}, status={:?}): {}",
                run.image,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>()
            ));
        }
    }

    for c in &cases {
        let dir = workdir.join("cases").join(c.dir_name());
        let exit = fs::read_to_string(dir.join("_exit")).ok();
        let stdout = fs::read_to_string(dir.join("_stdout")).unwrap_or_default();
        let stderr = fs::read_to_string(dir.join("_stderr")).unwrap_or_default();

        let Some(code) = exit.as_deref().and_then(|s| s.trim().parse::<i32>().ok()) else {
            // 実行結果が無い = タイムアウトかコンテナ側の異常。未検証にせず失敗として数える
            report.outcomes.push(CaseOutcome {
                problem_id: c.problem_id.clone(),
                kind: c.kind,
                passed: false,
                output: "実行結果が記録されていません (タイムアウトの可能性)".into(),
            });
            continue;
        };
        report.outcomes.push(CaseOutcome {
            problem_id: c.problem_id.clone(),
            kind: c.kind,
            passed: case_passed(code, &stdout),
            output: format!("exit={code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"),
        });
    }
    Ok(report)
}

