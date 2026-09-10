# 2026-09-11-latency-l1-stop-not-awaited: 停止待ちを外して本番平均 −30%、次は L2 (再利用、要 ADR)

セッション期間: `2026-09-11 02:5x` 〜 `2026-09-11 03:4x`
本 doc の目的: **次の Claude が cold restore して L2 (サンドボックス再利用) の ADR から着手できる状態**を残す。

---

## 1 行で言うと

前セッションの残課題 L1 (応答で停止を待つのをやめる) を実装・検証・デプロイした。
**本番の提出 1 回は 平均 6.89s → 4.81s (−30%)、中央値 6.37s → 4.89s。**
残課題は L2 (サンドボックス再利用) で、**隔離の根拠が変わるので実装前に ADR が要る**。

## やったこと

- `api/execute.rs`: 停止を `spawn_stop` で投げるだけにして、応答は待たない
- 同: HTTP クライアントをプロセスで 1 個 (`OnceLock`) にし、接続プールを提出間で使い回す
- 同: 上流のベース URL を `Upstream` に持たせ、**偽の上流を立てて挙動を検証できるようにした**
- `tests/production.rs` (新規): 本番 URL に 7 言語を投げて判定と秒数を測る。
  **レイテンシはデプロイの前後で同じ手順を回さないと比べられない**ので、
  scratchpad の使い捨てスクリプトではなくテストとして残した

## 残課題 — L2 (サンドボックス再利用)。**実装の前に ADR**

到達目標は「再利用」列の 0.7〜2.0 秒 (= コンパイル + 実行そのもの)。
Fluid Compute は関数インスタンスを再利用するので、インスタンス内 static に
session id を持てば使い回せる。要るもの:

- 生存確認と失効時の作り直し / `extend-timeout` の更新 / **45 分のセッション上限** / 同時 10 の枠
- **決めるべきこと (ADR の中身)**: いまは「1 提出 = 使い捨ての microVM」で、これが
  他人のコードを動かす隔離の根拠になっている (ADR 0003 の「結果として受け入れるリスク」)。
  再利用すると**同じ VM で別の提出が動く**ので、
  ケースごとの作業ディレクトリ分離 (いまも `mktemp -d`)、`/tmp` の残骸、
  プロセスの置き去り、**前の提出が書いたファイルを次の提出から読めてしまわないか**を
  誰がどう保証するかを先に決める。ADR 0003 への追記か新しい ADR で。

**vCPU を 2 に増やす案は実測して捨てた** (差はノイズ、遅くなる言語もある)。
`python3 scripts/measure-sandbox-latency.py 2` で再現できる。詳細は ADR 0003 の追記。

**「使う前に作っておくだけ (再利用はしない)」という中間案**もある。1 提出 = 使い捨ての
microVM は保てるので隔離は変わらないが、使われないまま `timeout` で消えるぶんだけ
Hobby の作成枠 (5,000 回/月) を余計に食う。枠切れ = 次サイクルまで実行停止なので、
体感と可用性のトレードオフとして ADR で決めること。

L3 (イメージ縮小 / 初回ウォームアップ) は L2 の後に必要性を再判断する。
**ばらつき (同じ言語で 1〜12 秒) は L1 では減っていない**。原因はコールドスタートと
region ごとのイメージの温まりで、そこは L3 の領域。

## バックグラウンドプロセス

なし (全て完了)。

## 触ったファイル (すべて commit・push 済み)

- `api/execute.rs` — `spawn_stop` / `Upstream` / `pending_stops` / 静的クライアント
- `tests/production.rs` — 本番の実測 (新規)
- `docs/decisions/0003-unified-sandbox-execution.md` — 「追記 2026-09-11 — 応答で停止を待つのをやめた (L1)」
- `docs/bootstrap/verification/latency-l1-stop-not-awaited.md` — 検証 8 行 (全 CLOSED) と残る穴
- `CLAUDE.md` — 「停止を `await` で待つ形に戻さない」を地雷に追加、本番実測のコマンドを追加

## 重要な memory / docs references

読む順:

1. `docs/decisions/0003-unified-sandbox-execution.md` の**末尾の追記** — L1 で何を変え、何を変えなかったか
2. `docs/bootstrap/verification/latency-l1-stop-not-awaited.md` — 検証の 8 行と、残る穴 (特に 1 番)
3. `CLAUDE.md` の「既知の地雷」 — Hobby の枠・`persistent:false`・停止を待たない
4. `scripts/measure-sandbox-latency.py` — 作成 / 実行 / 停止の内訳を測る (L2 の到達目標はこの「再利用」列)

## 検証手順

```bash
# 1. 単体 (ネットワーク不要) — 167 件
cargo test --workspace --exclude app && cargo test -p app

# 2. ローカル Docker で 7 言語 (要 knocks-runtime:local)
bash scripts/build-runner-image.sh
cargo test -p shared --test runner_docker -- --ignored --nocapture --test-threads=1

# 3. 実 Sandbox 経由で 7 言語 (要 OIDC トークン)
vercel link --yes --project 100-cord-knocks && vercel env pull --yes
set -a; . ./.env.local; set +a
cargo test -p rust-100-knocks-api --bin execute -- --ignored --nocapture --test-threads=1

# 4. 本番の実測 (デプロイの前後で複数回)
cargo test --release -p rust-100-knocks-api --test production -- --ignored --nocapture --test-threads=1
```

期待: 1 は 167 passed、2 は 5 件 ✓、3 は 5 件 ✓ (7 言語)、4 は 7/7 Passed で平均 5 秒前後。
**4 は 1 回では判断しない** (同じ言語でも 1〜12 秒に散る)。

## 次セッションへの起動文 (= コピペ用)

```
docs/bootstrap/handoffs/2026-09-11-latency-l1-stop-not-awaited.md を読んで状況把握してから、
残課題の L2 (サンドボックス再利用) を進めて。隔離の根拠が変わるので、まず ADR
(0003 への追記か新規) で「同じ VM で別の提出が動くときに何をどう保証するか」を決めてから実装すること。
```
