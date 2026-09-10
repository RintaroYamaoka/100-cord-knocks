# 0003 — 7 言語の実行を Vercel Sandbox の自前イメージに統一する

- **Status**: Accepted
- **Date**: 2026-09-11
- **Deciders**: RintaroYamaoka (本人決定) / Claude
- **References**: [ADR 0002](0002-multi-language-execution-backends.md) を改訂 /
  `docs/bootstrap/incidents/2026-09-07-wandbox-outage-shown-as-502.md` /
  `docs/bootstrap/verification/feat-unify-sandbox-runtime.md`

---

## Context (背景)

ADR 0002 は「Rust は play.rust-lang.org、他 6 言語は wandbox.org」と決め、**非公式・ボランティア
運用のサービスに依存するリスクを明示的に受け入れていた**。そのリスクが現実化した。

- **2026-09-07、Wandbox が全コンパイラで HTTP 500 (`Failed to get uid`) を返し始め、
  9-11 時点でも復旧していない**。Rust 以外の 6 言語 (= 問題の 6/7、1800 問) が 4 日以上実行不能
- 障害はこちら側では直せない。プロキシに再試行と 503 の文言を入れた (commit `ba17246`) が、
  それは**見え方の改善であって可用性の回復ではない**
- 退避先として ADR 0002 が挙げていた候補も塞がっている: Piston 公開 API は
  2026-02-15 からホワイトリスト制、セルフホストは常駐ホストが必要で Vercel に置けない
- 同時に、**本番 (Wandbox) と検証 (ローカル Docker) が別環境・別コマンド定義**だったことに
  由来する事故が既に起きていた (2026-08-29: tsc の `--target` を検証側だけに渡していて、
  ES2019+ の API を使う模範解答が「verifier は緑なのに本番では TS2550」になった)

## Decision (決定)

**7 言語すべてを、Vercel Sandbox 上の自前イメージ 1 枚 (VCR `knocks-runtime`) で実行する。**
Wandbox と play.rust-lang.org への依存を削除し、**verifier も同じイメージで検証する**。

- **Rust も Playground から移す**。「1 言語だけ別基盤」を残すと、判定経路とツールチェーン版が
  二重管理になり、統一で得た性質 (検証 = 本番) が Rust だけ成り立たなくなる
- **実行コマンドの定義は `shared::runner` に 1 箇所だけ置く**。本番 (Sandbox REST) と
  検証 (ローカル `docker run`) は同じ `run_plan` からスクリプトを組む
- **ツールチェーンの版は ADR 0002 の検証済みの組み合わせを完全に踏襲する**
  (g++ 13.4.0 / javac 22.0.2 / dotnet SDK 6.0.428 / CPython 3.13.15 / Node 20.20.2 /
  tsc 5.6.2 / rustc stable)。今回の変更で**問題コンテンツ 2100 問には手を入れない**ため、
  版を同時に動かすと回帰の切り分けができなくなる
- 採用しなかった代替案:
  - **Wandbox の復旧を待つ**: 復旧見込みが不明で、4 日間 6 言語が停止した事実がある
  - **Judge0 CE 公開インスタンス** (ADR 0002 のフォールバック): 非公式依存を別の
    非公式依存に替えるだけで、検証と本番の乖離も解けない
  - **Rust だけ Playground に残す**: Hobby の枠切れに対する可用性の保険にはなるが、
    判定経路が 2 系統に戻る。本人の判断で「システム的に統一」を採った
  - **公式 SDK (`@vercel/sandbox`) を使うために関数を TypeScript で書き直す**:
    判定ロジック (`shared`) を TS に再実装することになり、契約の二重管理が発生する。
    REST API が公開されている (OpenAPI 掲載) ので Rust の関数から直接叩く

### 実行の形 (2026-09-11 実測)

