//! `vx` — Vireo's token-lean code tools for external agent CLIs (Claude Code, Codex, ...).
//! Runs locally against the workspace (no server needed, works inside network-less sandboxes).
//! See `code_intel.rs` for the operations.

#[path = "../code_intel.rs"]
mod code_intel;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};

const USAGE: &str = "vx <map|outline|view|search|apply|edit|symbol|create|ast|check|mcp> ...   (vx help for details)\n\
  vx map [query] [--dir D]\n\
  vx outline <file>\n\
  vx view <file> [symbol|A..B]\n\
  vx search <regex> [--glob G]\n\
  vx apply [--file F] [--dry-run]                                                    (V4A / anchored patch on stdin or --file)\n\
  vx edit <file> A#h[..B#h] [--insert-after|--insert-before|--delete] [--text T|--file F]\n\
  vx symbol <file> <name> [--replace|--insert-after|--delete] [--text T|--file F]\n\
  vx create <file> [--force] [--text T|--file F]\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
  vx check [--raw] [--warnings] [--timeout S] [--cmd C | cmd args...]\n\
  vx mcp [--workspace DIR] [--read-only]";

const HELP: &str = "vx - token-lean code tools: hash-anchored view/edit, atomic patches, symbol ops, repo map, build check, MCP server\n\
\n\
  vx map [query] [--dir D]        ranked file/symbol map; a query narrows by file or symbol name\n\
  vx outline <file>               symbols as [start#hash..end#hash] ranges (markdown: headings)\n\
  vx view <file> [symbol|A..B]    lines as N#hash|code; target is a function/class name or a line range\n\
  vx search <regex> [--glob G]    regex search, hits as file:N#hash| text (use `--` before a pattern starting with -)\n\
  vx apply [--file F] [--dry-run] atomic multi-file V4A patch (anchored `@@ A#h..B#h` or context `@@` hunks; `vx help apply`)\n\
  vx edit <file> A#h[..B#h]       replace those lines with new code from stdin, --file F, or --text\n\
         [--insert-after|--insert-before|--delete]   (aliases --after/--before/--remove work; unknown flags are rejected)\n\
  vx symbol <file> <name>         print a whole function/class; add --replace|--insert-after|--delete to change it\n\
  vx create <file> [--force]      new file from stdin, --file F, or --text (--force overwrites)\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
                                  ast-grep structural search; -r rewrites files IN PLACE, --dry-run only lists\n\
  vx check [--raw] [--warnings] [--timeout S] [cmd ...]\n\
                                  run cargo/tsc/go or any command (e.g. `vx check cargo test -p x`); deduplicated anchored errors, rustc notes/labels, failing test blocks; exits 1 on failure\n\
  vx mcp [--workspace DIR] [--read-only]\n\
                                  stdio MCP JSON-RPC 2.0 server exposing vx tools with zero shell quoting\n\
\n\
Workflow: map or search -> outline / view (copy the N#hh anchors) -> apply / edit / symbol -> check.\n\
On Windows, prefer `vx mcp` or `--file F` over piping multi-line code through PowerShell.\n\
Edits report new anchors and flag new syntax errors immediately; no need to re-read after editing.";

const HELP_APPLY: &str = "vx apply [--file F] [--dry-run]  (reads patch from --file F or stdin)\n\
\n\
Applies many hunks across many files atomically (all validated in memory first; nothing written on error).\n\
Uses the V4A patch envelope with hashline-anchored hunks (preferred) or classic context hunks:\n\
\n\
  *** Begin Patch\n\
  *** Update File: src/app.rs\n\
  @@ 12#ab..15#cd          # replace lines 12..15 with the `+` lines (no old text needed)\n\
  +let x = 2;\n\
  @@ after 40#ef           # insert `+` lines after line 40 (also: `before 40#ef`)\n\
  +log(\"ok\");\n\
  @@ delete 50#aa..52#bb   # remove lines 50..52\n\
  @@ fn helper() {         # classic V4A context hunk: ' ' context, '-' remove, '+' add\n\
   let a = 1;\n\
  -let b = 2;\n\
  +let b = 3;\n\
  *** Add File: src/new.rs\n\
  +pub fn hi() {}\n\
  *** Delete File: src/old.rs\n\
  *** End Patch\n\
\n\
All anchors in a patch refer to the file BEFORE the patch (so anchors from one `vx view` stay valid for every hunk).";

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Decode text handed to us by a shell or an agent's file tool: UTF-8 (BOM stripped) or UTF-16
/// LE/BE (with or without BOM; `Out-File`/`>` in Windows PowerShell 5.1 writes UTF-16LE).
fn decode_input(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| -> String {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let s = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, true)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, false)
    } else if bytes.len() >= 4 && bytes.len() % 2 == 0 && {
        // BOM-less UTF-16: ASCII-heavy text has a NUL in every other byte.
        let odd_nuls = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let even_nuls = bytes.iter().step_by(2).filter(|b| **b == 0).count();
        odd_nuls * 10 >= bytes.len() / 2 * 9 || even_nuls * 10 >= bytes.len() / 2 * 9
    } {
        let le = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count() >= bytes.iter().step_by(2).filter(|b| **b == 0).count();
        utf16(bytes, le)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    // Windows PowerShell pipes a BOM; it is never part of the code being written.
    s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s)
}

