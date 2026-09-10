//! 検証の実行層。**`docker run` を組み立てる唯一の場所**。
//!
//! ここに閉じてあるのは、「バッチ 1 ファイルにつきコンテナ 1 回」を数えられる形で
//! 保証するため (1 問ごとに起こすと、1800 問 × 2 の起動オーバーヘッドだけで
//! 数時間かかる)。`plan_batch` は実行せず計画だけ返すので、回数はテストで固定できる。

use std::path::{Path, PathBuf};
use std::process::Command;

use shared::language::Language;
use shared::runner::{run_plan, LOCAL_IMAGE};

/// 1 問につき 2 通り実行する: 模範解答は通り、初期コードは落ちること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseKind {
    Answer,
    Starter,
}

impl CaseKind {
    pub fn suffix(self) -> &'static str {
        match self {
            CaseKind::Answer => "answer",
            CaseKind::Starter => "starter",
        }
    }
}

/// 実行 1 件 (= 1 問の answer か starter)。
#[derive(Clone, Debug)]
pub struct RunCase {
    pub problem_id: String,
    pub kind: CaseKind,
    /// 合成済みの提出コード (ユーザーコード + hidden_tests)
    pub code: String,
}

impl RunCase {
    /// 作業ディレクトリ内でこのケースが使うフォルダ名。
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.problem_id, self.kind.suffix())
    }
}

/// コンテナ 1 回分の実行計画。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerRun {
    pub image: String,
    /// ホスト側の作業ディレクトリ (コンテナの /w にマウントする)
    pub workdir: PathBuf,
    /// コンテナ内で実行する sh スクリプト
    pub script: String,
    pub network_disabled: bool,
    /// コンテナ全体の上限秒数 (ホスト側のハングに対する保険)
    pub overall_timeout_secs: u64,
}

/// 1 ケースあたりの実行上限。無限ループを書いた問題でバッチが止まらないようにする。
/// 本番 (Sandbox) と同じ値を使う (正本は shared::runner)。
pub const CASE_TIMEOUT_SECS: u64 = shared::runner::CASE_TIMEOUT_SECS;
/// コンテナ全体の上限。ケース数に比例させる。
pub const OVERALL_TIMEOUT_BASE_SECS: u64 = 120;

/// バッチをコンテナ何回で回すかの計画を返す。**実行はしない。**
///
/// 7 言語すべて本番と同じ 1 枚のイメージ (`shared::runner::LOCAL_IMAGE`) で回す。
/// 以前は言語ごとに別イメージで、Rust だけローカル cargo だった (ADR 0002)。
pub fn plan_batch(language: Language, cases: &[RunCase], workdir: &Path) -> Vec<DockerRun> {
    if cases.is_empty() {
        return Vec::new();
    }
    vec![DockerRun {
        image: LOCAL_IMAGE.to_string(),
        workdir: workdir.to_path_buf(),
        script: container_script(language, cases),
        network_disabled: true,
        overall_timeout_secs: OVERALL_TIMEOUT_BASE_SECS + CASE_TIMEOUT_SECS * cases.len() as u64,
    }]
}

/// コンテナ内で走らせる sh スクリプトを組む。
///
/// **コマンドは `shared::runner::run_plan` から取る。** ここに言語別のコマンドを
/// 書くと本番 (Sandbox) との二重管理になり、2026-08-29 の TypeScript target ずれ
/// (「verifier は緑なのに本番は TS2550」) と同じ事故が再発する。
///
/// 各ケースのディレクトリで「準備 → コンパイル → 実行」を行い、`_stdout` / `_stderr` /
/// `_exit` に結果を残す。stdout と stderr を混ぜないのは、正解の目印
/// (`test result: ok`) が stdout にあることを判定条件にしているため。
pub fn container_script(language: Language, cases: &[RunCase]) -> String {
    let plan = run_plan(language);
    let t = CASE_TIMEOUT_SECS;

    // 1 ケース分の「準備してコンパイルして実行する」コマンド列。
    // 出力は呼び出し側でリダイレクトする。
    let run_one = [plan.prepare, plan.compile, Some(plan.run)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" && ");

    let mut s = String::from("#!/bin/sh
# 自動生成 (verifier)
");
    // ひな形の cargo / dotnet が書き込める場所を与える (イメージ内の /opt は読み取り専用扱い)
    s.push_str(
        "export HOME=${HOME:-/tmp}
\
         export DOTNET_CLI_TELEMETRY_OPTOUT=1
\
         export DOTNET_NOLOGO=1
\
         export DOTNET_CLI_HOME=/tmp
",
    );

    for case in cases {
        let dir = case.dir_name();
        s.push_str(&format!(
            "cd /w/cases/{dir} 2>/dev/null && {{ timeout {t} sh -c '{run_one}' >_stdout 2>_stderr; echo $? >_exit; }}
"
        ));
    }
    s.push_str("exit 0
");
    s
}

/// 計画を実際に走らせる。呼ぶ側は `plan_batch` の結果をそのまま渡す。
pub fn execute(run: &DockerRun) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("timeout");
    cmd.arg(run.overall_timeout_secs.to_string())
        .arg("docker")
        .arg("run")
        .arg("--rm");
    if run.network_disabled {
        cmd.arg("--network=none");
    }
    cmd.arg("-v")
        .arg(format!("{}:/w", run.workdir.display()))
        .arg("-w")
        .arg("/w")
        .arg(&run.image)
        .arg("sh")
        .arg("/w/run.sh");
    cmd.output()
}

/// Docker と実行イメージが揃っているかを着手時に 1 回だけ検査する。
/// 揃っていないまま進むと「検証したつもりの未検証データ」が積み上がる。
pub fn preflight() -> Result<(), String> {
    let ok = Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Err("docker が使えません (デーモンが起動しているか確認してください)".into());
    }
    let found = Command::new("docker")
        .args(["image", "inspect", LOCAL_IMAGE])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if found {
        Ok(())
    } else {
        Err(format!(
            "実行イメージ {LOCAL_IMAGE} がありません。
  bash scripts/build-runner-image.sh で作ってください (本番と同じ Dockerfile から作られます)"
        ))
    }
}
