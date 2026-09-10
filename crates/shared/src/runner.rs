//! 提出コードを「どう動かすか」の唯一の定義 — 本番 (Vercel Sandbox) と
//! verifier (ローカル Docker) が同じコマンドを使うための層。
//!
//! ここを 1 箇所に保つのが ADR 0003 の要点。以前は本番が Wandbox、検証がローカル
//! Docker で**別々のコマンド定義**を持っていたため、「verifier は緑なのに本番は
//! TS2550」のように、検証をすり抜ける差が構造的に存在した。
//!
//! 出力は 1 回のシェル実行にまとめ、**nonce 区切りのセクション**で返す。これは
//! コンパイル診断とプログラム出力を分離して受け取るため (`compile_failed` を
//! 診断テキストだけから決める契約は ADR 0002 から不変)。

use crate::language::Language;
use crate::contract::{harness_ran, has_compile_error, strip_csharp_build_noise, ExecuteResponse};

/// 1 ケースの実行上限 (秒)。無限ループを書いた提出でサンドボックスを占有させない。
pub const CASE_TIMEOUT_SECS: u64 = 20;

/// 本番 (Vercel Sandbox) が使うイメージ。VCR の `knocks-runtime` リポジトリのタグ。
/// 更新したら `scripts/build-runner-image.sh --push <tag>` で上げてここを差し替える。
/// 本番側は環境変数 `KNOCKS_SANDBOX_IMAGE` で上書きできる (切り戻し用)。
pub const PRODUCTION_IMAGE: &str = "knocks-runtime:2026-09-11";

/// 検証 (ローカル Docker) が使うイメージ。`scripts/build-runner-image.sh` が作る。
/// **本番と同じ Dockerfile から作る**ことが、検証と本番を食い違わせない唯一の条件。
pub const LOCAL_IMAGE: &str = "knocks-runtime:local";

/// イメージ内に焼いてあるひな形プロジェクトの置き場。
/// Rust (cargo) と C# (csproj) は単一ファイルでは動かないため、restore 済みの
/// プロジェクトをイメージに焼き、ケースごとにコピーして使う。
pub const TEMPLATE_ROOT: &str = "/opt/knocks";

/// 言語ごとの実行手順。
///
/// - `prepare`: ソースを置いた後、コンパイル前に要る準備 (シェル片。省略可)
/// - `compile`: 独立したコンパイル段階 (シェル片。無い言語は None)
/// - `run`: 実行コマンド。**プレーンな argv** であること (`timeout` に直接渡すため、
///   `&&` やリダイレクトを含めてはいけない)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPlan {
    pub prepare: Option<String>,
    pub compile: Option<String>,
    pub run: String,
}

/// 診断テキストをどのチャンネルから取るか。
///
/// 分ける理由: gcc / javac / tsc / Roslyn は独立したコンパイル段階の出力に診断を出すが、
/// `cargo test` は 1 段階で、診断も実行時出力も同じ stderr に出る。Python /
/// JavaScript はそもそもコンパイル段階が無く、構文エラーはインタプリタ起動時の
/// stderr に出る。ここを一律に扱うと「プログラムが "error:" を印字しただけで
/// コンパイルエラー」か、逆に「構文エラーが RuntimeError に落ちる」のどちらかが必ず起きる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticsChannel {
    /// 独立したコンパイル段階の出力 (C++ / Java / C# / TypeScript)
    CompileStage,
    /// 実行段階の stderr (Rust の cargo test / Python / JavaScript)
    RunStderr,
}

pub fn diagnostics_channel(language: Language) -> DiagnosticsChannel {
    match language {
        Language::Cpp | Language::Java | Language::Csharp | Language::Typescript => {
            DiagnosticsChannel::CompileStage
        }
        Language::Rust | Language::Python | Language::Javascript => DiagnosticsChannel::RunStderr,
    }
}

