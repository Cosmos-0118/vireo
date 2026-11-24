//! Zero-dependency JSON-RPC 2.0 stdio MCP server exposing Vireo's `vx` tools.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::path::Path;

use super::code_intel;

/// Sent in the MCP `initialize` reply: exact argument shapes, so a host that lazy-loads tool
/// schemas needs no extra round trip to learn them.
const MCP_INSTRUCTIONS: &str = "vx tool arguments (JSON):\n\
- view {path, target?: \"10..50\" | \"10..30,80..100\" | \"Symbol\", outline?: true} or {specs: [\"a.rs:10..50\", \"b.rs:Foo\"]}\n\
- search {query, path?, glob?, literal?, ignore_case?, context?, files_only?}\n\
- edit {path, start: \"12#ab..15#cd\" | \"top\" | \"bottom\" | \"imports\", new_text, op?: replace|insert_after|insert_before|delete}; \
or {path, symbol, new_text}; or {path, old_text, new_text} (no anchors needed, returns fresh ones); or {path, edits: [{start, end?, op?, new_text}]}\n\
- apply {patch: \"*** Update File: a.rs\\n@@ 12#ab..15#cd\\n+new line\"}\n\
- replace {old, new, path?, glob?, regex?, expected?, dry_run?}\n\
- check {cmd?, dir?, warnings?}: a build still running after 60s continues in the background and returns a job id; call check {job} to wait for it.\n\
- release {action?: status|preflight|notes|bump|publish|verify, version?: patch|minor|major|prerelease|X.Y.Z, dry_run?, skip_tests?, write?, changelog?, notes_file?, publish?, no_push?, assets?, only?}: polyglot release engine; run publish with dry_run first.\n\
Anchors `N#hh` come from view/search/edit output; edits return fresh anchors, so no re-view is needed.";

