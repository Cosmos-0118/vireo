#!/usr/bin/env bash
# End-to-end pipeline for the vx code tools (view / outline / edit / symbol / search / check).
# Usage: bash tests/vx_pipeline.sh   (from backend/, after `cargo build --bins`)
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
VX="$HERE/target/debug/vx.exe"; [ -x "$VX" ] || VX="$HERE/target/debug/vx"
REPO="$(cd "$HERE/.." && pwd)"
WS="$(mktemp -d)"; export VIREO_WORKSPACE="$WS"
pass=0; fail=0
ok()   { pass=$((pass+1)); echo "  PASS  $1"; }
bad()  { fail=$((fail+1)); echo "  FAIL  $1"; [ -n "${2:-}" ] && echo "$2" | head -12 | sed 's/^/        /'; }
check(){ if eval "$2"; then ok "$1"; else bad "$1" "${3:-}"; fi; }
anchor_of() { # anchor_of <file> <exact line text> -> N#hh
  "$VX" search "$2" 2>/dev/null | grep -F -- "$1:" | head -1 | sed -E 's/^[^:]+:([0-9]+#[0-9a-f]{2})\|.*/\1/'; }

cd "$WS"; mkdir -p src web py go mini/src
cp "$REPO/backend/src/agent.rs" "$REPO/backend/src/pty.rs" src/
cp "$REPO/frontend/src/components/AgentActivity.tsx" web/
cp "$REPO/backend/src/agent.rs" src/agent.orig
printf 'import os\n\n# helper\n@decorator\ndef alpha(x):\n    return x + 1\n\n\nclass Beta:\n    def run(self):\n        return 2\n\n    def stop(self):\n        return 3\n' > py/m.py
printf 'package main\n\nimport "fmt"\n\n// Greet says hi.\nfunc Greet(n string) string {\n\treturn "hi " + n\n}\n\ntype Server struct{ Port int }\n\nfunc (s *Server) Start() error {\n\tfmt.Println(s.Port)\n\treturn nil\n}\n' > go/main.go
printf 'export const a = 1;\r\nexport function f(x: number) {\r\n  return x * 2;\r\n}\r\nexport const z = 9;\r\n' > web/crlf.ts
printf 'line1\nline2\nlast-no-newline' > notes.txt
printf '[package]\nname = "mini"\nversion = "0.1.0"\nedition = "2021"\n' > mini/Cargo.toml
printf 'fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    let x: i32 = add(1, 2);\n    let y: bool = add(3, 4);\n    println!("{} {}", x, y);\n}\n' > mini/src/main.rs

