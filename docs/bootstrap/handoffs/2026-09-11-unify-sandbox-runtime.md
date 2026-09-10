# 2026-09-11-unify-sandbox-runtime: 7 言語の実行を Vercel Sandbox に統一し、次はレイテンシ改善

セッション期間: `2026-09-11 00:50` 〜 `2026-09-11 03:4x`
本 doc の目的: **次の Claude が cold restore して「レイテンシ改善」から着手できる状態**を残す。

---

## 1 行で言うと

Wandbox の 4 日超の全体障害を受けて **7 言語すべての実行を Vercel Sandbox の自前イメージに統一**
(ADR 0003)、本番で 7/7 疎通・2100 問の再検証 0 件・main にマージ & push 済み。
**残課題は 1 件: 提出 1 回のレイテンシが 4.7〜7.1 秒**(旧 Wandbox 経由は 2〜5 秒)。

## 残課題 — レイテンシ改善 (次にやること)

### 内訳の実測 (2026-09-11、`scripts/measure-sandbox-latency.py`)

秒。「毎回作り直し」= 作成 + 1 回目 + 停止 (= いまの本番)、「再利用」= 2 回目以降の実測平均。

| 言語 | 作成 | 1 回目 | 2 回目 | 3 回目 | 停止 | 毎回作り直し | 再利用 |
|---|---|---|---|---|---|---|---|
| python | 0.77 | 3.00 | 0.69 | 0.69 | 2.68 | **6.45** | **0.69** |
| javascript | 1.13 | 1.16 | 0.67 | 0.67 | 2.62 | 4.91 | 0.67 |
| cpp | 0.72 | 2.39 | 0.87 | 0.87 | 1.62 | 4.74 | 0.87 |
| java | 0.92 | 3.38 | 1.41 | 1.43 | 1.87 | 6.16 | 1.42 |
| csharp | 0.87 | 3.54 | 2.57 | 1.50 | 2.71 | **7.12** | 2.04 |
| rust | 0.85 | 1.34 | 0.81 | 0.72 | 2.63 | 4.82 | 0.77 |

読み取れること: **コンパイル + 実行そのものは 0.7〜2.0 秒しかかかっていない**。
残りはすべて周辺コスト (作成 0.8 秒 / 初回のウォームアップ 1〜2 秒 / 停止 1.6〜2.7 秒)。

### 安い順に 3 段

| # | 何をする | 見込み | 対応案 |
|---|---|---|---|
| L1 | **停止を待つのをやめる** | **−1.6〜2.7 秒** (ほぼ全言語) | `api/execute.rs:run_in_sandbox` が `stop_session().await` を**応答前に**待っている。結果の詰め替えには不要。待たずに投げる (Vercel Functions は応答後も短時間生きるが保証は無いので、`timeout` が保険。`persistent:false` なのでスナップショットも作られない)。課金は provisioned memory が 1 分最低課金なので実質変わらない |
| L2 | **サンドボックスを再利用する (pooling)** | 残りを削って **0.7〜2.0 秒** | Fluid Compute は関数インスタンスを再利用するので、インスタンス内 static に session id を保持して使い回せる。要るもの: 生存確認と失効時の作り直し、`extend-timeout` の更新、**45 分のセッション上限**、同時 10 の枠。Hobby の作成枠 (5,000/月) も節約できる |
| L3 | イメージを小さくする / 初回ウォームアップ対策 | 1 回目の 1〜2 秒 | 3.23GB のイメージの page cache が冷たいぶん。言語別にイメージを割ると「1 枚で統一」を崩すので、**L1/L2 をやってから必要性を再判断**する |

**L2 は設計判断を含む**: いまは「1 提出 = 使い捨ての microVM」で、これが他人のコードを動かす
隔離の根拠になっている (ADR 0003 の「結果として受け入れるリスク」)。再利用すると
**同じ VM で別の提出が動く**ので、ケースごとの作業ディレクトリ分離 (いまも `mktemp -d`)、
`/tmp` の残骸、プロセスの置き去りを誰が掃除するかを決める必要がある。
ADR 0003 への追記か、新しい ADR で決めてから実装すること。

