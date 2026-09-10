//! 対応言語の定義。
//!
//! 実行基盤は 7 言語すべて Vercel Sandbox (自前イメージ) に統一したので、
//! ここに言語ごとのバックエンド分岐は無い。実行コマンドの正本は `shared::runner`、
//! 実行基盤の決定は ADR 0003 (ADR 0002 を改訂)。

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Cpp,
    Csharp,
    Java,
    Python,
    Typescript,
    Javascript,
}

/// TypeScript のコンパイラフラグ。
///
/// 本番と検証が同じイメージ・同じコマンド (`shared::runner`) を使うようになったので
/// 食い違いは構造的に起きないが、版の意図を残すために定数はここに置いたままにする。
/// 食い違っていた時代は、`Object.fromEntries` のような ES2019+ の API を使う模範解答が
/// 「ローカルの verifier は緑なのに本番では TS2550 で落ちる」形で壊れた (2026-08-29 実測)。
pub const TSC_FLAGS: &[&str] = &["--target", "es2020"];

/// ローカル Docker で `tsc` に渡す形 (空白区切り)。
pub fn tsc_flags_cli() -> String {
    TSC_FLAGS.join(" ")
}

impl Language {
    pub const ALL: [Language; 7] = [
        Language::Rust,
        Language::Cpp,
        Language::Csharp,
        Language::Java,
        Language::Python,
        Language::Typescript,
        Language::Javascript,
    ];

    /// URL / ディレクトリ名 / localStorage キーに使う識別子。JSON 表現と一致する。
    pub fn slug(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Cpp => "cpp",
            Language::Csharp => "csharp",
            Language::Java => "java",
            Language::Python => "python",
            Language::Typescript => "typescript",
            Language::Javascript => "javascript",
        }
    }

    /// UI 表示名。
    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Cpp => "C++",
            Language::Csharp => "C#",
            Language::Java => "Java",
            Language::Python => "Python",
            Language::Typescript => "TypeScript",
            Language::Javascript => "JavaScript",
        }
    }

    pub fn from_slug(s: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.slug() == s)
    }

    /// CodeMirror の言語モード識別子 (assets/js/editor-src.mjs が解釈する)。
    pub fn editor_mode(self) -> &'static str {
        self.slug()
    }

    /// 行コメントの開始記号。判定テストの区切りコメントに使う。
    /// Python に `//` を入れると SyntaxError になるので、ここを言語別にするのは必須。
    pub fn line_comment(self) -> &'static str {
        match self {
            Language::Python => "#",
            _ => "//",
        }
    }

    /// 提出コードのファイル名。本番 (Sandbox) と検証 (ローカル Docker) で同じ名前を使う。
    ///
    /// Java が `prog.java` なのは Wandbox 時代の制約の名残り。自前イメージでは
    /// 任意の名前にできるが、既存 300 問が「クラスを public にしない」前提で
    /// 書かれているため名前を変えていない (変えるなら 300 問の再検証が要る)。
    pub fn source_file_name(self) -> &'static str {
        match self {
            Language::Rust => "lib.rs",
            Language::Cpp => "prog.cc",
            Language::Csharp => "prog.cs",
            Language::Java => "prog.java",
            Language::Python => "prog.py",
            Language::Typescript => "prog.ts",
            Language::Javascript => "prog.js",
        }
    }
}
