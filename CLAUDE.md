# CLAUDE.md

## プロジェクト概要

コーディング練習アプリ「100本ノック」。ブラウザエディタで問題を解き、実物のコンパイラ/ランタイムの
エラー・テスト結果を返す。**Rust / C++ / C# / Java / Python / TypeScript / JavaScript の 7 言語**、
各言語 初級/中級/上級 各100問 (計2100問)、回答例と解説つき。
Vercel にデプロイ(静的 WASM フロント + Rust Functions)。

- 本番: https://100-cord-knocks.vercel.app
- リポジトリ: `RintaroYamaoka/100-cord-knocks` (旧 `rust-100-knocks` から改称。
  docs/bootstrap 配下の古い記録に出てくる `rust-100-knocks.vercel.app` は現在 404)

## 技術スタック

- 言語: アプリ本体は Rust。フロントは Leptos (CSR) を wasm32-unknown-unknown + Trunk でビルド
- エディタ: CodeMirror 6(`assets/js/editor.js` は esbuild 生成物、ソースは `editor-src.mjs`)。
  言語モードは `window.RustKnocksEditor.setLanguage(slug)` で切り替える
- バックエンド: Vercel **公式** Rust ランタイム (`vercel_runtime = "2"`, hyper 1 ベース)。
  `api/execute.rs` が **Rust → play.rust-lang.org / 他 6 言語 → wandbox.org** に振り分ける。
  `vercel.json` に `functions.runtime` は書かない (Cargo.toml の `[[bin]]` を自動検出)
- 問題データ: `data/problems/<言語>/<難易度>.json`(静的配信)。スキーマは `crates/shared`
- 進捗保存: ブラウザ localStorage(サーバー状態なし)。キーは `<言語>/<問題id>`
- テスト: cargo test(workspace)。問題コンテンツの品質検証は `crates/verifier`

## 開発コマンド

```bash
# テスト実行
cargo test --workspace --exclude app   # 契約層・プロキシ・verifier
cargo test -p app                      # フロントの純ロジック (host でコンパイルできる範囲)

# 問題コンテンツ検証 (実コンパイラを Docker で回す)
cargo run -p verifier                            # 全 2100 問
cargo run -p verifier -- --lang cpp              # 1 言語
cargo run -p verifier -- --expect 2100           # 件数まで含めて検算

# 開発サーバー
vercel dev             # :3000 で /api/execute
trunk serve            # :8080 でフロント (API は vercel dev へプロキシ)
```

## ディレクトリマップ

```
.
├── crates/shared/    # 言語定義・問題スキーマ・API契約・進捗モデル (front/back/verifier 共有)
├── crates/app/       # Leptos フロントエンド (wasm32)
├── crates/verifier/  # 問題品質検証ハーネス (docker.rs が docker run を組み立てる唯一の場所)
├── api/              # Vercel Rust Functions (execute.rs)
├── data/problems/    # 問題データ JSON (<言語>/<難易度>.json の 21 ファイル)
├── assets/           # CSS / JS glue / CodeMirror バンドル
└── docs/             # ADR + bootstrap 規律 (handoffs/incidents/sprint/verification/commission)
                      # problem-authoring.md = 問題を書くときの全言語共通契約
```

## アーキテクチャ概略

- 依存方向: `app` / `api` / `verifier` → `shared`。逆依存禁止。API 契約 (`ExecuteRequest/Response`)、
  問題スキーマ (`Problem`)、言語定義 (`Language`) の変更は必ず `shared` で行う
- 正誤判定はサーバー側に状態を持たない: ユーザーコード + `hidden_tests` を結合して実行し、
  **終了コードと stdout の目印**で分類する。判定順序と言語別の制約は ADR 0002 が正本
- `crates/app` は wasm32 専用ターゲット。純ロジック(フィルタ・進捗集計・出力パース)は
  host でもテストできるよう UI から分離して書く
- 画面は同じ DOM のまま、**幅で 2 つのレイアウトを切り替える**: 広い画面は 3 ペイン
  (サイドバー | 問題 | ワークベンチ、ドラッグで分割)、スマホ幅は 1 画面 1 ペイン +
  下部タブ。どちらを出すかは CSS だけが決め、Rust 側は「いまどのペインか」
  (`crates/app/src/mobile.rs`) しか持たない

## 既知の地雷

- **`Outcome::Passed` の必要条件に `test result: ok` が要る**。終了コードだけで正解にすると、
  ユーザーコードが判定テストより先に `exit(0)` するだけで「正解」になる (テストが 1 件も走らない)
- **`compose_submission` には必ず `Language` を渡す**。区切りコメントを `//` 固定にすると
  Python の問題が実行前に SyntaxError で全滅する
- **進捗は必ず `progress_key` / `&Problem` を取る関数を通す**。素の `p.id` で引いても型は通り、
  症状は「一覧の進捗色が静かに全部消える」だけなので気づけない