| 操作 | エンドポイント | 実測 |
|---|---|---|
| 作成 | `POST /v4/sandboxes` | 0.78s (4.5GB のカスタムイメージでも) |
| 実行 | `POST /v2/sandboxes/sessions/{id}/cmd?cmdId=…` (`wait=true, logs=true`) | 言語により 2〜8s |
| 停止 | `POST /v2/sandboxes/sessions/{id}/stop` (Content-Type 必須) | — |

- 認証は **OIDC (`VERCEL_OIDC_TOKEN`)**。本番の Rust 関数にも注入される。
  `projectId` は送らない (トークンがプロジェクトに紐づくため上流が解決する)
- **`persistent: false` を必ず指定する**。既定 (true) では停止のたびに自動スナップショットが
  作られ、Hobby の生涯 15GB のスナップショット枠を数回で使い切る
- **`networkPolicy: deny-all`**。他人のコードを動かす責務がこちら側に移ったため。
  Rust / C# のひな形プロジェクトは restore 済みでイメージに焼き、実行時の通信を不要にしてある
- 提出コードは **base64 で埋め込む**。シェルに直接展開すると提出側がスクリプトを壊せる
- 出力は **実行ごとに変わる nonce で区切った 5 セクション** (コンパイル stdout/stderr、
  プログラム stdout/stderr、終了コード) として 1 回の cmd で回収する。
  パーサは**後勝ち**で読む (本物のセクションは必ずプログラム出力より後に出るため、
  提出コードが偽の `exit` を印字しても乗っ取れない)

## Consequences (結果)

### 良い影響

- **7 言語すべての可用性が自前の管理下に入った**。非公式サービスの障害で止まらない
- **検証 = 本番**。verifier と本番が同じ Dockerfile 由来のイメージ・同じコマンドを使うので、
  「verifier は緑なのに本番は落ちる」クラスの事故が構造的に消える
- 判定経路が 1 本になった。`Backend` enum、Wandbox / Playground の応答型と詰め替えが
  すべて削除され、`shared::runner` + `shared::sandbox` に置き換わった
- C# の Wandbox 由来の制約が消えた (`dotnet new` の `File size limit exceeded` で
  6.0.425 固定だった問題、診断に出るファイル名)。診断は `prog.cs(2,67): error CS0029` の形に戻した

### 悪い影響 / トレードオフ

- **レイテンシが悪化した**。Wandbox / Playground は 2〜5s、Sandbox 経由は **4〜10s**
  (サンドボックス作成 + コンパイル)。イメージがその region で初回のときは 25s 程度かかる実測あり
- **Hobby の枠が新しい上限になった**: 作成 5,000 回/月、Active CPU 5 時間/月、同時 10。
  超過しても課金はされないが**次サイクルまでサンドボックス作成が止まる**。
  その状態は専用の文言 (`QUOTA_EXHAUSTED`) で「コードの問題ではない」と伝える
- イメージ (圧縮後 ~1.5GB) の保守が増えた。VCR の保存は $0.10/GB・月
- 提出コードは microVM 内で root として動く (ネットワーク無し・単発・Firecracker 隔離)。
  より強い隔離が要るなら非特権ユーザーでの実行を足す

### 移行後に必要な保守

- ツールチェーンの版を上げるときは **`cargo run -p verifier -- --expect 2100` を通してから**。
  イメージのタグは `shared::runner::PRODUCTION_IMAGE` が正本で、`scripts/build-runner-image.sh --push <tag>`
  で上げる。本番は環境変数 `KNOCKS_SANDBOX_IMAGE` で切り戻せる
- Hobby の枠の消費は Vercel の Usage で見る。手狭になったら
  **サンドボックスの再利用 (pooling)** が最初の最適化 (作成回数が枠の律速なので効果が大きい)
- イメージは**個人アカウント (`rintaroyamaoka-3890` / `rintaro-yamaokas-projects`) の
  プロジェクト `100-cord-knocks`** に属する。`scripts/build-runner-image.sh` は
  `vercel whoami` と `VERCEL_TOKEN` の不在を確認してから動く (会社スコープでの誤爆防止)
