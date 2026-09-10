# 動作検証: feat/unify-sandbox-runtime (ADR 0003)

7 言語の実行を Vercel Sandbox の自前イメージに統一する変更の動作テスト。
OPEN が 1 行でも残っている間は統合しない。

最終更新: 2026-09-11 (全行 CLOSED。本番デプロイと 2100 問の再検証まで完了)

## 継ぎ目 (どこを跨いだか)

テストは実装からではなく、この変更が跨いだ境界から導いた。

1. **上流 API の契約** (Vercel Sandbox REST) — 記憶ではなく実測で形を固定する
2. **実行環境** (自前イメージ 4.5GB のツールチェーン) — 版が検証済みの組み合わせと一致するか
3. **判定契約** (ADR 0002 の `Outcome` 5 分類) — 基盤を替えても分類が変わらないか
4. **問題コンテンツ 2100 問** — 基盤を替えて落ちる問題が無いか (コンテンツは無変更)
5. **本番環境** (Vercel Rust 関数 + OIDC) — ローカルで通っても本番で通るとは限らない
6. **可視オラクルを騙せるか** (提出コードが判定を偽装できないか)

| # | 条件 | 状態 | 証拠 |
|---|---|---|---|
| V1 | Sandbox REST が OIDC トークンで叩ける (作成 / 実行 / 停止) | **CLOSED** | 実測 2026-09-11: create `POST /v4/sandboxes` 200 / 0.78s、cmd 200 (ndjson で stdout・stderr 分離 + exitCode)、stop 200 (Content-Type 無しは 415)。`projectId` 省略でも OIDC がプロジェクトを解決する |
| V2 | 応答形のパースが実測値で固定されている | **CLOSED** | `crates/shared/tests/sandbox.rs` 11 件。固定値は実 API の応答そのまま (`REAL_CREATE_RESPONSE` / `REAL_CMD_STREAM`) |
| V3 | イメージのツールチェーン版が ADR 0002 の検証済みの組み合わせと一致 | **CLOSED** | `/opt/knocks/VERSIONS`: g++ 13.4.0 / javac 22.0.2 / dotnet 6.0.428 / Python 3.13.15 / node v20.20.2 / tsc 5.6.2 / rustc 1.98.1。dotnet は ADR 0002 の実測値 6.0.428 と同一 |
| V4 | ローカル Docker で 7 言語が 正解→Passed / 未実装→Passed でない / 壊れたコード→CompileError | **CLOSED** | `cargo test -p shared --test runner_docker -- --ignored` 5 件 (21 ケース)。本番と同じスクリプト・同じイメージを使う |
| V5 | 実 Sandbox 経由で 7 言語が同じ判定になる (= 配信される `dispatch` をそのまま実行) | **CLOSED** | `cargo test -p rust-100-knocks-api -- --ignored` 5 件。7/7 言語で Passed / starter 不正解 / 言語固有の診断 (`error[E0308]` `error CS0029` `error TS2322` `SyntaxError` …) |
| V6 | 無限ループが上限で殺され、Passed にならない | **CLOSED** | `an_infinite_loop_is_killed_by_the_timeout` (実コンテナ、20 秒で SIGKILL → RuntimeError) |
| V7 | 判定テストより先に exit(0) する提出が正解にならない | **CLOSED** | ローカル実コンテナと実 Sandbox の両方で `NoTestsRun` (`a_submission_that_exits_before_the_tests_is_not_passed` × 2 経路) |
| V8 | **提出コードが判定を偽装できない** (held-out oracle) | **CLOSED** | ① 区切りは実行ごとの nonce (getrandom) ② パーサは後勝ちで読む (`forged_sections_inside_program_output_do_not_win`) ③ コードは base64 で埋め、シェルに展開しない (`submitted_code_is_never_interpolated_into_the_shell`) |
| V9 | 本番と検証でコマンド定義が 1 箇所 | **CLOSED** | `script_commands_come_from_the_shared_run_plan` (7 言語で verifier のスクリプトが `shared::runner::run_plan` の文字列を含む)。verifier 側の言語別コマンドは削除済み |
| V10 | C# の診断が読める形 (一時パスと csproj 接尾辞が無い) | **CLOSED** | 実測で `prog.cs(2,67): error CS0029: Cannot implicitly convert type 'int' to 'string'`。`-p:GenerateFullPaths=false` + `strip_msbuild_project_suffix`。`csharp_response_carries_no_build_noise` が `.csproj]` も検査 |
| V11 | 全 2100 問で answer 通過 / starter 失敗 (新イメージ) | **CLOSED** | `cargo run -p verifier -- --expect 2100` → **「検証 2100 問 / 問題あり 0 件 / コンテナ起動 21 回」**、終了コード 0。問題コンテンツは 1 文字も変えずに実行基盤を差し替えて全問通過 |
| V12 | Wandbox / Playground への依存がコードから消えている | **CLOSED** | `grep -rn 'wandbox\|play.rust-lang' --include='*.rs'` が 0 件 (コメントの経緯説明のみ)。`Backend` enum、応答型、詰め替え関数を削除 |
| V13 | 本番デプロイで 7 言語が実行できる (OIDC が Rust 関数に届く) | **CLOSED (1 回失敗 → 修正後 7/7)** | 初回デプロイは全言語 500「認証情報がありません」。原因は **OIDC トークンが env ではなくリクエストヘッダ `x-vercel-oidc-token` で来る**こと (`docs/bootstrap/incidents/2026-09-11-oidc-token-is-a-header-not-an-env-var.md`)。ヘッダ優先で読むよう修正し再デプロイ → 下記「本番の実測」で 7/7 |
| V14 | 枠切れ・一時障害の文言が「コードの問題ではない」と伝える | **CLOSED (一部は実測不能)** | 分類は `classify_sandbox_failure` の 6 件のテストで固定。**枠切れの実応答は枠を使い切るまで観測できない**ため、402 と `quota` / `resource_limit` / `exceeded your` の語で判定し、外れたら一般エラーに落ちる (黙って再試行はしない) |
| V15 | フロントの文言が実行基盤と一致 | **CLOSED** | `backend_label` は 7 言語とも "Vercel Sandbox"。`crates/app/tests/lang.rs` 2 件 |

