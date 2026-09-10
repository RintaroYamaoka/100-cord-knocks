# ADR 0002 — 多言語対応の実行バックエンド

- 状態: **一部を [ADR 0003](0003-unified-sandbox-execution.md) が改訂** (実行バックエンドの選定)
- 日付: 2026-08-29
- 関連: ADR 0001 (Playground プロキシ実行) / ADR 0003 (Sandbox への統一)

> **⚠️ 2026-09-11 改訂**: 「Rust は Playground / 他 6 言語は Wandbox」という**バックエンド選定と
> コンパイラ ID の表、および検証イメージの表は ADR 0003 が置き換えた**
> (Wandbox の 4 日以上の全体障害を受けて、7 言語すべて Vercel Sandbox の自前イメージに統一)。
> **判定契約 (終了コードと stdout の目印、`Outcome` の判定順序) と問題コンテンツ側の制約は
> 本 ADR が引き続き正本**。

## 背景

Rust に加えて C++ / C# / Java / Python / TypeScript / JavaScript の各 300 問を提供する。
ADR 0001 と同じく「ブラウザで書いて、**実物のコンパイラ/ランタイムのエラーログ**を読む」
体験を全言語で成立させる必要がある。サーバーに正誤判定の状態は持たない。

## 選択肢と実測

2026-08-29 に候補を実測した。

| 候補 | 実測結果 | 判定 |
|---|---|---|
| Piston 公開 API (emkc.org) | **2026-02-15 からホワイトリスト制**。`/execute` は 403 相当のメッセージを返す | 不採用 |
| Piston セルフホスト (Docker) | 動くが常駐ホスト (VPS) が別途必要。Vercel には置けない | 将来の退避先 |
| Judge0 CE 公開 (ce.judge0.com) | 生存。C++ 提出が通り実 gcc 診断を返した | フォールバック |
| **Wandbox (wandbox.org)** | 生存。目標 6 言語すべてに対応し、実コンパイラ診断・終了コードが取れた | **採択** |
| ブラウザ内実行 (Pyodide / tsc / V8) | JS/TS/Python は可能。C++/Java/C# は非現実的 | 将来の最適化 |

## 決定

**Rust は Playground を維持し、他 6 言語は Wandbox にプロキシする。**
フロントからは 1 本の `/api/execute` 契約に見える。

| 言語 | バックエンド | コンパイラ ID |
|---|---|---|
| Rust | play.rust-lang.org | channel=stable / edition=2024 / lib / tests |
| C++ | wandbox.org | `gcc-13.2.0` (options `warning,c++17`) |
| C# | wandbox.org | `dotnetcore-6.0.425` |
| Java | wandbox.org | `openjdk-jdk-22+36` |
| Python | wandbox.org | `cpython-3.13.8` |
| TypeScript | wandbox.org | `typescript-5.6.2` |
| JavaScript | wandbox.org | `nodejs-20.17.0` |

ブラウザ内実行は採用しない。実行経路が 2 系統に増え、Pyodide (10MB 超) と
tsc (7MB 超) の初期ロードを抱える割に、得られるのはレイテンシ改善だけだから。
Wandbox のレート制限が実運用で問題になったときに再検討する (退避先はセルフホスト Piston)。

## 判定契約 (全言語共通)

サーバーに状態を持たないため、正誤は **提出コードの終了コードと標準出力の目印** で判定する。
`hidden_tests` はユーザーコードの**後ろ**に連結する (ユーザーコードの行番号が
コンパイラ診断とずれないため)。各言語の `hidden_tests` は次を満たす:

- 全テスト成功 → **stdout** に `test result: ok` を出し **exit 0**
- テスト失敗 → **stdout** に `test result: FAILED` を出し、
  内訳 `FAILED: <名前>` を stderr に出して **exit != 0**

**目印は必ず stdout に出す。** cargo test が `test result: ok. N passed` を stdout に
書くのと同じ位置に揃えるためで、これにより Rust と非 Rust で判定経路が 1 本に保てる。
stderr はコンパイラ診断と実行時エラーの表示専用とする。

`Outcome` の判定順序は全言語共通:

1. exit 0 **かつ** stdout に `test result: ok` がある → `Passed`
2. コンパイル診断にエラー行がある → `CompileError`
3. `test result: FAILED` がある → `TestsFailed`
4. exit != 0 → `RuntimeError`
5. exit 0 なのに目印が無い → `NoTestsRun`

