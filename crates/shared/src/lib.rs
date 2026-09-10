//! front (Leptos/wasm) / back (Vercel Function) / verifier が共有する契約層。
//! 問題スキーマ・実行API契約・進捗モデルはすべてここで定義し、他 crate は逆依存しない。

pub mod contract;
pub mod fixtures;
pub mod language;
pub mod problem;
pub mod progress;
pub mod runner;
pub mod sandbox;