pub fn mcp_tools(read_only: bool) -> Value {
    let mut tools = vec![
        json!({
            "name": "map",
            "description": "Ranked repo file/symbol map. Narrow by query or dir (auto-resolves unique subdir names).",
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
            "description": "Read files with N#hash|code anchors: single file (full, range A..B, comma-separated multi-range A..B,C..D, symbol, or outline=true) OR batch multiple files via `specs` (e.g. [\"a.ts:10..30\", \"b.ts:foo\"]). Auto-resolves unique workspace paths.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (omit if using `specs`)" },
                    "target": { "type": "string", "description": "Optional line range (40..90), comma-separated ranges (10..30,80..100), or symbol name" },
                    "specs": { "type": "array", "items": { "type": "string" }, "description": "Optional batch list of `file[:range_or_symbol]` specs" },
                    "outline": { "type": "boolean", "description": "If true, return symbol/heading outline with [start#h..end#h] ranges" }
                }
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "search",
            "description": "Anchored regex/literal search across the workspace. Accepts path scope, single or multiple globs (auto-recovers omitted monorepo prefixes like `src/**`), context lines with anchors, and files-only mode.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Regex (or literal if literal=true) pattern" },
                    "path": { "type": "string", "description": "Optional directory or file path scope" },
                    "glob": { "type": "string", "description": "Optional file glob, e.g. *.tsx or frontend/src/**" },
                    "globs": { "type": "array", "items": { "type": "string" }, "description": "Optional list of file globs (supports !exclude)" },
                    "literal": { "type": "boolean", "description": "Treat query as a fixed literal string (no regex escaping)" },
                    "ignore_case": { "type": "boolean", "description": "Case-insensitive search" },
                    "context": { "type": "integer", "description": "Lines of anchored context around each match (0..10)" },
                    "files_only": { "type": "boolean", "description": "List matching files with hit counts instead of lines" },
                    "limit": { "type": "integer", "description": "Max hits/files to return (default 80, max 500)" }
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
                "description": "Apply a multi-file/multi-hunk V4A patch atomically (`*** Update File:` / `*** Add File:` / `*** Delete File:`, optionally wrapped in `*** Begin Patch` / `*** End Patch`). Supports hashline-anchored hunks (`@@ 12#ab..15#cd`, `@@ after 12#ab`, `@@ before 12#ab`, `@@ delete 12#ab..15#cd`; write ONLY `+` lines there: the anchors already name the replaced lines, so no `-`/context lines (a stray `-old` echo is ignored)), anchor-free `@@ top` / `@@ bottom` / `@@ after imports` (inserts after the file's import block and skips imports already present; no view needed), and classic `@@` context hunks (` `, `-`, `+`). All anchors refer to pre-patch lines. Paths auto-resolve unique suffixes (`components/X.tsx` -> `frontend/src/components/X.tsx`); new files go next to an existing matching folder.",
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
                "name": "replace",
                "description": "Atomic literal (default) or regex find-and-replace across one file, a directory, or a glob. Preserves CRLF, runs syntax checks, supports dry_run and expected occurrence count guard.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "old": { "type": "string", "description": "Exact text (or regex if regex=true) to replace" },
                        "new": { "type": "string", "description": "Replacement text" },
                        "path": { "type": "string", "description": "Target file or directory" },
                        "glob": { "type": "string", "description": "Optional file glob" },
                        "globs": { "type": "array", "items": { "type": "string" } },
                        "regex": { "type": "boolean", "description": "Interpret `old` as regex" },
                        "dry_run": { "type": "boolean", "description": "Preview matches without writing" },
                        "expected": { "type": "integer", "description": "Abort if total occurrences != expected" },
                        "all_files": { "type": "boolean", "description": "Allow replacing across entire workspace with no path/glob" }
                    },
                    "required": ["old", "new"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "edit",
                "description": "Edit lines by N#hash anchor (no old text needed). For several edits to one file, pass them together in `edits: [{start, end?, op?, new_text}]` so every anchor resolves against the current file at once. Accepts the same paths as view (unique suffixes auto-resolve). Returns fresh anchors around the edit and syntax errors with the offending lines.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start": { "type": "string", "description": "Anchor `12#ab`, a whole range `12#ab..15#cd` (as printed by view/outline), or `top` / `bottom` / `imports` to insert new_text there without viewing first (`imports` skips lines already present)" },
                        "end": { "type": "string", "description": "Optional end anchor when `start` is a single anchor" },
                        "op": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"], "description": "Default replace (needs new_text); use delete to remove lines" },
                        "new_text": { "type": "string", "description": "Replacement / inserted text" },
                        "old_text": { "type": "string", "description": "No anchors yet (e.g. you read the file another way)? Pass the exact text to replace instead of `start`; it must occur once. Returns fresh N#hash anchors for follow-up edits" },
                        "edits": { "type": "array", "description": "Several edits to this file in ONE atomic call; anchors all refer to the file as it is now. Each: {start, end?, op?, new_text?}", "items": { "type": "object" } },
                        "symbol": { "type": "string", "description": "Instead of anchors: edit a whole symbol by name (op replace / insert_after / delete)" }
                    },
                    "required": ["path"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "symbol",
                "description": "Read, replace, insert_after, or delete a whole symbol (function, struct, class, method) by name.",
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
                "description": "Run build/typecheck/tests (auto-detected or custom `cmd`) and return deduplicated anchored errors, rustc labels/notes, and failing test output. A check still running after `wait` seconds (default 60) keeps running in the background and returns a job id: call again with {job} to wait for it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string", "description": "Optional command, e.g. `cargo test -p vireo`. In a monorepo, npm/cargo/go/... commands run in the one top-level folder holding their manifest when the root has none" },
                        "dir": { "type": "string", "description": "Folder to run in (auto-detected checks or `cmd`); unique names like `frontend` auto-resolve" },
                        "raw": { "type": "boolean" },
                        "warnings": { "type": "boolean" },
                        "timeout": { "type": "integer", "description": "Hard kill after this many seconds in total (default 900)" },
                        "wait": { "type": "integer", "description": "Seconds to wait before returning a background job id (default 60)" },
                        "job": { "type": "string", "description": "Job id from an earlier `⏳ still running` reply: wait for that check and report it" }
                    }
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "release",
                "description": "Release engineering across Rust/Node/Tauri/.NET/Python/Java/Go/installers. `status` (default): stacks, version files + drift, commits since last tag, suggested bump, CI mode. `preflight`: clean tree, branch, remote sync, free tag, gh auth, build gate. `notes`: Conventional-Commit release notes (`write` saves an editable draft, `changelog` prepends CHANGELOG.md). `bump`: atomic lossless version sync of every manifest + lockfile (`dry_run` previews). `publish`: preflight -> bump -> commit -> tag -> push (CI tag workflows) or draft GitHub release (`publish: true` makes it public); ALWAYS call with dry_run first and show the plan to the user. `verify`: remote tag, release state, CI run.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["status", "preflight", "notes", "bump", "publish", "verify"] },
                        "version": { "type": "string", "description": "patch | minor | major | prerelease | X.Y.Z[-pre]. Omit to use the suggested version" },
                        "dry_run": { "type": "boolean", "description": "bump/publish: report what would happen, change nothing" },
                        "skip_tests": { "type": "boolean", "description": "preflight/publish: skip the build & test gate" },
                        "write": { "type": "boolean", "description": "notes: save the draft to .github/release-notes/<tag>.md (or .vireo/release-notes/) so it can be edited before publish" },
                        "changelog": { "type": "boolean", "description": "notes: also prepend the section to CHANGELOG.md" },
                        "notes_file": { "type": "string", "description": "notes/publish: notes markdown file to write / use as the release body" },
                        "publish": { "type": "boolean", "description": "publish (gh mode): promote the draft release to public" },
                        "no_push": { "type": "boolean", "description": "publish: commit and tag locally only" },
                        "assets": { "type": "array", "items": { "type": "string" }, "description": "publish (gh mode): globs of files to attach (a SHA256SUMS.txt is added)" },
                        "only": { "type": "array", "items": { "type": "string" }, "description": "bump/publish: limit to these version files (default all)" }
                    }
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "create",
                "description": "Create a new file (refuses to overwrite unless force=true). Parent folders are created.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" },
                        "force": { "type": "boolean", "description": "Overwrite if the file exists" }
                    },
                    "required": ["path", "content"]
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "ast",
                "description": "Structural (ast-grep) search, optionally rewrite with `rewrite` (use $VAR metavariables). Set dry_run to preview a rewrite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "ast-grep pattern, e.g. `$X.unwrap()`" },
                        "rewrite": { "type": "string", "description": "Optional rewrite template" },
                        "lang": { "type": "string" },
                        "path": { "type": "string" },
                        "dry_run": { "type": "boolean" }
                    },
                    "required": ["pattern"]
                },
                "annotations": { "readOnlyHint": false }
            }),
        ]);
    }
    Value::Array(tools)
}