**5 が要る理由**: 判定テストをユーザーコードの後ろに連結する方式では、ユーザーコードが
先に `sys.exit(0)` / `process.exit(0)` を呼べば、テストが 1 件も走らないまま exit 0 になる。
「exit 0 なら正解」とすると、これが「✓ 正解!」として通ってしまう。成功の目印の存在を
`Passed` の必要条件にすることで塞ぐ。

Rust も同じ判定を通る (cargo test が `test result: ok` / `test result: FAILED` を
stdout に出すため、`#[test]` 形式のまま既存 300 問に手を入れずに済む)。

## 実測で判明した言語別の制約

これらは問題コンテンツの書き方を縛るので、`verifier` が機械検査する。

- **Java**: Wandbox はファイル名が `prog.java` 固定。`public class` は
  `class X is public, should be declared in a file named X.java` で落ちる。
  → 問題の全クラスを **package-private (`class Main`)** で書く
- **C#**: `using` はファイル先頭にしか置けず、harness はユーザーコードの後ろに来る。
  → harness は `System.Console` のように**完全修飾**で書く
- **C#**: `dotnetcore` は成功時も `dotnet new` / MSBuild の定型出力を
  `compiler_output` に吐く。→ プロキシ側で定型行を除去する。
  除去は**ホワイトリスト方向** (`error` / `warning` を含む行は必ず残す) で行う。
  詳しい規則は下の「ADR 0001 との関係」節が正本
- **C# `dotnetcore-8.0.402` は使用不可**: `dotnet new console` が
  `File size limit exceeded (core dumped)` で落ちる (Wandbox 側の制約)
- **TypeScript**: Node の型定義が無く `process` が `TS2580` になる。
  → harness は `declare const process: { exit(code: number): never };` を自前で置く
- **Wandbox は既定 User-Agent を 403 で弾く**。→ プロキシは明示的に UA を送る
- **Wandbox は過負荷時に `OCI runtime error: crun: clone: Resource temporarily
  unavailable` を返す**。→ 一時エラーとして扱い、利用者に再試行を促す

## 品質ゲート (verifier)

Playground / Wandbox に一括負荷をかけない原則 (ADR 0001) は維持する。
1800 問の検証は**ローカル Docker** で行い、イメージは Wandbox の版に合わせて固定する。

| 言語 | 検証イメージ | 実測版 |
|---|---|---|
| C++ | `gcc:13` | 13.4.0 |
| Java | `eclipse-temurin:22-jdk` | javac 22.0.2 |
| C# | `mcr.microsoft.com/dotnet/sdk:6.0` | 6.0.428 |
| Python | `python:3.13` | 3.13.15 |
| JavaScript | `node:20` | v20.20.2 |
| TypeScript | `knocks-ts:5.6.2` (node:20 + typescript@5.6.2) | 5.6.2 |

Rust は従来どおりローカル `cargo test` (高速で既に動いている)。

コンテナ起動は 1 問ごとではなく **バッチ (ファイル) ごとに 1 回**にする。
1800 問 × 2 (answer/starter) の起動を個別に行うと、コンテナ起動だけで数時間かかるため。

## ADR 0001 との関係 (レスポンス無変換原則の一部改訂)

ADR 0001 は「バックエンドは許可リスト検証だけを行い、上流の応答を変換しない」と決めていた。
本 ADR はこれを **C# のビルドノイズ除去に限って改訂**する。`dotnetcore` は成功時も
`dotnet new` / MSBuild の定型出力を吐き、そのまま見せると「実物の診断を読む」という
目的そのものを潰すため。除去は次のホワイトリスト方向で行う:

- `error` / `warning` を含む行は**必ず残す** (MSBuild 自身のエラー `error MSBnnnn` も残る)
- 既知の定型行 (`The template`, `Processing post-creation`, `Running 'dotnet restore'`,
  `Determining projects`, `Restored `, `Restore succeeded`, `MSBuild version`,
  `All projects are up-to-date`, `Build succeeded`, `Time Elapsed`, ビルド成果物パス行) だけを落とす

Wandbox → `ExecuteResponse` の詰め替え (`program_output` → stdout、診断 → stderr、
`status` → `success`) も変換にあたるが、これは契約の翻訳であって内容の改変ではない。

## 結果として受け入れるリスク

- Wandbox は非公式・ボランティア運用で、過負荷時に一時エラーを返す。
  ADR 0001 が Playground について受け入れたのと同じリスク階級である
- 退避先は Judge0 CE 公開インスタンス、その次にセルフホスト Piston