/// その言語の実行手順。ファイル名は `Language::source_file_name()` と対応する。
pub fn run_plan(language: Language) -> RunPlan {
    let src = language.source_file_name();
    match language {
        // cargo プロジェクトのひな形を持ってきて src/lib.rs に差し替える。
        // Playground と同じ「lib クレート + tests モード」を再現する (ADR 0002 の判定契約)。
        Language::Rust => RunPlan {
            prepare: Some(format!(
                "cp -a {TEMPLATE_ROOT}/rust/. . && mkdir -p src && mv {src} src/lib.rs"
            )),
            compile: None,
            run: "cargo test --offline".into(),
        },
        Language::Cpp => RunPlan {
            prepare: None,
            compile: Some(format!("g++ -std=c++17 -w -o _prog {src}")),
            run: "./_prog".into(),
        },
        // Wandbox 時代は prog.java 固定だったのでクラスを public にできなかった。
        // 自前イメージでは制約は消えるが、問題コンテンツ 300 問を触らないため名前は踏襲する。
        Language::Java => RunPlan {
            prepare: None,
            compile: Some(format!("javac -nowarn {src}")),
            run: "java Main".into(),
        },
        // restore 済みプロジェクトをコピーし、ひな形の Program.cs を消す。
        // 提出は prog.cs のまま置く (csproj が **/*.cs を拾う)。消さないと
        // エントリポイントが 2 つになり、**提出のファイル名が診断に出なくなる**。
        //
        // --no-restore: サンドボックスは外向き通信を閉じている (deny-all) ため
        // GenerateFullPaths=false: 診断に一時ディレクトリの絶対パスを混ぜないため
        Language::Csharp => RunPlan {
            prepare: Some(format!("cp -a {TEMPLATE_ROOT}/csharp/. . && rm -f Program.cs")),
            compile: Some(
                "dotnet build --no-restore -v q --nologo -p:GenerateFullPaths=false -o _out".into(),
            ),
            run: "dotnet _out/knocks.dll".into(),
        },
        Language::Python => RunPlan {
            prepare: None,
            compile: None,
            run: format!("python3 {src}"),
        },
        // target を固定するのは必須。ここを省くと ES2019+ の API を使う正解が TS2550 で落ちる
        Language::Typescript => RunPlan {
            prepare: None,
            compile: Some(format!("tsc {} {src}", crate::language::tsc_flags_cli())),
            run: "node prog.js".into(),
        },
        Language::Javascript => RunPlan {
            prepare: None,
            compile: None,
            run: format!("node {src}"),
        },
    }
}

/// セクション区切りに使う nonce。提出コードが区切りを偽装して
/// 「コンパイルエラーを正解に見せる」ことを防ぐため、実行ごとに変える。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nonce(String);

impl Nonce {
    /// 呼び出し側が用意した乱数 (16 進など) から作る。
    /// shared は wasm でも使うので、乱数生成器はここに持たない。
    pub fn new(raw: &str) -> Self {
        let cleaned: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).take(32).collect();
        Nonce(format!("KNOCKS{cleaned}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// サンドボックス / コンテナ内で走らせるシェルスクリプトを組む。
///
/// `code_base64` として渡すのは、提出コードに任意のバイト列 (クォート・改行・
/// `$(...)`) が入りうるため。シェルスクリプトに直接埋めると提出側がスクリプトを壊せる。
pub fn build_script(language: Language, code_base64: &str, nonce: &Nonce) -> String {
    let plan = run_plan(language);
    let src = language.source_file_name();
    let n = nonce.as_str();
    let t = CASE_TIMEOUT_SECS;

    let mut s = String::new();
    s.push_str("set -u\n");
    // ホームが無い / 書けない環境でも dotnet と cargo が動くようにする
    s.push_str(
        "export HOME=${HOME:-/tmp}\n\
         export DOTNET_CLI_TELEMETRY_OPTOUT=1\n\
         export DOTNET_NOLOGO=1\n\
         export DOTNET_CLI_HOME=/tmp\n",
    );
    s.push_str("__d=$(mktemp -d) && cd \"$__d\" || exit 90\n");
    s.push_str(&format!("printf %s '{code_base64}' | base64 -d > {src}\n"));

    // コンパイル段階 (準備を含む)。出力は _cout / _cerr に分けて残す。
    let compile_block = match (&plan.prepare, &plan.compile) {
        (Some(p), Some(c)) => Some(format!("{p} && {c}")),
        (Some(p), None) => Some(p.clone()),
        (None, Some(c)) => Some(c.clone()),
        (None, None) => None,
    };
    match compile_block {
        Some(block) => s.push_str(&format!("{{ {block} ; }} >_cout 2>_cerr\n__c=$?\n")),
        None => s.push_str(": >_cout\n: >_cerr\n__c=0\n"),
    }

    // 実行段階。コンパイルが落ちたら走らせない (Wandbox / Playground と同じ挙動)。
    s.push_str(&format!(
        "if [ \"$__c\" -eq 0 ]; then\n  \
           timeout {t} {run} >_pout 2>_perr\n  \
           echo $? >_exit\n\
         else\n  : >_pout\n  : >_perr\n  : >_exit\nfi\n",
        run = plan.run
    ));

    // セクションを 1 本の stdout にまとめて返す。
    // 各区切りの前に改行を 1 つ入れるのは、直前のファイルが改行で終わっていなくても
    // 区切りが行頭から始まるようにするため (パーサ側でこの 1 つだけ取り除く)。
    for section in ["cout", "cerr", "pout", "perr", "exit"] {
        s.push_str(&format!("printf '\\n{n}:{section}\\n'\ncat _{section} 2>/dev/null\n"));
    }
    s.push_str(&format!("printf '\\n{n}:end\\n'\nexit 0\n"));
    s
}

/// スクリプトが返した生の実行結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunnerOutput {
    /// コンパイル段階の stdout (C# の MSBuild 診断はここに来る)
    pub compile_stdout: String,
    /// コンパイル段階の stderr (gcc / javac はここ)
    pub compile_stderr: String,
    pub program_stdout: String,
    pub program_stderr: String,
    /// プログラムの終了コード。コンパイル段階で止まった場合は None。
    pub exit_code: Option<i32>,
    /// コンパイル段階が 0 以外で終わったか。
    pub compile_failed_stage: bool,
}

/// スクリプトの stdout をセクションに分解する。
///
/// 区切りが 1 つでも欠けていたら (= スクリプトが途中で死んだ) `Err` を返す。
/// 黙って空の結果を返すと「テストが 0 件走って exit 0」(`NoTestsRun`) と区別できなくなる。
pub fn parse_runner_output(nonce: &Nonce, raw: &str) -> Result<RunnerOutput, String> {
    let marker = format!("{}:", nonce.as_str());
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        // 区切りは「行全体が <nonce>:<名前>」のときだけ。提出コードが同じ形を
        // 印字しても nonce が実行ごとに違うので一致しない。
        if let Some(name) = trimmed.strip_prefix(&marker) {
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase()) {
                if let Some(done) = current.take() {
                    sections.push(done);
                }
                current = Some((name.to_string(), String::new()));
                continue;
            }
        }
        if let Some((_, buf)) = current.as_mut() {
            buf.push_str(line);
        }
        // 最初の区切りより前の行 (シェルの起動メッセージ等) は捨てる
    }
    if let Some(done) = current.take() {
        sections.push(done);
    }

