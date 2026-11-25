//! Zero-dependency JSON-RPC 2.0 stdio MCP server exposing Vireo's `vx` tools.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, RwLock};
use std::path::{Path, PathBuf};

use super::code_intel;

/// Sent in the MCP `initialize` reply: exact argument shapes, so a host that lazy-loads tool
/// schemas needs no extra round trip to learn them.
const MCP_INSTRUCTIONS: &str = r##"vx tool arguments (JSON). Anchors `N#hh` come from view/search/edit output; edits return fresh ones, so never re-view after editing.
- map {query?, dir?} · search {query, path?, glob?, literal?, ignore_case?, context?, files_only?}
- view {path, target?: "10..50" | "10..30,80..100" | "Symbol", outline?: true} or {specs: ["a.rs:10..50", "b.rs:Foo"], outline?: true}
Edit, pick one:
- by anchor: edit {path, start: "12#ab..15#cd" | "top" | "bottom" | "imports", new_text, op?: replace|insert_after|insert_before|delete}
- by exact text (no view): edit {path, old_text, new_text}
- by symbol (no view): edit {path, symbol: "Foo::bar", new_text, op?: replace|insert_after|insert_before|delete}
- many at once: edit {edits: [{path?, start | old_text | symbol, new_text?, op?}]} (any mix, any files, atomic) · replace {rules: [{old, new, regex?}], path?, glob?} · apply {patch}
- replace {old, new, path?, glob?, regex?, expected?, dry_run?}: literal by default; regex replacement uses $1 / ${1}
- apply {patch: "*** Update File: a.rs\n@@ 12#ab..15#cd\n+new line"}; other hunks: @@ symbol Foo::bar (+lines replace it) · @@ after symbol Foo · @@ delete symbol Foo · @@ top|bottom|after imports
- check {cmd?, dir?, warnings?}: a build still running after 60s returns a job id; call check {job} to wait.
- release {action?: status|preflight|notes|bump|publish|verify|activity, version?, dry_run?, ...}: `status` (default) says what to do next; always publish {dry_run:true} first."##;

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
                "description": "Read files as N#hash|code: one file (full, range A..B, ranges A..B,C..D, or Symbol) or several via `specs` (e.g. [\"a.ts:10..30\", \"b.ts:foo\"]); the output budget is shared fairly, so every file gets shown. `outline: true` (with `path` or `specs`) lists symbols with [start#h..end#h] ranges. Unique path suffixes auto-resolve.",
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
                "description": "Apply a multi-file V4A patch atomically (`*** Update File:` / `*** Add File:` / `*** Delete File:`). Hunks: anchored `@@ 12#ab..15#cd`, `@@ after 12#ab`, `@@ before 12#ab`, `@@ delete 12#ab..15#cd` (write ONLY `+` lines; the anchors name the replaced lines); by symbol `@@ symbol Foo::bar` (+lines replace it), `@@ after symbol Foo`, `@@ before symbol Foo`, `@@ delete symbol Foo`; anchor-free `@@ top`, `@@ bottom`, `@@ after imports` (skips imports already present); classic `@@` context hunks (` `, `-`, `+`). Anchors refer to pre-patch lines. Paths auto-resolve unique suffixes.",
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
                "description": "Atomic find-and-replace across a file, directory or glob: literal by default, regex=true for patterns (replacement groups $1 / ${1}; \\1 also works; ^/$ match per line). `rules: [{old, new, regex?}]` batches several. Preserves CRLF, checks syntax, supports dry_run and expected count.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "rules": { "type": "array", "description": "Several replacements in ONE atomic pass (applied in order to each file): [{old, new, regex?}]. Use instead of `old`/`new`", "items": { "type": "object" } },
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
                    "required": []
                },
                "annotations": { "readOnlyHint": false }
            }),
            json!({
                "name": "edit",
                "description": "Edit by anchor (`start`), exact text (`old_text`) or symbol name (`symbol`); batch many in `edits` (any mix, any files). Returns fresh anchors and any new syntax errors with the offending lines. Paths auto-resolve unique suffixes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start": { "type": "string", "description": "Anchor `12#ab`, a whole range `12#ab..15#cd` (as printed by view/outline), or `top` / `bottom` / `imports` to insert new_text there without viewing first (`imports` skips lines already present)" },
                        "end": { "type": "string", "description": "Optional end anchor when `start` is a single anchor" },
                        "op": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"], "description": "Default replace (needs new_text); use delete to remove lines" },
                        "new_text": { "type": "string", "description": "Replacement / inserted text" },
                        "old_text": { "type": "string", "description": "No anchors yet (e.g. you read the file another way)? Pass the exact text to replace instead of `start`; it must occur once. Returns fresh N#hash anchors for follow-up edits" },
                        "edits": { "type": "array", "description": "Several edits in ONE atomic call; anchors refer to the files as they are now. Each: {path?, start?, end?, op?, new_text?} or {path?, old_text, new_text} or {path?, symbol, op?, new_text?}; a per-edit `path` makes one call span files (the top-level `path` is the default)", "items": { "type": "object" } },
                        "symbol": { "type": "string", "description": "Instead of anchors: edit a whole symbol by name (op replace / insert_after / delete)" }
                    },
                    "required": []
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
                "description": "Release engine (Rust/Node/Tauri/.NET/Python/Java/Go). `status` (default) shows stacks, version drift, commits since the last tag and the exact next command. Others: preflight, notes (write:true saves an editable draft), bump, publish, verify, activity. Always publish {dry_run:true} first and show the plan; every action shows live on the user's Releases page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["status", "preflight", "notes", "bump", "publish", "verify", "activity"] },
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
            if name == "release" {
                // Credited to the agent in the Releases page activity feed.
                if let Some(o) = args.as_object_mut() {
                    o.insert("source".into(), json!("agent"));
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
            || (p["name"] == "release" && matches!(p["arguments"]["action"].as_str().unwrap_or("status"), "status" | "preflight" | "verify" | "activity")))
}

fn send(line: &Value) -> bool {
    let Ok(mut s) = serde_json::to_string(line) else { return true };
    s.push('\n');
    let stdout = std::io::stdout();
    let mut out = stdout.lock(); // one lock per message: replies from worker threads never interleave
    out.write_all(s.as_bytes()).is_ok() && out.flush().is_ok()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///c%3A/Users/me/repo` -> `c:/Users/me/repo`; `file:///home/me/repo` -> `/home/me/repo`.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let dec = percent_decode(rest);
    let b = dec.as_bytes();
    // `/c:/x` (Windows drive) -> `c:/x`.
    let p = if b.len() >= 3 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() { &dec[1..] } else { &dec[..] };
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// The folder a client says it is working in: `workspaceFolders[0]`, `rootUri`, or `rootPath`.
fn init_workspace(params: &Value) -> Option<PathBuf> {
    let from_uri = params["workspaceFolders"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|f| f["uri"].as_str())
        .or_else(|| params["rootUri"].as_str())
        .and_then(uri_to_path);
    from_uri
        .or_else(|| params["rootPath"].as_str().map(PathBuf::from))
        .filter(|p| p.is_absolute() && p.is_dir())
}

/// Nearest enclosing git root of `p` (a file or folder), else the folder itself.
fn repo_root_of(p: &Path) -> Option<PathBuf> {
    let dir = if p.is_dir() { p } else { p.parent()? };
    if !dir.is_dir() {
        return None;
    }
    Some(dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf())
}

/// The first absolute `path`/`dir`/`cwd`/`specs[0]` of a tool call, if any.
fn absolute_target(args: &Value) -> Option<PathBuf> {
    let spec0 = args["specs"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).map(|s| code_intel::split_view_spec(s).0.to_string());
    ["path", "dir", "cwd"]
        .iter()
        .find_map(|k| args[*k].as_str().map(str::to_string))
        .or(spec0)
        .map(|s| PathBuf::from(s.trim().trim_matches('"')))
        .filter(|p| p.is_absolute())
}

/// Follow the client's root on `initialize`, and (while the launch folder isn't a git repo) the
/// repo of the first absolute path a tool call names, so an IDE-spawned server isn't stuck in its
/// install folder.
fn adopt_workspace(ws: &RwLock<PathBuf>, req: &Value) {
    let target = match req["method"].as_str() {
        Some("initialize") => init_workspace(&req["params"]),
        Some("tools/call") => {
            let cur = ws.read().map(|w| w.clone()).unwrap_or_default();
            if cur.join(".git").exists() {
                return;
            }
            absolute_target(&req["params"]["arguments"])
                .filter(|p| !code_intel::resolve(&cur, &p.to_string_lossy()).is_ok())
                .and_then(|p| repo_root_of(&p))
        }
        _ => None,
    };
    if let (Some(t), Ok(mut w)) = (target, ws.write()) {
        *w = t;
    }
}

/// `locked`: the workspace came from `--workspace`/`VIREO_WORKSPACE`, so the client's roots and
/// absolute paths never move it. Otherwise it follows the client's `initialize` root, and a
/// launch directory that isn't a git repo (an IDE's install folder) rebinds to the repo that
/// the first absolute path in a tool call lives in.
pub fn run_mcp(ws: &Path, read_only: bool, locked: bool) {
    let stdin = std::io::stdin();
    let ws: Arc<RwLock<PathBuf>> = Arc::new(RwLock::new(ws.to_path_buf()));
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(t);
        if let (Ok(req), false) = (&parsed, locked) {
            adopt_workspace(&ws, req);
        }
        let cur: PathBuf = ws.read().map(|w| w.clone()).unwrap_or_default();
        match parsed {
            Ok(req) if runs_concurrently(&req) => {
                workers.retain(|h| !h.is_finished());
                workers.push(std::thread::spawn(move || {
                    if let Some(r) = handle_mcp_request(&cur, read_only, &req) {
                        send(&r);
                    }
                }));
            }
            Ok(req) => {
                if let Some(r) = handle_mcp_request(&cur, read_only, &req) {
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

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(uri_to_path("file:///c%3A/Users/me/my%20repo"), Some(PathBuf::from("c:/Users/me/my repo")));
        assert_eq!(uri_to_path("file:///C:/x"), Some(PathBuf::from("C:/x")));
        assert_eq!(uri_to_path("file:///home/me/repo"), Some(PathBuf::from("/home/me/repo")));
        assert_eq!(uri_to_path("https://example.com"), None);
    }

    #[test]
    fn unlocked_server_follows_the_client_root_and_absolute_paths() {
        let base = std::env::temp_dir().join(format!("vx-mcp-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let launch = base.join("ide-install");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::create_dir_all(&launch).unwrap();

        // `initialize` with a root URI moves the workspace.
        let ws = RwLock::new(launch.clone());
        let uri = format!("file:///{}", repo.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
        adopt_workspace(&ws, &json!({"method": "initialize", "params": {"rootUri": uri}}));
        assert_eq!(ws.read().unwrap().to_string_lossy().replace('\\', "/").to_lowercase(), repo.to_string_lossy().replace('\\', "/").to_lowercase());

        // Without a root, the first absolute path in a tool call selects the enclosing git repo.
        let ws = RwLock::new(launch.clone());
        let file = repo.join("src/a.rs");
        adopt_workspace(&ws, &json!({"method": "tools/call", "params": {"name": "view", "arguments": {"path": file.to_string_lossy()}}}));
        assert_eq!(*ws.read().unwrap(), repo);

        // A workspace that is already a git repo never moves.
        let ws = RwLock::new(repo.clone());
        adopt_workspace(&ws, &json!({"method": "tools/call", "params": {"name": "map", "arguments": {"dir": launch.to_string_lossy()}}}));
        assert_eq!(*ws.read().unwrap(), repo);
        let _ = std::fs::remove_dir_all(&base);
    }
}
