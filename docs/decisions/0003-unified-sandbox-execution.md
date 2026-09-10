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

- **レイテンシが悪化した**。Wandbox / Playground は 2〜5s、Sandbox 経由は **4.7〜7.1s**
  (下の「追記」の L1 で本番平均 4.8s まで戻した)。
  イメージがその region で初回のときは 25s 程度かかる実測あり。
  内訳 (2026-09-11 実測、`scripts/measure-sandbox-latency.py`): **コンパイル + 実行自体は
  0.7〜2.0s** しかなく、残りは サンドボックス作成 0.8s / 初回のウォームアップ 1〜2s /
  **停止待ち 1.6〜2.7s**。つまり削れる余地は大きい (停止を待たない → 再利用の順)
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
- Hobby の枠の消費は Vercel の Usage で見る。
- **レイテンシ改善**: 安い順に ① 応答前に停止を待つのをやめる (−1.6〜2.7s)
  ② サンドボックスの再利用 (0.7〜2.0s まで。作成枠の節約にもなる) ③ イメージ縮小。
  **① は実施済み (下の「追記」)。② は「1 提出 = 使い捨ての microVM」という隔離の
  根拠を変えるので、実装前にこの ADR への追記か新しい ADR で決める**。出発点は
  `docs/bootstrap/handoffs/2026-09-11-unify-sandbox-runtime.md`
- イメージは**個人アカウント (`rintaroyamaoka-3890` / `rintaro-yamaokas-projects`) の
  プロジェクト `100-cord-knocks`** に属する。`scripts/build-runner-image.sh` は
  `vercel whoami` と `VERCEL_TOKEN` の不在を確認してから動く (会社スコープでの誤爆防止)

---

## 追記 2026-09-11 — 応答で停止を待つのをやめた (L1)

**決定**: `api/execute.rs` は、コマンドの結果を受け取ったら**サンドボックスの停止を
投げるだけ投げて、完了を待たずに応答を返す** (`spawn_stop`)。

**根拠**: 停止は 1.6〜2.7 秒かかるのに、結果の詰め替えには 1 バイトも要らない。
提出 1 回の体感時間の 3〜4 割が、利用者にとって何も起きていない時間だった。

**隔離は変わらない**。ここが ② (再利用) との決定的な差で、いまも
**1 提出 = 使い捨ての microVM 1 台**である。変わったのは「誰が片づけを待つか」だけ:

- 停止は元から best effort (失敗しても利用者の結果には影響させない契約だった)
- 届かなくてもサンドボックスは自身の `timeout` (`SANDBOX_TIMEOUT_MS` = 60s) で必ず消える。
  残骸は残らない。この関係はテスト
  (`a_sandbox_outlives_a_lost_stop_by_at_most_its_own_timeout`) で固定した
- `persistent: false` なので、停止時の自動スナップショットも作られない
- 課金は provisioned memory の 1 分最低課金なので、数十秒早く止めても実質変わらない

**受け入れたリスク**: 「投げたが届いていない」区間ができた。Fluid Compute は応答後も
関数インスタンスを生かすので実際はほぼ届くが、保証はない。届かなかったぶんは上の
`timeout` で消える。**同時 10 台という Hobby の上限に対しては、最悪でも
「1 提出あたり最大 60 秒 生き残る」で見積もる。**

**あわせて**: HTTP クライアントをプロセスで 1 個にした (`OnceLock`)。毎回作り直すと
接続プールも作り直しになり、提出のたびに api.vercel.com への TLS 握手が要る。

### 効果 (本番の実測)

`cargo test -p rust-100-knocks-api --test production -- --ignored --nocapture`
を本番 URL に対して、デプロイの**前に 2 回 (14 検体) / 後に 3 回 (21 検体)** 回した。

| | 平均 | 中央値 | 最短 | 最長 |
|---|---|---|---|---|
| 前 (停止を待つ) | 6.89s | 6.37s | 3.23s | 13.22s |
| 後 (待たない) | **4.81s** | **4.89s** | 0.89s | 11.85s |

**平均 −2.08s (−30%)**。見込み (−1.6〜2.7s) の範囲に入っている。

**ばらつきは大きいまま**で、同じ言語でも 1〜12 秒に散る。これは停止待ちとは別の要因
(関数のコールドスタート、region でのイメージの温まり具合) で、L1 では触っていない。
**「速くなった」は 1 回の計測では言えない**ので、比べるときは必ず複数回まわして
平均と中央値で見ること。

**次にやるなら ② (再利用)**。ここから先は隔離の根拠が変わるので、この ADR とは別に決める。