fn stdin_text() -> String {
    if std::io::stdin().is_terminal() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut bytes);
    decode_input(&bytes)
}

/// Payload text from `--file F` (written by the agent's own file tool: no shell quoting at all),
/// else `--text`, else stdin.
fn read_payload_file(path: &str) -> String {
    match std::fs::read(path) {
        Ok(b) => decode_input(&b),
        Err(e) => fail(&format!("Error: cannot read --file {path}: {e}. Nothing was changed.")),
    }
}

/// Lexically collapse `.` and `..` so `sub/../a.rs` resolves without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn flat(p: &Path) -> String {
    let s = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_lowercase()
}

fn is_inside(dir: &Path, root: &Path) -> bool {
    let (d, r) = (flat(dir), flat(root));
    d == r || d.starts_with(&format!("{r}/"))
}

/// The workspace is `VIREO_WORKSPACE` when the shell is inside it, else the enclosing git repo,
/// else the current directory. This lets `vx` work from any subfolder of a repo.
fn find_workspace(cwd: &Path) -> PathBuf {
    if let Some(w) = std::env::var_os("VIREO_WORKSPACE").map(PathBuf::from).filter(|w| !w.as_os_str().is_empty()) {
        if is_inside(cwd, &w) {
            return w;
        }
    }
    for dir in cwd.ancestors() {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
    }
    cwd.to_path_buf()
}

fn join_shell_cmd(pos: &[String]) -> Option<String> {
    if pos.is_empty() {
        return None;
    }
    if pos.len() == 1 {
        return Some(pos[0].clone());
    }
    let parts: Vec<String> = pos
        .iter()
        .map(|p| {
            if p.is_empty() || p.chars().any(|c| c.is_whitespace() || c == '"') {
                format!("\"{}\"", p.replace('"', "\\\""))
            } else {
                p.clone()
            }
        })
        .collect();
    Some(parts.join(" "))
}

// --------------------------------- MCP stdio server -------------------------------------------

