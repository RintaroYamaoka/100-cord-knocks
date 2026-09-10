//! 実行結果コンソール。コンパイラ / ランタイムの出力を行種別で色分けする。
//! 行の分類は言語共通 (shared::contract::classify_line)、エラーコードのリンクは
//! 公式のコード別ページがある Rust だけ。

use leptos::prelude::*;
use shared::language::Language;
use shared::contract::{classify_line, ExecuteResponse, LineKind, Outcome};

use crate::lang::{console_idle_hint, console_running_hint, links_error_codes};

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Idle,
    Running,
    Done { resp: ExecuteResponse, outcome: Outcome },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleSegment {
    Text(String),
    ErrorCode(String),
}

/// 1 行を「エラーコード (E0308 等)」とそれ以外のテキストに分割する。
pub fn split_error_codes(line: &str) -> Vec<ConsoleSegment> {
    let mut segs = Vec::new();
    let mut cursor = 0usize;
    let mut search_from = 0usize;
    while let Some(rel) = line[search_from..].find("error[") {
        let code_start = search_from + rel + "error[".len();
        let Some(end_rel) = line[code_start..].find(']') else { break };
        let code = &line[code_start..code_start + end_rel];
        let valid = code.len() == 5
            && code.starts_with('E')
            && code[1..].chars().all(|c| c.is_ascii_digit());
        if valid {
            if code_start > cursor {
                segs.push(ConsoleSegment::Text(line[cursor..code_start].to_string()));
            }
            segs.push(ConsoleSegment::ErrorCode(code.to_string()));
            cursor = code_start + end_rel;
            search_from = cursor;
        } else {
            search_from = code_start;
        }
    }
    if cursor < line.len() || segs.is_empty() {
        segs.push(ConsoleSegment::Text(line[cursor..].to_string()));
    }
    segs
}

fn line_class(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Error => "line-error",
        LineKind::Warning => "line-warning",
        LineKind::Note => "line-note",
        LineKind::Plain => "line-plain",
    }
}

/// 診断 1 行の描画。`link_codes` が false の言語ではエラーコードを素のテキストで出す
/// (rustc の error_codes に相当する公式ページが無いので、リンクにすると 404 に飛ばす)。
fn render_stderr_line(line: &str, link_codes: bool) -> impl IntoView {
    let class = line_class(classify_line(line));
    let segs = if link_codes {
        split_error_codes(line)
    } else {
        vec![ConsoleSegment::Text(line.to_string())]
    };
    let segs = segs
        .into_iter()
        .map(|seg| match seg {
            ConsoleSegment::Text(t) => view! { <span>{t}</span> }.into_any(),
            ConsoleSegment::ErrorCode(c) => {
                let href = format!("https://doc.rust-lang.org/error_codes/{c}.html");
                view! {
                    <a class="error-code-link" href=href target="_blank" rel="noreferrer" title="エラーコードの公式解説を開く">{c}</a>
                }
                .into_any()
            }
        })
        .collect_view();
    view! { <div class=class>{segs}</div> }
}

/// 判定結果バナーの (CSS クラス, 表示文言)。
pub fn outcome_banner_parts(outcome: Outcome) -> (&'static str, &'static str) {
    match outcome {
        Outcome::Passed => ("outcome-banner passed", "✓ 正解!"),
        Outcome::TestsFailed => ("outcome-banner tests-failed", "△ テスト失敗 — もう一歩"),
        Outcome::CompileError => ("outcome-banner compile-error", "✗ コンパイルエラー"),
        Outcome::RuntimeError => ("outcome-banner runtime-error", "✗ 実行時エラー"),
        Outcome::NoTestsRun => ("outcome-banner no-tests-run", "✗ テストが実行されませんでした"),
    }
}