## バックグラウンドプロセス

なし (全て完了)。このセッションで走らせたもの:

- `cargo run -p verifier -- --expect 2100` — 完了 (2100 問 / 問題あり 0 件 / コンテナ 21 回)
- `docker push …/knocks-runtime:2026-09-11` — 完了 (digest `sha256:b0606a22…`)

## 触ったファイル

### 永続化したい (すべて commit・push 済み)

main の `0fb92ff` → `8b4bdff` の 5 commit に入っている。

- `crates/shared/src/runner.rs` — **実行コマンドの正本**。スクリプト生成・出力パース・
  詰め替え・イメージタグ (`PRODUCTION_IMAGE` / `LOCAL_IMAGE`)・実行先の表示名 (`BACKEND_LABEL`)
- `crates/shared/src/sandbox.rs` — Sandbox REST の型、ndjson の畳み込み、失敗分類
  (`Transient` / `QuotaExhausted` / `Unauthorized` / `Other`)、トークンの優先順 (`pick_token`)
- `crates/shared/src/fixtures.rs` — 7 言語の見本コード (本番疎通テストとローカルスモークが共有)
- `crates/shared/src/contract.rs` — 旧 `playground.rs`。Wandbox / Playground の型を削除
- `api/execute.rs` — Sandbox REST を直接駆動。`dispatch(req, header_token)`。文言は `mod messages` が検査
- `crates/verifier/src/docker.rs` — コマンドは `shared::runner::run_plan` から取る。7 言語とも同一イメージ
- `docker/runner/Dockerfile` — 7 言語のツールチェーン 1 枚
- `scripts/build-runner-image.sh` — 個人アカウントのガード付きビルド / push
- `scripts/measure-sandbox-latency.py` — **上の表を再現するスクリプト** (残課題の出発点)

### untracked / ephemeral

- `.env.local` — `vercel env pull` が置いた開発用 OIDC トークン (12 時間で失効、gitignore 済み)

## 重要な memory / docs references

読む順:

1. `docs/decisions/0003-unified-sandbox-execution.md` — 実行基盤の決定と受け入れたトレードオフ
2. `docs/bootstrap/verification/feat-unify-sandbox-runtime.md` — 検証 16 行 (全 CLOSED) と残る穴
3. `docs/bootstrap/incidents/2026-09-11-oidc-token-is-a-header-not-an-env-var.md` —
   OIDC は関数にはヘッダで来る (ローカル検証では見つからない差)
4. `CLAUDE.md` の「既知の地雷」 — Hobby の枠・`persistent:false`・イメージ push の手順

## 検証手順

```bash
# 1. 単体 (ネットワーク不要)
cargo test --workspace --exclude app && cargo test -p app

# 2. ローカル Docker で 7 言語 (要 knocks-runtime:local)
bash scripts/build-runner-image.sh
cargo test -p shared --test runner_docker -- --ignored --nocapture --test-threads=1

# 3. 実 Sandbox 経由で 7 言語 (要 OIDC トークン)
vercel link --yes --project 100-cord-knocks && vercel env pull --yes
set -a; . ./.env.local; set +a
cargo test -p rust-100-knocks-api -- --ignored --nocapture --test-threads=1

# 4. レイテンシの内訳
python3 scripts/measure-sandbox-latency.py
```

期待: 1 は 226 passed、2 は 5 件すべて ✓、3 は 7 言語で Passed / CompileError、
4 は上の表と同程度 (「再利用」列が L2 の到達目標)。

## 次セッションへの起動文 (= コピペ用)

```
docs/bootstrap/handoffs/2026-09-11-unify-sandbox-runtime.md を読んで状況把握してから、
残課題の L1 (停止を待つのをやめる) から作業を続けて。L2 (サンドボックス再利用) は
隔離の性質が変わるので、実装の前に ADR で決めること。
```