- **Java の問題でクラスを `public` にしない**。Wandbox のファイル名が `prog.java` 固定のため
  `class X is public, should be declared in a file named X.java` で落ちる
- **C# は `dotnetcore-6.0.425`**。`dotnetcore-8.0.402` は Wandbox 側で `dotnet new` が
  `File size limit exceeded` で落ちる。成功時も MSBuild の定型出力が出るのでプロキシで除去している
- **Wandbox は既定 User-Agent を 403 で弾く**。過負荷時は `OCI runtime error` を返すので、
  コンパイルエラーとして見せずに再試行 → 503 で返す
- **Wandbox 全体が落ちることがある** (2026-09-07: 全コンパイラで HTTP 500
  `Failed to get uid: status=exit status: 125`)。Rust 以外の 6 言語が一斉に失敗したら、
  まず `curl -A x -X POST https://wandbox.org/api/compile.json` で上流に直接投げて切り分ける。
  プロキシは 5xx を再試行して 503「Wandbox が一時的に停止しています」で返す。
  復旧すれば再デプロイ不要。詳細: `docs/bootstrap/incidents/2026-09-07-wandbox-outage-shown-as-502.md`
- Playground / Wandbox は非公式・レート制限あり。問題コンテンツの検証は必ず `verifier`
  (ローカル Docker) で行い、上流に一括負荷をかけない
- **verifier の検査を弱めない**。通らない問題は検査を消すのではなく問題を作り直す
- Vercel ビルドで `cargo install trunk` は遅すぎる。`scripts/build-frontend.sh` は
  prebuilt バイナリをダウンロードする
- リポジトリ直下に `build.sh` を置いてはならない: Rust ランタイムが関数ビルド前フックとして
  自動実行してしまう (2026-08-25 のデプロイ失敗の原因)
- `vercel-rust` (vercel-community/rust, `vercel_runtime 1.x`) は 2026-01 アーカイブ済み。
  使うと本番で `FUNCTION_INVOCATION_FAILED`。詳細:
  `docs/bootstrap/incidents/2026-08-25-vercel-rust-deploy-failures.md`
- Vercel build image は glibc が古い。prebuilt バイナリ (trunk 等) は musl 版を使う
- `crates/app` を `cargo test --workspace` に含めると wasm 前提コードが host ビルドで壊れることがある。
  テストは `--exclude app` で回し、app の純ロジックは shared 側に置く
- WSL で Playwright を使うときは `libnspr4` 等が不足する。sudo 不要の回避策は
  `apt-get download libnspr4 libnss3 libasound2t64` → `dpkg-deb -x` → `LD_LIBRARY_PATH`
- **起動時に N 本のリクエストを逐次投げない**。言語一覧を 21 本の HEAD で決めていたとき、
  遅い回線で 16 秒間 Rust しか選べなかった。収録済み言語は
  `data/problems/index.json` (ビルド時に `scripts/gen-manifest.mjs` が生成) を 1 本読む
- **スマホ幅 (≤820px、および横向きスマホ) は「1 画面 1 ペイン + 下部タブ」**。
  どのペインを見せるかは `.main[data-pane=list|problem|code]` (CSS) が決め、状態は
  `crates/app/src/mobile.rs`。3 ペインの縦積みに戻すと、375px ではヘッダーが横に溢れて
  「上級」タブと進捗が画面外に出るうえ、エディタまで 2〜3 画面スクロールすることになる
- **Leptos: 1 つのクロージャで Memo とその材料を読むときは、材料を先に読む**。
  `format!("{} / {}", passed_in_level.get(), problems.with(|p| p.len()))` と書くと、
  Memo の初回計算の中で `problems` が読まれるせいで外側の購読が張られず、表示が初期値の
  まま固まる (ヘッダーの進捗が本番でずっと `0 / 0` だった)。詳細:
  `docs/bootstrap/incidents/2026-08-30-header-progress-frozen.md`
- **スマホのエディタ折り返しは CSS だけで実現している**。`.cm-content` に
  `white-space: pre-wrap` を当てるだけでは効かず、`min-width: 0 !important` と
  `flex-shrink: 1` が要る (CodeMirror は非折り返し時、最長行ぶんの min-width を
  inline style で付け、content を `flex-shrink: 0` で置く)。1 つでも欠けると横スクロールに戻る
- **入力要素のフォントをスマホ幅で 16px 未満にしない**。iOS がフォーカス時に画面を
  勝手に拡大し、戻すのはピンチ操作しかない。`.search-input` / `.lang-select` /
  `.cm-content` はスマホ幅で 16px に上げてある
- **ブラウザ検証で `waitForTimeout(N)` を使わない**。待ち時間を入れると、それより遅い
  問題が永久に見えなくなる (上記の 16 秒はこれで隠れていた)。
  「揃うまでの時間」を測って上限で判定し、回線を絞った計測も 1 本入れる
  (詳細: `docs/bootstrap/incidents/2026-08-30-language-selector-slow-network.md`)