fn mcp_tools(read_only: bool) -> Value {
    let mut tools = vec![
        json!({
            "name": "map",
            "description": "Ranked repo file/symbol map. Narrow by query or dir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional file or symbol filter" },
                    "dir": { "type": "string", "description": "Optional subdirectory" }
                }
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "view",
            "description": "Read a file with N#hash|code anchors (full file, line range A..B, symbol name, or outline=true for symbol list).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "target": { "type": "string", "description": "Optional line range (e.g. 40..90) or symbol name" },
                    "outline": { "type": "boolean", "description": "If true, return symbol/heading outline with [start#h..end#h] ranges" }
                },
                "required": ["path"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "search",
            "description": "Regex search across the workspace; every hit includes its N#hash anchor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Regex pattern" },
                    "glob": { "type": "string", "description": "Optional file glob, e.g. *.rs" }
                },
                "required": ["query"]
            },
            "annotations": { "readOnlyHint": true }
        }),
    ];
    if !read_only {
        tools.extend([
            json!({
                "name": "apply",
                "description": "Apply a multi-file/multi-hunk V4A patch atomically (`*** Begin Patch` / `*** Update File:` / `*** Add File:` / `*** Delete File:` / `*** End Patch`). Supports hashline-anchored hunks (`@@ 12#ab..15#cd`, `@@ after 12#ab`, `@@ before 12#ab`, `@@ delete 12#ab..15#cd` with `+` lines) and classic `@@` context hunks (` `, `-`, `+`). All anchors refer to pre-patch lines.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "patch": { "type": "string" },
                        "dry_run": { "type": "boolean" }
                    },
                    "required": ["patch"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "edit",
                "description": "Edit lines by N#hash anchor (no old text needed). Reports new anchors and syntax errors.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start": { "type": "string", "description": "Start anchor N#hh" },
                        "end": { "type": "string", "description": "Optional end anchor N#hh for a range" },
                        "op": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"] },
                        "new_text": { "type": "string" }
                    },
                    "required": ["path", "start"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "symbol",
                "description": "Read, replace, insert after, or delete a whole function/struct/class by name (includes doc comments/attributes).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "symbol": { "type": "string" },
                        "mode": { "type": "string", "enum": ["read", "replace", "insert_after", "delete"] },
                        "new_code": { "type": "string" }
                    },
                    "required": ["path", "symbol"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "check",
                "description": "Run build/tests (auto-detects cargo/tsc/go or runs cmd) and return deduplicated anchored diagnostics, rustc notes/labels, and failing test output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string", "description": "Optional shell command, e.g. 'cargo test -p pkg'" },
                        "raw": { "type": "boolean", "description": "Return unfiltered output" },
                        "warnings": { "type": "boolean", "description": "Include compiler warnings" },
                        "timeout": { "type": "integer", "description": "Timeout in seconds (default 180)" }
                    }
                },
                "annotations": { "readOnlyHint": false }
            }),
        ]);
    }
    Value::Array(tools)
}

fn handle_mcp_request(ws: &Path, read_only: bool, req: &Value) -> Option<Value> {
    let id = req.get("id").filter(|v| !v.is_null()).cloned();
    let method = req["method"].as_str().unwrap_or("");
    // Notifications have no id and must not receive a response.
    let id = id?;
    let ok = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    let err = |code: i64, message: &str| {
        Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
    };
    match method {
        "initialize" => {
            let ver = req["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05");
            ok(json!({
                "protocolVersion": ver,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "vx", "version": env!("CARGO_PKG_VERSION") }
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": mcp_tools(read_only) })),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("");
            let args = req["params"].get("arguments").cloned().unwrap_or_else(|| json!({}));
            if read_only && !matches!(name, "map" | "view" | "search") {
                return ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: tool `{name}` is disabled in read-only mode") }],
                    "isError": true
                }));
            }
            let (op, payload) = match name {
                "map" => ("map", args),
                "view" if args["outline"].as_bool() == Some(true) => ("outline", args),
                "view" => ("view", args),
                "search" => ("search", args),
                "apply" => ("apply", args),
                "edit" => {
                    let op = args["op"].as_str().unwrap_or("replace");
                    if op == "replace" && args["new_text"].as_str().unwrap_or("").is_empty() {
                        return ok(json!({
                            "content": [{ "type": "text", "text": "Error: empty replacement text; pass `new_text` or set `op` to `delete`." }],
                            "isError": true
                        }));
                    }
                    ("edit", args)
                }
                "symbol" => ("symbol", args),
                "check" => ("check", args),
                other => return err(-32602, &format!("unknown tool `{other}`")),
            };
            match code_intel::run_op(ws, op, &payload) {
                Ok(text) => {
                    let is_err = op == "check" && code_intel::check_failed(&text);
                    ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_err }))
                }
                Err(e) => ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                })),
            }
        }
        other => err(-32601, &format!("method not found: {other}")),
    }
}