/// 結果だけでは理由が分からない種別に、なぜそうなったかの補足を出す。
pub fn outcome_note(outcome: Outcome) -> Option<&'static str> {
    match outcome {
        Outcome::NoTestsRun => Some(
            "判定用テストは 1 件も実行されませんでした。テストはあなたのコードの後ろに連結されるので、\
             途中でプログラムを終了させる (exit / process.exit / System.exit など) と、そこから先が走りません。\
             終了処理を消すか、関数の中だけで完結する形に書き直してください。",
        ),
        _ => None,
    }
}

fn outcome_banner(outcome: Outcome) -> impl IntoView {
    let (class, label) = outcome_banner_parts(outcome);
    view! { <span class=class>{label}</span> }
}

#[component]
pub fn ConsolePane(
    state: Signal<RunState>,
    language: Signal<Language>,
    on_next: Callback<()>,
) -> impl IntoView {
    let head_extra = move || match state.get() {
        RunState::Running => Some(view! { <span class="outcome-banner"><span class="spinner"></span>" コンパイル・実行中…"</span> }.into_any()),
        RunState::Done { outcome, .. } => Some(outcome_banner(outcome).into_any()),
        RunState::Failed(_) => Some(view! { <span class="outcome-banner upstream-error">"! 実行できませんでした"</span> }.into_any()),
        RunState::Idle => None,
    };

    let body = move || match state.get() {
        RunState::Idle => view! {
            <div class="console-placeholder">{console_idle_hint()}</div>
        }
        .into_any(),
        RunState::Running => view! {
            <div class="console-placeholder">{console_running_hint(language.get())}</div>
        }
        .into_any(),
        RunState::Failed(msg) => view! { <div class="line-error">{msg}</div> }.into_any(),
        RunState::Done { resp, outcome } => {
            let link_codes = links_error_codes(language.get());
            let note_view = outcome_note(outcome)
                .map(|note| view! { <div class="console-note">{note}</div> });
            let stderr_view = (!resp.stderr.trim().is_empty()).then(|| {
                let lines = resp
                    .stderr
                    .lines()
                    .map(|l| render_stderr_line(l, link_codes))
                    .collect_view();
                view! {
                    <div class="console-section-label">"診断出力 (stderr)"</div>
                    <div>{lines}</div>
                }
            });
            let stdout_view = (!resp.stdout.trim().is_empty()).then(|| {
                view! {
                    <div class="console-section-label">"実行出力 (stdout)"</div>
                    <div class="stdout-block">{resp.stdout.clone()}</div>
                }
            });
            // コンパイルエラー時は stderr を先に、それ以外はテスト結果 (stdout) を先に見せる
            if outcome == Outcome::CompileError {
                view! {
                    {note_view}
                    {stderr_view}
                    {stdout_view}
                }
                .into_any()
            } else {
                view! {
                    {note_view}
                    {stdout_view}
                    {stderr_view}
                }
                .into_any()
            }
        }
    };

    let next_cta = move || {
        matches!(state.get(), RunState::Done { outcome: Outcome::Passed, .. }).then(|| {
            view! {
                <button class="next-cta" on:click=move |_| on_next.run(())>"次の問題へ →"</button>
            }
        })
    };

    // スマホ幅では画面の高さが足りないので、コンソールは既定で畳んでおき、
    // 実行したときだけ開く (畳んだ状態は見出しの帯だけが残る)。
    // デスクトップ幅では CSS がこのクラスを見ないため、常に開いた表示のまま。
    let open = RwSignal::new(false);
    Effect::new(move |_| {
        if !matches!(state.get(), RunState::Idle) {
            open.set(true);
        }
    });

    view! {
        <div class="console" class:open=move || open.get()>
            <div class="console-head">
                "実行結果" {head_extra} <span class="toolbar-spacer"></span> {next_cta}
                <button
                    class="console-toggle"
                    on:click=move |_| open.update(|o| *o = !*o)
                    title="実行結果の開閉"
                    aria-label="実行結果の開閉"
                    aria-expanded=move || open.get().to_string()
                >
                    {move || if open.get() { "▼" } else { "▲" }}
                </button>
            </div>
            <div class="console-body">{body}</div>
        </div>
    }
}
