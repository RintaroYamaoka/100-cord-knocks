//! ルートコンポーネント: 状態の持ち主と UI 配線。純ロジックは各モジュール / shared 側に置く。

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::language::Language;
use shared::contract::{classify, ExecuteRequest, Outcome};
use shared::problem::{compose_submission, Level, Problem};
use shared::progress::{
    passed_count, progress_key, saved_code_of, status_of, ProblemStatus, ProgressEntry, ProgressMap,
    StatusFilter,
};

use crate::console::{ConsolePane, RunState};
use crate::lang::{initial_language, resolve_selection, selector_languages};
use crate::layout::{load_layout, LayoutSizes, SplitTarget};
use crate::list::Sidebar;
use crate::mobile::{pane_after_problem_change, MobilePane};
use crate::problem_view::ProblemPane;
use crate::splitter::Splitter;
use crate::{api, editor, storage};

/// 判定結果による進捗ステータス遷移。一度 Passed になったら失敗しても降格しない。
pub fn next_status(current: ProblemStatus, outcome: Outcome) -> ProblemStatus {
    match (current, outcome) {
        (_, Outcome::Passed) => ProblemStatus::Passed,
        (ProblemStatus::Passed, _) => ProblemStatus::Passed,
        _ => ProblemStatus::Attempted,
    }
}

/// エディタに載せるコード。下書きがあればそれ、無ければ starter_code。
///
/// 下書きの取り出しは `saved_code_of` (= `progress_key`) 経由に固定してある。
/// 素の `id` で引くと、`b001` を共有する 7 言語の下書きが混ざる。
pub fn code_for(p: &Problem, map: &ProgressMap) -> String {
    saved_code_of(map, p)
        .map(String::from)
        .unwrap_or_else(|| p.starter_code.clone())
}

/// 選択中の言語・レベルの問題データを覚えておくキー。
type CacheKey = (Language, Level);