    // **後勝ちで読む。** 本物のセクションは必ずプログラム出力より後に出るので、
    // 提出コードが nonce を当てて偽のセクションを印字しても本物が勝つ。
    // 先勝ちにすると `<nonce>:exit` を印字するだけで不正解を正解にできる。
    let take = |name: &str| -> Option<String> {
        sections.iter().rev().find(|(k, _)| k == name).map(|(_, v)| {
            // 区切りの直前に入れた改行 1 つだけを戻す
            let mut v = v.clone();
            if v.ends_with('\n') {
                v.pop();
            }
            v
        })
    };

    if take("end").is_none() {
        return Err("実行結果が途中で途切れています".to_string());
    }
    let (
        Some(compile_stdout),
        Some(compile_stderr),
        Some(program_stdout),
        Some(program_stderr),
        Some(exit_raw),
    ) = (take("cout"), take("cerr"), take("pout"), take("perr"), take("exit"))
    else {
        return Err("実行結果のセクションが欠けています".to_string());
    };

    let exit_trimmed = exit_raw.trim();
    let exit_code = if exit_trimmed.is_empty() {
        None
    } else {
        exit_trimmed.parse::<i32>().ok()
    };

    Ok(RunnerOutput {
        compile_stdout,
        compile_stderr,
        program_stdout,
        program_stderr,
        // exit セクションが空 = コンパイル段階で止まった
        compile_failed_stage: exit_code.is_none(),
        exit_code,
    })
}

/// 実行結果を `ExecuteResponse` (フロントとの契約) に詰め替える。
///
/// 判定順序と `compile_failed` の決め方は ADR 0002 から不変。変わったのは
/// 「診断をどのチャンネルから取るか」が言語ごとに明示されたことだけ。
pub fn normalize_runner(language: Language, out: &RunnerOutput) -> ExecuteResponse {
    let mut diagnostics = String::new();
    for part in [&out.compile_stderr, &out.compile_stdout] {
        if !part.trim().is_empty() {
            if !diagnostics.is_empty() {
                diagnostics.push('\n');
            }
            diagnostics.push_str(part.trim_end());
        }
    }
    if language == Language::Csharp {
        diagnostics = strip_csharp_build_noise(&diagnostics);
    }

    // 診断をどこから読むか (コンパイル段階 / 実行段階の stderr)
    let diag_text = match diagnostics_channel(language) {
        DiagnosticsChannel::CompileStage => diagnostics.clone(),
        DiagnosticsChannel::RunStderr => out.program_stderr.clone(),
    };

    // 判定テストが走ったなら、その時点でコンパイルは成功している。
    // cargo test はテスト失敗時に stderr へ `error: test failed` を出すので、
    // ここを見落とすと Rust のテスト失敗が全件「コンパイルエラー」になる。
    let compile_failed = !harness_ran(&out.program_stdout)
        && (out.compile_failed_stage || has_compile_error(language, &diag_text));

    let mut stderr = diagnostics;
    if !out.program_stderr.trim().is_empty() {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(out.program_stderr.trim_end());
    }

    ExecuteResponse {
        success: !out.compile_failed_stage && out.exit_code == Some(0),
        stdout: out.program_stdout.clone(),
        stderr,
        compile_failed,
    }
}

/// 提出コードをスクリプトへ埋めるための base64。
///
/// 依存を増やさないのは、shared が wasm ターゲットでもビルドされるため。
/// 標準の base64 (RFC 4648、パディングあり) を返す。
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}
