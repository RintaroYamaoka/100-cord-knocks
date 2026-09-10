# 引き継ぎ: 7 言語の実行を Vercel Sandbox に統一 (2026-09-11)

賞味期限 1〜2 週間。永続記録は ADR 0003 / incidents / CLAUDE.md にある。

## いま何が本番で動いているか

- **7 言語すべて Vercel Sandbox の自前イメージで実行**。本番で 7/7 実測済み
  (正解 → `test result: ok`、壊したコード → 言語固有の診断。レイテンシ 4〜8 秒)
- イメージ: `vcr.vercel.com/rintaro-yamaokas-projects/100-cord-knocks/knocks-runtime:2026-09-11`
  (digest `sha256:b0606a22…`)。タグの正本は `shared::runner::PRODUCTION_IMAGE`
- ブランチ `feat/unify-sandbox-runtime` の 2 commit (`0fb92ff` 統一本体 / `27b0b23` OIDC ヘッダ修正)。
  **本番には CLI で直接デプロイ済み**。main へのマージは未実施

## 触った場所 (地図)

| 場所 | 何をした |
|---|---|
| `crates/shared/src/runner.rs` | **新規・正本**。言語別の実行コマンド、nonce 区切りのスクリプト生成、出力パース、`ExecuteResponse` への詰め替え、イメージタグ定数 |
| `crates/shared/src/sandbox.rs` | **新規**。Sandbox REST の型、ndjson の畳み込み、失敗分類、トークンの優先順 (`pick_token`) |
| `crates/shared/src/fixtures.rs` | **新規**。7 言語の見本コード (本番疎通テストとローカル Docker スモークが共有) |
| `crates/shared/src/contract.rs` | `playground.rs` から改名。Wandbox / Playground の型と詰め替えを削除 |
| `crates/shared/src/language.rs` | `Backend` enum と `verify_image()` を削除 (イメージは 1 枚) |
| `api/execute.rs` | Sandbox REST を直接駆動。`dispatch(req, header_token)` にシグネチャ変更 |
| `crates/verifier/src/docker.rs` | コマンドを `shared::runner::run_plan` から取る。7 言語とも同じイメージ |
| `crates/verifier/src/{lib,main}.rs` | Rust だけローカル cargo だった経路 (`run_problem_rust`) を削除 |
| `docker/runner/Dockerfile` | **新規**。7 言語のツールチェーン 1 枚 |
| `scripts/build-runner-image.sh` | **新規**。個人アカウントのガード付きビルド / push |

## 次にやるなら

1. **`cargo run -p verifier -- --expect 2100` の結果を確認して記録を閉じる**
   (この session では 1700 問まで ✗0 を確認した時点で引き継ぎを書いている。
   `docs/bootstrap/verification/feat-unify-sandbox-runtime.md` の V11 が OPEN)
2. **main へマージ** (V11 が CLOSED になってから。verification gate がそれを要求する)
3. **レイテンシ改善**: 提出ごとに毎回サンドボックスを作っているので 4〜8 秒かかる。
   サンドボックスの再利用 (pooling) が最初の手で、Hobby の作成回数の枠 (5,000/月) にも効く。
   ただし「1 提出 = 捨てる microVM」という隔離の性質が変わるので、設計判断として扱うこと
4. **ツールチェーンの版上げ**は別件に切る。いまは ADR 0002 で検証済みの版を踏襲している
   (dotnet 6.0 は EOL、temurin 22 も非 LTS)。上げるなら `verifier -- --expect 2100` を通してから

## 落とし穴 (この session で踏んだもの)

- **OIDC トークンは関数にはヘッダ `x-vercel-oidc-token` で来る。env ではない。**
  ローカルは `vercel env pull` の env で通るので、この差は**本番に出すまで分からない**
- CLI 54 系に `vercel vcr` が無い。`docker login vcr.vercel.com -u oidc` で代替した
  (グローバル CLI の更新は副作用があるので避けた。更新するなら deploy の前後で確認を)
- ADR 番号を 0008 と書き始めてしまった。**このリポジトリの次番は 0003**
  (0004 以降は project-bootstrap plugin 側の ADR で、repo のものではない)