/// レベル切替 (初級/中級/上級)。
///
/// デスクトップではヘッダーに、スマホでは一覧ペインの中に置く。狭い画面では
/// ヘッダーに 3 タブ + 言語セレクタ + 進捗を並べると溢れて横スクロールが出るうえ、
/// レベルは一覧の絞り込みなので、一覧と一緒にあるほうが操作としても素直。
/// 表示の出し分けは CSS (`class` で渡す修飾クラス) が行う。
#[component]
pub fn LevelTabs(level: RwSignal<Level>, #[prop(into)] class: String) -> impl IntoView {
    view! {
        <div class=format!("level-tabs {class}")>
            {Level::ALL
                .iter()
                .map(|lv| {
                    let lv = *lv;
                    view! {
                        <button
                            class="level-tab"
                            class:active=move || level.get() == lv
                            on:click=move |_| level.set(lv)
                        >
                            {lv.label_ja()}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let language = RwSignal::new(initial_language(storage::load_language().as_deref()));
    let level = RwSignal::new(Level::Beginner);
    let cache: RwSignal<HashMap<CacheKey, Vec<Problem>>> = RwSignal::new(HashMap::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let selected: RwSignal<Option<Problem>> = RwSignal::new(None);
    let filter = RwSignal::new(StatusFilter::All);
    let query = RwSignal::new(String::new());
    // 旧フラットキー (Rust 専用時代の `b001`) はここで `rust/b001` に移行される
    let (initial_progress, migration_save_failed) = storage::load_progress_migrated();
    let progress: RwSignal<ProgressMap> = RwSignal::new(initial_progress);
    let run_state: RwSignal<RunState> = RwSignal::new(RunState::Idle);
    let revealed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let editor_ready = RwSignal::new(false);
    // 問題データを取得中か。これが無いと、切替直後に「一覧が空 = 該当なし」と
    // 表示しつつ前の言語の問題を実行できてしまう
    let loading = RwSignal::new(false);
    // 進捗を保存できなかったか (localStorage のクォータ超過など)。
    // 握り潰すと、画面上は成功と区別が付かないまま進捗が消える。
    // 起動時の取り込みが保存できなかった場合もここに乗せる
    let storage_failed = RwSignal::new(migration_save_failed);
    let mount_started = RwSignal::new(false);
    // データが 3 レベル揃っていることを確認できた言語 (確認前は空)
    let available: RwSignal<Vec<Language>> = RwSignal::new(Vec::new());
    let probe_started = RwSignal::new(false);
    // スマホ幅で全画面表示するペイン (デスクトップ幅では CSS が参照しないので無害)
    let pane = RwSignal::new(MobilePane::List);

    // 起動時に一度だけ、収録済み言語のマニフェストを読む。
    //
    // 以前は 21 本の HEAD を**逐次**投げて確かめていたが、揃うまで数秒かかり、
    // その間セレクタには Rust しか出ない。回線が遅いほど長く、1 本でも瞬断すれば
    // その言語は消える。リクエスト 1 本にして、待ち時間も取りこぼしも構造的に無くす。
    Effect::new(move |_| {
        if probe_started.get_untracked() {
            return;
        }
        probe_started.set(true);
        spawn_local(async move {
            // 取得や解釈に失敗したら None。そのときはセレクタを触らない
            // (フォールバックは selector_languages が持つ)
            let Some(langs) = api::fetch_language_manifest().await else {
                return;
            };
            let resolved = resolve_selection(language.get_untracked(), &langs);
            available.set(langs);
            if resolved != language.get_untracked() {
                language.set(resolved);
            }
        });
    });

    // 言語 / レベル切替時に問題データをフェッチ (キャッシュ済みなら何もしない)
    Effect::new(move |_| {
        let key = (language.get(), level.get());
        if cache.with_untracked(|c| c.contains_key(&key)) {
            load_error.set(None);
            return;
        }
        // 取得を始める時点で前のエラーを消す。残すと、失敗した言語のエラー文が
        // 次に選んだ言語の読み込み中ずっと表示されたままになる
        load_error.set(None);
        loading.set(true);
        spawn_local(async move {
            let result = api::fetch_problems(key.0, key.1).await;
            // 取得中に別の言語へ切り替わっていたら、その結果で画面を触らない。
            // 触ると「Java の取得に失敗 → Python に切替」で Python を表示しながら
            // Java のエラー文が出る (キャッシュへの投入だけは有効なので行う)
            let still_current = (language.get_untracked(), level.get_untracked()) == key;
            match result {
                Ok(ps) => {
                    cache.update(|c| {
                        c.insert(key, ps);
                    });
                }
                Err(e) => {
                    if still_current {
                        load_error.set(Some(e));
                    }
                }
            }
            if still_current {
                loading.set(false);
            }
        });
    });

    let problems =
        Memo::new(move |_| cache.with(|c| c.get(&(language.get(), level.get())).cloned().unwrap_or_default()));
    // 選択中の言語のデータが実際に読めているか (セレクタのフォールバック判定に使う)
    let current_loaded = Memo::new(move |_| {
        let lang = language.get();
        cache.with(|c| c.keys().any(|(l, _)| *l == lang))
    });
    let visible_languages = Memo::new(move |_| {
        selector_languages(&available.get(), language.get(), current_loaded.get())
    });

    // 下書き保存 (エディタ内容が starter と同じなら保存しない)
    let save_draft_code = move |code: String| {
        let Some(p) = selected.get_untracked() else {
            return;
        };
        progress.update(|m| {
            let e = m.entry(progress_key(&p)).or_insert(ProgressEntry {
                status: ProblemStatus::Unanswered,
                saved_code: None,
                updated_at_ms: 0.0,
            });
            e.saved_code = if code.trim().is_empty() || code == p.starter_code {
                None
            } else {
                Some(code)
            };
            e.updated_at_ms = storage::now_ms();
        });
        if !progress.with_untracked(storage::save_progress) {
            storage_failed.set(true);
        }
    };
    let save_draft = move || {
        if editor_ready.get_untracked() {
            save_draft_code(editor::get_value());
        }
    };

    let select_problem = Callback::new(move |p: Problem| {
        save_draft();
        run_state.set(RunState::Idle);
        let code = progress.with_untracked(|m| code_for(&p, m));
        let lang = p.language;
        selected.set(Some(p));
        if editor_ready.get_untracked() {
            // 言語モードを先に切り替えてから中身を入れる。どちらも再マウントせず、
            // JS 側で debounce 中の変更通知を破棄する契約 (前の言語のコードが
            // 切替後の下書きを上書きしないようにするため)。
            editor::set_language(lang);
            editor::set_value(&code);
            editor::focus();
        }
    });

    // スマホでは、一覧から**明示的に**選んだときだけ問題ペインへ移る。すぐ下の
    // 自動選択 (データ到着時 / 言語・レベル切替時) で移すと、レベルを切り替えただけで
    // 一覧から問題文へ飛ばされ、続けて別の問題を選べなくなる
    let select_from_list = Callback::new(move |p: Problem| {
        select_problem.run(p);
        pane.set(pane_after_problem_change());
    });

    // 問題データ到着時 / 言語・レベル切替時:
    // 未選択か、いま表示している言語・レベルと違う問題が残っていたら先頭を自動選択
    Effect::new(move |_| {
        let ps = problems.get();
        let (lang, lv) = (language.get(), level.get());
        let needs_select = selected.with_untracked(|s| {
            s.as_ref()
                .is_none_or(|p| p.level != lv || p.language != lang)
        });
        if needs_select {
            if let Some(first) = ps.first() {
                select_problem.run(first.clone());
            }
        }
    });

    // 言語の選択を localStorage に残す (次回起動時に復元する)
    Effect::new(move |prev: Option<Language>| {
        let lang = language.get();
        if prev.is_some_and(|p| p == lang) {
            return lang;
        }
        storage::save_language(lang);
        lang
    });

    // 実行して判定
    let run = move || {
        let Some(p) = selected.get_untracked() else {
            return;
        };
        // 取得中は実行させない。ボタンだけ無効化してもキーボード (Ctrl+Enter) から
        // 入れるので、ここで塞ぐ。押せると「新しい言語を表示しながら
        // 前の言語の問題を実行する」状態になる
        if run_state.with_untracked(|s| matches!(s, RunState::Running))
            || !editor_ready.get_untracked()
            || loading.get_untracked()
        {
            return;
        }
        let code = editor::get_value();
        save_draft_code(code.clone());
        run_state.set(RunState::Running);
        let submission = compose_submission(p.language, &code, &p.hidden_tests);
        let req = ExecuteRequest::judge(p.language, &submission);
        spawn_local(async move {
            match api::execute(&req).await {
                Ok(resp) => {
                    let outcome = classify(&resp);
                    progress.update(|m| {
                        let cur = status_of(m, &p);
                        let e = m.entry(progress_key(&p)).or_insert(ProgressEntry {
                            status: ProblemStatus::Unanswered,
                            saved_code: None,
                            updated_at_ms: 0.0,
                        });
                        e.status = next_status(cur, outcome);
                        e.updated_at_ms = storage::now_ms();
                    });
                    if !progress.with_untracked(storage::save_progress) {
            storage_failed.set(true);
        }
                    run_state.set(RunState::Done { resp, outcome });
                }
                Err(e) => run_state.set(RunState::Failed(e)),
            }
        });
    };

    let reset_code = move |_| {
        if let Some(p) = selected.get_untracked() {
            if editor_ready.get_untracked() {
                editor::set_value(&p.starter_code);
                save_draft_code(p.starter_code.clone());
            }
        }
    };

    let nav = move |delta: i64| {
        let ps = problems.get_untracked();
        // 素の id で探さない。b001 は 7 言語すべてに存在するので、一覧が別言語に
        // 切り替わった直後に押されると、別言語の同じ番号へ飛ぶ
        let Some(cur) = selected.with_untracked(|s| s.as_ref().map(progress_key)) else {
            return;
        };
        if let Some(idx) = ps.iter().position(|p| progress_key(p) == cur) {
            let target = idx as i64 + delta;
            if target >= 0 && (target as usize) < ps.len() {
                select_problem.run(ps[target as usize].clone());
                pane.set(pane_after_problem_change());
            }
        }
    };

    // エディタは常設 DOM に一度だけマウントし、glue script のロードを待つ
    Effect::new(move |_| {
        if mount_started.get_untracked() {
            return;
        }
        mount_started.set(true);
        let on_mounted: Rc<dyn Fn()> = Rc::new(move || {
            editor_ready.set(true);
            let sel = selected.get_untracked();
            let lang = sel.as_ref().map_or_else(|| language.get_untracked(), |p| p.language);
            editor::set_language(lang);
            let code = sel
                .as_ref()
                .map(|p| progress.with_untracked(|m| code_for(p, m)));
            editor::set_value(&code.unwrap_or_else(|| {
                format!("{} 左の一覧から問題を選んでください", lang.line_comment())
            }));
            editor::on_run(run);
            editor::on_save(move || save_draft());
            editor::on_change(save_draft_code);
            editor::focus();
        });
        let initial_lang = selected
            .get_untracked()
            .map_or_else(|| language.get_untracked(), |p| p.language);
        editor::mount_retrying("editor-host", String::new(), initial_lang, on_mounted);
    });

    let layout: RwSignal<LayoutSizes> = RwSignal::new(load_layout());
    // ドラッグ中は iframe/エディタがポインタイベントを奪わないよう .main に resizing を付ける
    let layout_dragging = RwSignal::new(false);
    provide_context(layout_dragging);

    // 一覧の選択表示も言語込みのキーで突き合わせる (切替直後に別言語の同 id を
    // 選択中として光らせないため)
    let selected_id = Signal::derive(move || selected.with(|s| s.as_ref().map(progress_key)));
    let passed_in_level = Memo::new(move |_| {
        let ps = problems.get();
        progress.with(|m| passed_count(&ps, m))
    });
    let progress_pct = move || {
        let total = problems.with(|p| p.len());
        if total == 0 {
            0.0
        } else {
            passed_in_level.get() as f64 / total as f64 * 100.0
        }
    };

    view! {
        <div class="app">
            <header class="header">
                <div class="brand">
                    <span class="mark" inner_html=crate::GEAR_SVG></span>
                    <span class="brand-text"><span class="knocks">"100本ノック"</span></span>
                </div>
                <select
                    class="lang-select"
                    title="練習する言語"
                    prop:value=move || {
                        // 一覧にも依存させる: option が後から (存在確認の完了後に) 増えたとき、
                        // ここを再評価しないと select の表示が先頭の言語にずれる
                        let _ = visible_languages.get();
                        language.get().slug().to_string()
                    }
                    on:change=move |ev| {
                        if let Some(l) = Language::from_slug(&event_target_value(&ev)) {
                            language.set(l);
                        }
                    }
                >
                    <For
                        each=move || visible_languages.get()
                        key=|l| l.slug()
                        children=move |l: Language| {
                            view! {
                                <option value=l.slug() selected=move || language.get() == l>
                                    {l.label()}
                                </option>
                            }
                        }
                    />
                </select>
                <LevelTabs level=level class="header-level-tabs"/>
                {move || {
                    storage_failed
                        .get()
                        .then(|| {
                            view! {
                                <span class="storage-warning" title="ブラウザの保存領域がいっぱいの可能性があります">
                                    "⚠ 進捗を保存できませんでした"
                                </span>
                            }
                        })
                }}
                <div class="header-progress">
                    <span class="progress-label">
                        <span class="progress-count">
                            {move || {
                                // 総数を先に読む。passed_in_level (Memo) を先に読むと、
                                // その初回計算の中で problems が読まれ、外側 (この表示) の
                                // 購読が張られないまま固まる (0 / 0 のまま更新されない)
                                let total = problems.with(|p| p.len());
                                format!("{} / {}", passed_in_level.get(), total)
                            }}
                        </span>
                        <span class="progress-unit">" 問クリア"</span>
                    </span>
                    <div class="progress-track">
                        <div class="progress-fill" style:width=move || format!("{}%", progress_pct())></div>
                    </div>
                </div>
            </header>
            <div
                class="main"
                class:resizing=move || layout_dragging.get()
                data-pane=move || pane.get().slug()
                style=move || layout.with(|l| l.css_vars())
            >
                <Sidebar
                    problems=problems
                    progress=progress
                    level=level
                    filter=filter
                    query=query
                    selected_id=selected_id
                    load_error=load_error
                    loading=loading
                    on_select=select_from_list
                />
                <Splitter target=SplitTarget::Sidebar sizes=layout/>
                <ProblemPane problem=selected.into() progress=progress revealed=revealed/>
                <Splitter target=SplitTarget::Problem sizes=layout/>
                <section class="workbench">
                    <div class="workbench-toolbar">
                        <button
                            class="run-btn"
                            prop:disabled=move || {
                                // 取得中は押させない。押せると、切替直後に
                                // 「新しい言語を表示しながら前の言語の問題を実行する」
                                matches!(run_state.get(), RunState::Running)
                                    || selected.with(|s| s.is_none())
                                    || loading.get()
                            }
                            on:click=move |_| run()
                        >
                            {move || {
                                if matches!(run_state.get(), RunState::Running) {
                                    view! { <span class="spinner"></span> }.into_any()
                                } else {
                                    view! { <span>"▶"</span> }.into_any()
                                }
                            }}
                            "実行して判定"
                        </button>
                        <span class="toolbar-hint">"Ctrl+Enter"</span>
                        <div class="toolbar-spacer"></div>
                        <div class="nav-btns">
                            <button class="nav-btn" on:click=move |_| nav(-1) title="前の問題">
                                "←"<span class="nav-label">" 前の問題"</span>
                            </button>
                            <button class="nav-btn" on:click=move |_| nav(1) title="次の問題">
                                <span class="nav-label">"次の問題 "</span>"→"
                            </button>
                        </div>
                        <button class="reset-btn" on:click=reset_code title="コードを初期状態に戻す">"リセット"</button>
                    </div>
                    <div class="editor-host" id="editor-host"></div>
                    <Splitter target=SplitTarget::Console sizes=layout/>
                    <ConsolePane
                        state=Signal::derive(move || run_state.get())
                        language=Signal::derive(move || {
                            // 表示中の問題の言語が正 (セレクタ操作の直後でも実行結果と食い違わない)
                            selected.with(|s| s.as_ref().map_or_else(|| language.get(), |p| p.language))
                        })
                        on_next=Callback::new(move |()| nav(1))
                    />
                </section>
            </div>
            <nav class="mobile-tabs" aria-label="表示の切替">
                {MobilePane::ALL
                    .iter()
                    .map(|mp| {
                        let mp = *mp;
                        let icon = match mp {
                            MobilePane::List => crate::ICON_LIST,
                            MobilePane::Problem => crate::ICON_DOC,
                            MobilePane::Code => crate::ICON_CODE,
                        };
                        view! {
                            <button
                                class="mobile-tab"
                                class:active=move || pane.get() == mp
                                // 見た目 (色と上線) だけでなく、支援技術にも
                                // 「いまどのペインか」を渡す
                                aria-current=move || (pane.get() == mp).then_some("page")
                                on:click=move |_| pane.set(mp)
                            >
                                <span class="mobile-tab-icon" inner_html=icon></span>
                                <span>{mp.label_ja()}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </nav>
        </div>
    }
}