## 本番の実測 (V13)

2026-09-11、本番 `POST https://100-cord-knocks.vercel.app/api/execute` に投入した結果。
正解コードと「わざと壊したコード」を各言語 1 本ずつ (スクリプトは session の scratchpad)。

| 言語 | 正解 | 壊したコード (実診断) | 秒 (正解/壊れ) |
|---|---|---|---|
| rust | ✓ Passed | `error[E0308]: mismatched types` | 5.0 / 3.9 |
| cpp | ✓ Passed | `prog.cc:1:33: error: invalid conversion from 'const char*' to 'int'` | 8.3 / 3.7 |
| csharp | ✓ Passed | `prog.cs(2,67): error CS0029: Cannot implicitly convert type 'int' to 'string'` | 7.3 / 6.5 |
| java | ✓ Passed | `prog.java:1: error: incompatible types: int cannot be converted to String` | 4.2 / 4.5 |
| python | ✓ Passed | `SyntaxError: expected ':'` | 5.5 / 7.8 |
| typescript | ✓ Passed | `prog.ts(1,52): error TS2322: Type 'number' is not assignable to type 'string'` | 6.1 / 6.0 |
| javascript | ✓ Passed | `SyntaxError: Unexpected token ';'` | 4.9 / 5.7 |

レイテンシは **4〜8 秒** (Wandbox / Playground 時代は 2〜5 秒)。

## 残る穴 (正直な記録)

1. **レイテンシが悪化した (受け入れた劣化)**
   Wandbox / Playground 経由は 2〜5s、Sandbox 経由は 4〜10s。さらに、そのリージョンで
   イメージが初回のときは 25s 程度かかった実測がある (2 回目以降は 7s 台)。
   体験上の劣化であり、機能の欠落ではないので統合を止めない。改善するなら
   サンドボックスの再利用 (pooling) が最初の手で、Hobby の作成回数の枠にも効く。

2. **Hobby の枠切れは実測していない (V14)**
   作成 5,000 回/月・Active CPU 5 時間/月を使い切った状態は意図的に作れない。
   枠切れの応答本文が想定と違えば、専用の文言ではなく一般エラーになる。
   その場合でも再試行はせず、Vercel の Usage で原因が分かる。

3. **提出コードは microVM 内で root として動く**
   ネットワークは deny-all、サンドボックスは 1 提出で捨てる、Firecracker 隔離がある、
   という 3 点で受け入れた。より強い隔離が要るなら非特権ユーザーでの実行を足す。

4. **ブラウザでの検証は未実施**
   `/api/execute` の契約はフロントから見て不変 (`{language, code}` → `ExecuteResponse`) で、
   フロントの変更は表示文言 1 箇所だけなので API レベルの検証に留めた。
   UI 経路そのものを触る変更をするときは Playwright 検証を足す。