fn run_mcp(ws: &Path, read_only: bool) {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(t) {
            Ok(req) => handle_mcp_request(ws, read_only, &req),
            Err(e) => Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {e}") }
            })),
        };
        if let Some(r) = resp {
            if let Ok(s) = serde_json::to_string(&r) {
                if writeln!(out, "{s}").is_err() || out.flush().is_err() {
                    break;
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        println!("{HELP}");
        return;
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        if args.get(1).map(String::as_str) == Some("apply") {
            println!("{HELP_APPLY}");
        } else {
            println!("{HELP}");
        }
        return;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ws = find_workspace(&cwd);
    // Paths may be relative to the shell's cwd (what the agent typed) or to the workspace root
    // (what `search`/`map`/`check` print); make them absolute so the core can map them.
    let abs = |p: &str| -> String {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            return normalize(&pb).to_string_lossy().to_string();
        }
        let from_cwd = normalize(&cwd.join(&pb));
        let from_ws = normalize(&ws.join(&pb));
        let chosen = if !from_cwd.exists() && from_ws.exists() { from_ws } else { from_cwd };
        chosen.to_string_lossy().to_string()
    };

    // Split flags from positionals; `--` makes everything after it positional. For `check`,
    // the first positional starts the passthrough command so `vx check cargo test -p x` works.
    let mut pos: Vec<String> = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let valued = [
        "--dir", "--text", "--file", "-f", "--glob", "--cmd", "--timeout", "-r", "--rewrite",
        "-l", "--lang", "--workspace", "-w",
    ];
    let mut only_pos = false;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if only_pos {
            pos.push(a.clone());
            i += 1;
        } else if a == "--" {
            only_pos = true;
            i += 1;
        } else if a == "-h" || a == "--help" {
            if cmd == "apply" {
                println!("{HELP_APPLY}");
            } else {
                println!("{HELP}");
            }
            return;
        } else if a.starts_with('-') && a.len() > 1 && !a.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            let takes_val = if cmd == "check" && a == "-w" {
                false
            } else {
                valued.contains(&a.as_str())
            };
            if takes_val && i + 1 < args.len() {
                flags.push((a.clone(), Some(args[i + 1].clone())));
                i += 2;
            } else {
                flags.push((a.clone(), None));
                i += 1;
            }
        } else {
            pos.push(a.clone());
            if cmd == "check" {
                only_pos = true;
            }
            i += 1;
        }
    }
    // Friendly aliases for flags.
    for (name, _) in flags.iter_mut() {
        let canon = match name.as_str() {
            "--after" | "--append" | "--insert" => "--insert-after",
            "--before" | "--prepend" => "--insert-before",
            "--remove" | "--del" | "--rm" => "--delete",
            "--overwrite" => "--force",
            "-f" => "--file",
            "-w" if cmd == "check" => "--warnings",
            "-w" if cmd == "mcp" => "--workspace",
            _ => continue,
        };
        *name = canon.to_string();
    }
    // Reject flags the command does not understand. A typo like `--after` used to fall through to
    // a plain replace and silently overwrite the anchored line.
    let allowed: &[&str] = match cmd {
        "outline" | "view" => &[],
        "map" => &["--dir"],
        "search" => &["--glob"],
        "apply" => &["--file", "--dry-run"],
        "edit" => &["--text", "--file", "--delete", "--insert-after", "--insert-before"],
        "symbol" => &["--text", "--file", "--replace", "--insert-after", "--delete"],
        "create" => &["--text", "--file", "--force"],
        "ast" => &["-r", "--rewrite", "-l", "--lang", "--dry-run"],
        "check" => &["--cmd", "--timeout", "--raw", "--warnings"],
        "mcp" => &["--workspace", "--read-only"],
        _ => &[],
    };
    for (name, _) in &flags {
        if !allowed.contains(&name.as_str()) {
            let hint = allowed.iter().copied().find(|a| a.trim_start_matches('-').starts_with(name.trim_start_matches('-')) && name.len() > 2);
            let list = if allowed.is_empty() { "no flags".to_string() } else { allowed.join(" ") };
            let did = hint.map(|h| format!(" Did you mean `{h}`?")).unwrap_or_default();
            fail(&format!("Error: unknown flag `{name}` for `vx {cmd}` (accepts: {list}).{did} Nothing was changed.\n{USAGE}"));
        }
    }
    // Mode flags are mutually exclusive.
    let modes: Vec<&str> = ["--delete", "--insert-after", "--insert-before", "--replace"]
        .into_iter()
        .filter(|m| flags.iter().any(|(k, _)| k == m))
        .collect();
    if modes.len() > 1 {
        fail(&format!("Error: conflicting flags {} - pick one. Nothing was changed.", modes.join(" + ")));
    }
    let flag = |names: &[&str]| -> Option<Option<String>> {
        flags.iter().find(|(k, _)| names.contains(&k.as_str())).map(|(_, v)| v.clone())
    };
    let flag_val = |names: &[&str]| -> Option<String> { flag(names).flatten() };
    let has = |n: &str| flag(&[n]).is_some();
    let need = |idx: usize, what: &str| -> String {
        pos.get(idx).cloned().unwrap_or_else(|| fail(&format!("missing {what}\n{USAGE}")))
    };
    let text_input = || -> String {
        let from_file = flag(&["--file"]);
        let from_text = flag(&["--text"]);
        if from_file.is_some() && from_text.is_some() {
            fail("Error: pass either --file or --text, not both. Nothing was changed.");
        }
        if let Some(opt) = from_file {
            let p = opt.unwrap_or_else(|| fail("Error: --file requires a path. Nothing was changed."));
            return read_payload_file(&p);
        }
        if let Some(opt) = from_text {
            return opt.unwrap_or_default();
        }
        stdin_text()
    };

    if cmd == "mcp" {
        let target_ws = flag_val(&["--workspace"]).map(PathBuf::from).unwrap_or(ws);
        run_mcp(&target_ws, has("--read-only"));
        return;
    }

    let (op, payload): (&str, Value) = match cmd {
        "outline" => ("outline", json!({ "path": abs(&need(0, "<file>")) })),
        "view" => ("view", json!({ "path": abs(&need(0, "<file>")), "target": pos.get(1) })),
        "map" => (
            "map",
            json!({ "query": if pos.is_empty() { Value::Null } else { json!(pos.join(" ")) }, "dir": flag_val(&["--dir"]).map(|d| abs(&d)) }),
        ),
        "search" => ("search", json!({ "query": need(0, "<regex>"), "glob": flag_val(&["--glob"]) })),
        "apply" => {
            let patch = if let Some(opt) = flag(&["--file"]) {
                let p = opt.unwrap_or_else(|| fail("Error: --file requires a path. Nothing was changed."));
                read_payload_file(&p)
            } else {
                stdin_text()
            };
            if patch.trim().is_empty() {
                fail("Error: empty patch (nothing on stdin and no --file). See `vx help apply`.");
            }
            ("apply", json!({ "patch": patch, "cwd": cwd.to_string_lossy(), "dry_run": has("--dry-run") }))
        }
        "edit" => {
            let file = abs(&need(0, "<file>"));
            let range = need(1, "A#h..B#h");
            let (start, end) = match range.split_once("..") {
                Some((a, b)) => (a.to_string(), Some(b.to_string())),
                None => (range.clone(), None),
            };
            let op = if has("--delete") {
                "delete"
            } else if has("--insert-after") {
                "insert_after"
            } else if has("--insert-before") {
                "insert_before"
            } else {
                "replace"
            };
            let text = if op == "delete" { String::new() } else { text_input() };
            // An empty replacement would silently delete the lines (e.g. stdin was empty or never
            // piped); deleting must be explicit.
            if op == "replace" && text.is_empty() {
                fail("Error: empty replacement text (nothing on stdin and no --text/--file). Pass the new code, or use --delete to remove the lines.");
            }
            ("edit", json!({ "path": file, "op": op, "start": start, "end": end, "new_text": text }))
        }
        "symbol" => {
            let mode = if has("--replace") {
                "replace"
            } else if has("--insert-after") {
                "insert_after"
            } else if has("--delete") {
                "delete"
            } else {
                "read"
            };
            let code = if matches!(mode, "replace" | "insert_after") { text_input() } else { String::new() };
            ("symbol", json!({ "path": abs(&need(0, "<file>")), "symbol": need(1, "<name>"), "mode": mode, "new_code": code }))
        }
        "create" => {
            let content = text_input();
            ("create", json!({ "path": abs(&need(0, "<file>")), "content": content, "force": has("--force") }))
        }
        "ast" => (
            "ast",
            json!({
                "pattern": need(0, "<pattern>"),
                "lang": flag_val(&["-l", "--lang"]),
                "rewrite": flag_val(&["-r", "--rewrite"]),
                "path": pos.get(1).map(|p| abs(p)),
                "dry_run": has("--dry-run"),
            }),
        ),
        "check" => {
            let cmd_str = flag_val(&["--cmd"]).or_else(|| join_shell_cmd(&pos));
            (
                "check",
                json!({
                    "cmd": cmd_str,
                    "timeout": flag_val(&["--timeout"]).and_then(|t| t.parse::<u64>().ok()),
                    "raw": has("--raw"),
                    "warnings": has("--warnings"),
                }),
            )
        }
        other => fail(&format!("unknown command `{other}`\n{USAGE}")),
    };

    match code_intel::run_op(&ws, op, &payload) {
        Ok(out) => {
            print!("{}", if out.ends_with('\n') { out.as_str() } else { "" });
            if !out.ends_with('\n') {
                println!("{out}");
            }
            if op == "check" && code_intel::check_failed(&out) {
                std::process::exit(1);
            }
        }
        Err(e) => fail(&format!("Error: {e}")),
    }
}

