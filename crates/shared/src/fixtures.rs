//! 判定契約の見本コード — 7 言語それぞれの「模範解答 / 初期コード / 壊れたコード /
//! 判定テスト」の最小セット。
//!
//! これは**実行基盤を検証するための共有オラクル**。本番プロキシの疎通テストと、
//! ローカル Docker のスモークテストが同じ見本を使うことで、「片方だけ通る」状態を
//! 作らないようにしている。内容は `docs/problem-authoring.md` のテンプレートに従う
//! (= 実際の 2100 問と同じ形)。

use crate::language::Language;

pub struct Fixture {
    /// 正解。判定テストを全部通る
    pub answer: &'static str,
    /// 初期コード。コンパイルは通るがテストは通らない (未実装)
    pub starter: &'static str,
    /// わざと壊したコード。コンパイル / 型検査 / 構文解析で落ちる
    pub broken: &'static str,
    /// 判定テスト (ユーザーコードの後ろに連結される)
    pub hidden_tests: &'static str,
}

/// その言語のコンパイラ診断に必ず現れる文字列。
/// 「実物のエラーログが返っているか」を、言語固有の形まで含めて確かめるために使う。
pub fn compiler_error_signature(language: Language) -> &'static str {
    match language {
        Language::Rust => "error[E",
        Language::Cpp => "error:",
        Language::Csharp => "error CS",
        Language::Java => "error:",
        Language::Python => "SyntaxError",
        Language::Typescript => "error TS",
        Language::Javascript => "SyntaxError",
    }
}

pub fn for_language(language: Language) -> Fixture {
    match language {
        Language::Rust => Fixture {
            answer: "pub fn add(a: i32, b: i32) -> i32 { a + b }",
            starter: "pub fn add(a: i32, b: i32) -> i32 { todo!() }",
            broken: "pub fn add(a: i32, b: i32) -> i32 { let x: i32 = \"nope\"; x }",
            hidden_tests: "#[test]\nfn pos() { assert_eq!(add(1, 2), 3); }\n#[test]\nfn zero() { assert_eq!(add(0, 0), 0); }",
        },
        Language::Cpp => Fixture {
            answer: "int add(int a, int b) { return a + b; }",
            starter: "int add(int a, int b) { return 0; }",
            broken: "int add(int a, int b) { int x = \"nope\"; return x; }",
            hidden_tests: "#include <iostream>\nstatic int f = 0;\nstatic void chk(bool c, const char* n) { if (!c) { std::cerr << \"FAILED: \" << n << \"\\n\"; f++; } }\nint main() { chk(add(1,2)==3, \"pos\"); chk(add(0,0)==0, \"zero\"); if (f) { std::cout << \"test result: FAILED\\n\"; return 1; } std::cout << \"test result: ok\\n\"; return 0; }",
        },
        Language::Csharp => Fixture {
            answer: "using System;\nclass Solution { public static int Add(int a, int b) { return a + b; } }",
            starter: "using System;\nclass Solution { public static int Add(int a, int b) { return 0; } }",
            broken: "using System;\nclass Solution { public static int Add(int a, int b) { string s = 1; return 0; } }",
            hidden_tests: "class KnockTests {\n  static int f = 0;\n  static void Chk(bool c, string n) { if (!c) { System.Console.Error.WriteLine(\"FAILED: \" + n); f++; } }\n  static int Main() { Chk(Solution.Add(1,2)==3, \"pos\"); Chk(Solution.Add(0,0)==0, \"zero\"); if (f > 0) { System.Console.WriteLine(\"test result: FAILED\"); return 1; } System.Console.WriteLine(\"test result: ok\"); return 0; }\n}",
        },
        Language::Java => Fixture {
            answer: "class Solution { static int add(int a, int b) { return a + b; } }",
            starter: "class Solution { static int add(int a, int b) { return 0; } }",
            broken: "class Solution { static int add(int a, int b) { String s = 1; return 0; } }",
            hidden_tests: "class Main {\n  static int f = 0;\n  static void chk(boolean c, String n) { if (!c) { System.err.println(\"FAILED: \" + n); f++; } }\n  public static void main(String[] a) { chk(Solution.add(1,2)==3, \"pos\"); chk(Solution.add(0,0)==0, \"zero\"); if (f > 0) { System.out.println(\"test result: FAILED\"); System.exit(1); } System.out.println(\"test result: ok\"); }\n}",
        },
        Language::Python => Fixture {
            answer: "def add(a, b):\n    return a + b",
            starter: "def add(a, b):\n    raise NotImplementedError",
            broken: "def add(a, b)\n    return a + b",
            hidden_tests: "import sys\n_f = 0\ndef _chk(c, n):\n    global _f\n    if not c:\n        print(\"FAILED: \" + n, file=sys.stderr); _f += 1\n_chk(add(1, 2) == 3, \"pos\")\n_chk(add(0, 0) == 0, \"zero\")\nif _f > 0:\n    print(\"test result: FAILED\"); sys.exit(1)\nprint(\"test result: ok\")",
        },
        Language::Typescript => Fixture {
            answer: "function add(a: number, b: number): number { return a + b; }",
            starter: "function add(a: number, b: number): number { throw new Error(\"TODO\"); }",
            broken: "function add(a: number, b: number): number { const s: string = a + b; return s; }",
            hidden_tests: "declare const process: { exit(code: number): never };\nlet __f = 0;\nfunction __chk(c: boolean, n: string): void { if (!c) { console.error(\"FAILED: \" + n); __f++; } }\n__chk(add(1, 2) === 3, \"pos\");\n__chk(add(0, 0) === 0, \"zero\");\nif (__f > 0) { console.log(\"test result: FAILED\"); process.exit(1); }\nconsole.log(\"test result: ok\");",
        },
        Language::Javascript => Fixture {
            answer: "function add(a, b) { return a + b; }",
            starter: "function add(a, b) { }",
            broken: "function add(a, b) { return a + ; }",
            hidden_tests: "let __f = 0;\nfunction __chk(c, n) { if (!c) { console.error(\"FAILED: \" + n); __f++; } }\n__chk(add(1, 2) === 3, \"pos\");\n__chk(add(0, 0) === 0, \"zero\");\nif (__f > 0) { console.log(\"test result: FAILED\"); process.exit(1); }\nconsole.log(\"test result: ok\");",
        },
    }
}
