"""提出 1 回のレイテンシの内訳を実測する (ADR 0003 の残課題「レイテンシ改善」用)。

測るもの:
  (a) サンドボックスの作成 / 停止そのもののコスト
  (b) 同一サンドボックスで提出を繰り返したときの 2 回目以降 (= 再利用で到達できる下限)

使い方:
    vercel link --yes --project 100-cord-knocks && vercel env pull --yes
    python3 scripts/measure-sandbox-latency.py

注意:
  - `.env.local` の値は引用符付きで書かれる。素朴に `split("=")` すると 403 になる
  - api.vercel.com は User-Agent を見る。明示的に名乗らないと 403 になる
  - ここで組むスクリプトは `shared::runner::build_script` の写しであって正本ではない。
    判定まで含めて確かめたいときは `cargo test -p rust-100-knocks-api -- --ignored` を使う
"""
import base64, json, os, time, urllib.request, uuid

API = "https://api.vercel.com"
TOKEN = None
for line in open(".env.local"):
    if line.startswith("VERCEL_OIDC_TOKEN="):
        TOKEN = line.split("=", 1)[1].strip().strip('"')
IMAGE = "knocks-runtime:2026-09-11"
T = 20

def req(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(API + path, data=data, method=method,
                               headers={"Authorization": f"Bearer {TOKEN}", "Content-Type": "application/json",
                                        "User-Agent": "knocks-latency-probe/1.0"})
    with urllib.request.urlopen(r, timeout=180) as resp:
        return resp.read().decode()

PLANS = {
  "python":  (None, None, "python3 prog.py", "prog.py", "def add(a,b):\n    return a+b\n\nimport sys\nif add(1,2)!=3:\n    print('test result: FAILED'); sys.exit(1)\nprint('test result: ok')\n"),
  "javascript": (None, None, "node prog.js", "prog.js", "function add(a,b){return a+b}\nif(add(1,2)!==3){console.log('test result: FAILED');process.exit(1)}\nconsole.log('test result: ok')\n"),
  "cpp":     (None, "g++ -std=c++17 -w -o _prog prog.cc", "./_prog", "prog.cc", "int add(int a,int b){return a+b;}\n#include <iostream>\nint main(){ if(add(1,2)!=3){std::cout<<\"test result: FAILED\\n\";return 1;} std::cout<<\"test result: ok\\n\"; return 0;}\n"),
  "java":    (None, "javac -nowarn prog.java", "java Main", "prog.java", "class Solution{static int add(int a,int b){return a+b;}}\nclass Main{public static void main(String[] x){ if(Solution.add(1,2)!=3){System.out.println(\"test result: FAILED\");System.exit(1);} System.out.println(\"test result: ok\");}}\n"),
  "csharp":  ("cp -a /opt/knocks/csharp/. . && rm -f Program.cs", "dotnet build --no-restore -v q --nologo -p:GenerateFullPaths=false -o _out", "dotnet _out/knocks.dll", "prog.cs", "class Solution{public static int Add(int a,int b){return a+b;}}\nclass KnockTests{static int Main(){ if(Solution.Add(1,2)!=3){System.Console.WriteLine(\"test result: FAILED\");return 1;} System.Console.WriteLine(\"test result: ok\"); return 0;}}\n"),
  "rust":    ("cp -a /opt/knocks/rust/. . && mkdir -p src && mv lib.rs src/lib.rs", None, "cargo test --offline", "lib.rs", "pub fn add(a:i32,b:i32)->i32{a+b}\n#[test]\nfn t(){assert_eq!(add(1,2),3);}\n"),
}

def script(lang):
    prep, comp, run, src, code = PLANS[lang]
    b64 = base64.b64encode(code.encode()).decode()
    n = "KNOCKS" + uuid.uuid4().hex
    blocks = " && ".join([x for x in (prep, comp) if x])
    s = f"set -u\nexport HOME=/tmp\nexport DOTNET_CLI_HOME=/tmp\n__d=$(mktemp -d) && cd \"$__d\" || exit 90\nprintf %s '{b64}' | base64 -d > {src}\n"
    s += (f"{{ {blocks} ; }} >_cout 2>_cerr\n__c=$?\n" if blocks else ": >_cout\n: >_cerr\n__c=0\n")
    s += f"if [ \"$__c\" -eq 0 ]; then\n timeout {T} {run} >_pout 2>_perr\n echo $? >_exit\nelse\n : >_pout\n : >_perr\n : >_exit\nfi\n"
    for sec in ["pout", "exit"]:
        s += f"printf '\\n{n}:{sec}\\n'\ncat _{sec} 2>/dev/null\n"
    s += f"printf '\\n{n}:end\\n'\nexit 0\n"
    return s, n

def run_cmd(sid, s):
    t = time.time()
    out = req("POST", f"/v2/sandboxes/sessions/{sid}/cmd?cmdId=c{uuid.uuid4().hex[:16]}",
              {"command": "sh", "args": ["-c", s], "wait": True, "logs": True, "timeout": 45000})
    el = time.time() - t
    ok = "test result: ok" in out
    return el, ok

print(f"{'言語':<12} {'作成':>6} {'1回目':>7} {'2回目':>7} {'3回目':>7} {'停止':>6}  {'毎回作り直し':>12} {'再利用':>8}")
for lang in PLANS:
    t = time.time()
    sid = json.loads(req("POST", "/v4/sandboxes", {"image": IMAGE, "resources": {"vcpus": 1},
         "timeout": 180000, "persistent": False, "networkPolicy": {"mode": "deny-all"}}))["session"]["id"]
    create = time.time() - t
    runs = []
    for _ in range(3):
        s, n = script(lang)
        el, ok = run_cmd(sid, s)
        runs.append((el, ok))
    t = time.time()
    req("POST", f"/v2/sandboxes/sessions/{sid}/stop", {})
    stop = time.time() - t
    cold = create + runs[0][0] + stop
    warm = sum(r[0] for r in runs[1:]) / 2
    marks = "".join("✓" if r[1] else "✗" for r in runs)
    print(f"{lang:<12} {create:6.2f} {runs[0][0]:7.2f} {runs[1][0]:7.2f} {runs[2][0]:7.2f} {stop:6.2f}  {cold:12.2f} {warm:8.2f}  {marks}")