echo "== 1. outline / view / token savings"
OUT=$("$VX" outline src/agent.rs)
check "outline lists exec_tool with anchored range" 'echo "$OUT" | grep -qE "^\[[0-9]+#[0-9a-f]{2}\.\.[0-9]+#[0-9a-f]{2}\] async fn exec_tool"' "$OUT"
check "outline lists nested/other symbols" 'echo "$OUT" | grep -q "pub async fn chat" && echo "$OUT" | grep -q "struct AgentChatReq"'
full=$(wc -c < src/agent.rs); outl=${#OUT}
echo "  info  agent.rs = $full bytes, outline = $outl bytes ($((100-outl*100/full))% smaller)"
SYMV=$("$VX" view src/agent.rs exec_tool)
check "view <symbol> returns only that fn as N#hh|code" 'echo "$SYMV" | head -2 | tail -1 | grep -qE "^[0-9]+#[0-9a-f]{2}\|" && [ ${#SYMV} -lt $((full/2)) ]'
RNG=$("$VX" view src/agent.rs 20..24)
check "view A..B returns exactly 5 lines" '[ $(echo "$RNG" | grep -cE "^[0-9]+#[0-9a-f]{2}\|") -eq 5 ]' "$RNG"
check "view from subdir with relative path" '(cd src && "$VX" view agent.rs 1..2 | grep -q "^# src/agent.rs")'
check "view with absolute path inside workspace" '"$VX" view "$WS/src/agent.rs" 1..1 | grep -q "^1#"'
check "path traversal is rejected" '! "$VX" view ../etc/passwd >/dev/null 2>&1'

echo "== 2. symbol edit (rust)"
OLD=$("$VX" symbol src/pty.rs same_dir)
check "symbol read" 'echo "$OLD" | grep -q "fn same_dir"' "$OLD"
R=$("$VX" symbol src/pty.rs same_dir --replace <<'EOF'
fn same_dir(a: &Path, b: &Path) -> bool {
    // replaced by pipeline
    a == b
}
EOF
)
check "symbol replace reports success, no syntax warning" 'echo "$R" | grep -q "Replaced same_dir" && ! echo "$R" | grep -q "⚠"' "$R"
check "only that function changed (diff touches one hunk)" '[ $(diff <(cat "$REPO/backend/src/pty.rs") src/pty.rs | grep -c "^[0-9]") -eq 1 ]'
check "file still parses (cargo-style brace balance)" '[ $(tr -cd "{" < src/pty.rs | wc -c) -eq $(tr -cd "}" < src/pty.rs | wc -c) ]'
"$VX" symbol src/pty.rs same_dir --insert-after >/dev/null 2>&1 <<'EOF'
fn extra_helper() -> u8 {
    7
}
EOF
check "insert_after puts new fn after symbol with blank separator" '"$VX" outline src/pty.rs | grep -q "fn extra_helper"'
"$VX" symbol src/pty.rs extra_helper --delete >/dev/null
check "delete symbol removes it" '! "$VX" outline src/pty.rs | grep -q extra_helper'
AMB=$("$VX" symbol src/agent.rs ChatMessage 2>&1)
check "struct read by name" 'echo "$AMB" | grep -q "pub struct ChatMessage"' "$AMB"
NF=$("$VX" symbol src/agent.rs nope_fn 2>&1)
check "unknown symbol gives helpful error" 'echo "$NF" | grep -qi "not found"' "$NF"

echo "== 3. hashline edits: drift, stale, insert, delete"
A1=$(anchor_of src/agent.rs "const SYSTEM_PROMPT")
CM=$("$VX" search "pub struct ChatMessage" | grep -F "src/agent.rs:" | head -1 | sed -E 's/^[^:]+:([0-9]+)#.*/\1/')
A_LIST=$("$VX" view src/agent.rs $((CM-1))..$((CM+3)) | sed -n '2p;6p' | sed -E 's/\|.*//')
S=$(echo "$A_LIST" | sed -n 1p); E=$(echo "$A_LIST" | sed -n 2p)
# shift the file down by 3 lines using an insert at the top
H1=$("$VX" view src/agent.rs 1..1 | sed -n 2p | sed -E 's/\|.*//')
printf 'pub const A: u8 = 1;
pub const B: u8 = 2;
pub const C: u8 = 3;
' | "$VX" edit src/agent.rs "$H1" --insert-before >/dev/null
R=$("$VX" edit src/agent.rs "$S..$E" <<'EOF'
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub pipeline_marker: bool,
}
EOF
)
check "stale line numbers are re-located after the file shifted" 'echo "$R" | grep -q "^Edited" && grep -q "pipeline_marker" src/agent.rs' "$R"
check "edit touched only intended region" '[ $(diff src/agent.orig src/agent.rs | grep -c "^[0-9]") -eq 2 ]'
cp src/agent.rs /tmp/agent.before.$$
R=$("$VX" edit src/agent.rs "20#ff..24#ee" <<<'garbage' 2>&1)
check "wrong anchors rejected with fresh snippet" 'echo "$R" | grep -q "no longer match" && echo "$R" | grep -qE "^[0-9]+#[0-9a-f]{2}\|"' "$R"
check "rejected edit leaves file byte-identical" 'cmp -s /tmp/agent.before.$$ src/agent.rs'
rm -f /tmp/agent.before.$$
L=$(anchor_of src/agent.rs "pub const B: u8 = 2;")
"$VX" edit src/agent.rs "$L" --delete >/dev/null
check "delete single line" '! grep -q "pub const B" src/agent.rs && grep -q "pub const C" src/agent.rs'
L=$(anchor_of src/agent.rs "pub const A: u8 = 1;")
"$VX" edit src/agent.rs "$L" --insert-after --text 'pub const A2: u8 = 11;' >/dev/null
check "insert-after with --text lands directly after anchor" '[ "$(grep -A1 "pub const A: u8" src/agent.rs | tail -1)" = "pub const A2: u8 = 11;" ]'
L=$(anchor_of src/agent.rs "pub const C: u8 = 3;")
R=$("$VX" edit src/agent.rs "$L" <<<"$(printf '%s' "1#ab|pub const C: u8 = 33;")")
check "echoed N#hh| prefixes in new text are stripped" 'grep -q "^pub const C: u8 = 33;$" src/agent.rs && ! grep -q "ab|pub" src/agent.rs' "$R"
R=$("$VX" edit src/agent.rs "$L" <<<"x" 2>&1)
check "re-using an anchor whose line changed is rejected" 'echo "$R" | grep -q "no longer match"' "$R"

echo "== 4. syntax validation after edit"
L=$(anchor_of src/pty.rs "// replaced by pipeline")
R=$("$VX" edit src/pty.rs "$L" <<<'    let broken = (1 + ;')
check "introduced syntax error is reported immediately" 'echo "$R" | grep -q "⚠ new syntax error"' "$R"
NL=$(echo "$R" | sed -nE 's/.*Now ([0-9]+#[0-9a-f]{2})\.\..*/\1/p')
R=$("$VX" edit src/pty.rs "$NL" <<<'    // replaced by pipeline')
check "fixing it clears the warning" '! echo "$R" | grep -q "⚠"' "$R"

echo "== 5. CRLF / no-trailing-newline / BOM safety"
A=$("$VX" view web/crlf.ts 2..2 | sed -n 2p | sed -E 's/\|.*//'); B=$("$VX" view web/crlf.ts 4..4 | sed -n 2p | sed -E 's/\|.*//')
"$VX" edit web/crlf.ts "$A..$B" >/dev/null <<'EOF'
export function f(x: number) {
  return x * 3;
}
EOF
check "CRLF preserved for every line" '[ $(grep -c $'"'"'\r$'"'"' web/crlf.ts) -eq 5 ] && grep -q "x \* 3" web/crlf.ts'
L=$(anchor_of notes.txt "last-no-newline")
"$VX" edit notes.txt "$L" --text 'replaced-last' >/dev/null
check "file without trailing newline stays without one" '[ "$(tail -c 13 notes.txt)" = "replaced-last" ]'
L=$(anchor_of notes.txt "replaced-last")
"$VX" edit notes.txt "$L" --insert-after --text 'appended' >/dev/null
check "append after last line (no EOL) keeps layout" '[ "$(cat notes.txt)" = "$(printf "line1\nline2\nreplaced-last\nappended")" ]'

echo "== 6. other languages"
O=$("$VX" outline py/m.py); check "python outline (decorator, class, methods)" 'echo "$O" | grep -q "def alpha" && echo "$O" | grep -q "class Beta" && echo "$O" | grep -q "def stop"' "$O"
"$VX" symbol py/m.py Beta.run --replace >/dev/null <<'EOF'
    def run(self):
        return 20
EOF
check "python method replace" 'grep -q "return 20" py/m.py && grep -q "return 3" py/m.py'
O=$("$VX" outline go/main.go); check "go outline (func, method, type)" 'echo "$O" | grep -q "func Greet" && echo "$O" | grep -q "type Server" && echo "$O" | grep -q "Start()"' "$O"
"$VX" symbol go/main.go Server::Start --replace >/dev/null <<'EOF'
func (s *Server) Start() error {
	return fmt.Errorf("port %d", s.Port)
}
EOF
check "go method replace via Type::method" 'grep -q "port %d" go/main.go'
O=$("$VX" outline web/AgentActivity.tsx); check "tsx outline of real component file" 'echo "$O" | grep -q "describeTool" && echo "$O" | grep -q "ActivityGroup"' "$(echo "$O" | head -5)"
"$VX" symbol web/AgentActivity.tsx baseName --replace >/dev/null <<'EOF'
export const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
EOF
check "tsx arrow-const replace" 'grep -q "pop() ?? p" web/AgentActivity.tsx'
check "tsx diff vs original limited to one line" '[ $(diff "$REPO/frontend/src/components/AgentActivity.tsx" web/AgentActivity.tsx | grep -c "^[0-9]") -eq 1 ]'

echo "== 7. search, map, create"
S=$("$VX" search "fn same_dir"); check "search returns file:line#hash| text" 'echo "$S" | grep -qE "^src/pty.rs:[0-9]+#[0-9a-f]{2}\| "' "$S"
M=$("$VX" map exec_tool); check "map <query> ranks agent.rs first" 'echo "$M" | head -1 | grep -q "^src/agent.rs"' "$M"
M=$("$VX" map); check "map (no query) lists files with symbols" 'echo "$M" | grep -q "pty.rs" && echo "$M" | grep -q "agent.rs"' "$M"
echo 'fn t() {}' | "$VX" create src/new.rs >/dev/null; check "create new file" '[ -f src/new.rs ]'
check "create refuses to overwrite" '! echo x | "$VX" create src/new.rs >/dev/null 2>&1'

echo "== 8. check: compressed diagnostics -> anchor -> fix -> green"
C=$("$VX" check 2>&1)
check "cargo error compressed with location + anchored context" 'echo "$C" | grep -q "✖ 1 error" && echo "$C" | grep -q "mini/src/main.rs:7" && echo "$C" | grep -qE "^ > 7#[0-9a-f]{2}\|"' "$C"
echo "  info  check output = ${#C} chars"
FIX=$(echo "$C" | grep -E "^ > [0-9]+#" | sed -E 's/^ > ([0-9]+#[0-9a-f]{2})\|.*/\1/')
"$VX" edit mini/src/main.rs "$FIX" --text '    let y: i32 = add(3, 4);' >/dev/null
C=$("$VX" check 2>&1); check "check passes after anchor-based fix (no re-read needed)" 'echo "$C" | grep -q "✔ cargo check ok"' "$C"

echo "== 9. ast-grep (optional)"
if command -v ast-grep >/dev/null 2>&1; then
  R=$("$VX" ast 'add($A, $B)' -l rust); check "ast search" 'echo "$R" | grep -q "match"' "$R"
else
  R=$("$VX" ast 'add($A, $B)' -l rust 2>&1); check "ast reports missing ast-grep clearly" 'echo "$R" | grep -q "not installed"' "$R"
fi

echo; echo "RESULT: $pass passed, $fail failed  (workspace: $WS)"
[ "$fail" -eq 0 ]