pub fn handle_mcp_request(ws: &Path, read_only: bool, req: &Value) -> Option<Value> {
    let method = req["method"].as_str().unwrap_or("");
    let id = req.get("id").cloned();
    // JSON-RPC notifications have no `id` and expect no reply.
    let id = id?;
    let ok = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    let err = |code: i64, message: &str| {
        Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
    };

    match method {
        "initialize" => {
            let ver = req["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26");
            ok(json!({
                "protocolVersion": ver,
                "capabilities": { "tools": { "listChanged": false } },
                "instructions": MCP_INSTRUCTIONS,
                "serverInfo": { "name": "vx", "version": env!("CARGO_PKG_VERSION") }
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": mcp_tools(read_only) })),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("");
            let mut args = req["params"].get("arguments").cloned().unwrap_or_else(|| json!({}));
            if name == "check" {
                // Background long builds before the host's own tool timeout fires, and let the build
                // itself run far longer than one MCP call.
                if let Some(o) = args.as_object_mut() {
                    o.entry("wait").or_insert(json!(60));
                    if o.get("job").is_none() {
                        o.entry("timeout").or_insert(json!(900));
                    }
                }
            }
            if read_only && !matches!(name, "map" | "view" | "search") {
                return ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: `{name}` is disabled in read-only mode") }],
                    "isError": true
                }));
            }
            let (op, payload) = match name {
                "view" if args["outline"].as_bool() == Some(true) => ("outline", args),
                "map" | "view" | "search" | "apply" | "replace" | "edit" | "symbol" | "check" | "create" | "ast" | "release" => (name, args),
                other => {
                    return ok(json!({
                        "content": [{ "type": "text", "text": format!("Error: unknown tool `{other}`") }],
                        "isError": true
                    }));
                }
            };
            match code_intel::run_op(ws, op, &payload) {
                Ok(text) => {
                    let is_err = matches!(op, "check" | "release") && code_intel::check_failed(&text);
                    ok(json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": is_err
                    }))
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

/// Tools that never write source files. They run on worker threads so a slow `check` (or a big
/// `search`) can't block the requests queued behind it on the single stdio pipe.
fn runs_concurrently(req: &Value) -> bool {
    let p = &req["params"];
    req["method"] == "tools/call"
        && (matches!(p["name"].as_str(), Some("map" | "view" | "search" | "check"))
            // Read-only release stages (preflight can run a long build) must not block the pipe.
            || (p["name"] == "release" && matches!(p["arguments"]["action"].as_str().unwrap_or("status"), "status" | "preflight" | "verify")))
}

fn send(line: &Value) -> bool {
    let Ok(mut s) = serde_json::to_string(line) else { return true };
    s.push('\n');
    let stdout = std::io::stdout();
    let mut out = stdout.lock(); // one lock per message: replies from worker threads never interleave
    out.write_all(s.as_bytes()).is_ok() && out.flush().is_ok()
}

pub fn run_mcp(ws: &Path, read_only: bool) {
    let stdin = std::io::stdin();
    let ws: Arc<std::path::PathBuf> = Arc::new(ws.to_path_buf());
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(t) {
            Ok(req) if runs_concurrently(&req) => {
                workers.retain(|h| !h.is_finished());
                let ws = Arc::clone(&ws);
                workers.push(std::thread::spawn(move || {
                    if let Some(r) = handle_mcp_request(&ws, read_only, &req) {
                        send(&r);
                    }
                }));
            }
            Ok(req) => {
                if let Some(r) = handle_mcp_request(&ws, read_only, &req) {
                    if !send(&r) {
                        break;
                    }
                }
            }
            Err(e) => {
                let r = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                if !send(&r) {
                    break;
                }
            }
        }
    }
    // stdin closed: let in-flight replies finish (piped/scripted use) before the process exits.
    for w in workers {
        let _ = w.join();
    }
}