#[cfg(test)]
mod vx_tests {
    use super::*;

    #[test]
    fn decode_input_handles_utf8_bom_and_utf16() {
        assert_eq!(decode_input(b"\xEF\xBB\xBFhello\n"), "hello\n");
        // UTF-16LE with BOM (PowerShell 5.1 Out-File default)
        let mut le_bom = vec![0xFF, 0xFE];
        for u in "hi—✓".encode_utf16() {
            le_bom.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_input(&le_bom), "hi—✓");
        // UTF-16BE with BOM
        let mut be_bom = vec![0xFE, 0xFF];
        for u in "abc".encode_utf16() {
            be_bom.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_input(&be_bom), "abc");
        // BOM-less UTF-16LE
        let mut le_raw = Vec::new();
        for u in "fn main() {}".encode_utf16() {
            le_raw.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_input(&le_raw), "fn main() {}");
    }

    #[test]
    fn mcp_initialize_list_and_call() {
        let ws = std::env::temp_dir().join(format!("vireo-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.rs"), "fn hi() -> i32 {\n    1\n}\n").unwrap();

        let init = handle_mcp_request(
            &ws,
            false,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        // Notification returns None.
        assert!(handle_mcp_request(&ws, false, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());

        let list = handle_mcp_request(&ws, false, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"view") && names.contains(&"apply") && names.contains(&"check"), "{names:?}");

        let ro_list = handle_mcp_request(&ws, true, &json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})).unwrap();
        let ro_names: Vec<&str> = ro_list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(!ro_names.contains(&"apply") && ro_names.contains(&"view"), "{ro_names:?}");

        // View via MCP, then apply an anchored patch via MCP.
        let v = handle_mcp_request(
            &ws,
            false,
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"view","arguments":{"path":"a.rs"}}}),
        )
        .unwrap();
        let txt = v["result"]["content"][0]["text"].as_str().unwrap();
        let l2 = txt.lines().nth(2).unwrap().split('|').next().unwrap();
        let patch = format!("*** Begin Patch\n*** Update File: a.rs\n@@ {l2}\n+    42\n*** End Patch\n");
        let a = handle_mcp_request(
            &ws,
            false,
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"apply","arguments":{"patch":patch}}}),
        )
        .unwrap();
        assert_eq!(a["result"]["isError"], false, "{a}");
        assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("42"));

        // Read-only mode blocks mutating calls.
        let blocked = handle_mcp_request(
            &ws,
            true,
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"apply","arguments":{"patch":"x"}}}),
        )
        .unwrap();
        assert_eq!(blocked["result"]["isError"], true);
    }
}
