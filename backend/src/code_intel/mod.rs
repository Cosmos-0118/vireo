//! Token-lean code intelligence shared by Vireo's built-in agent and the `vx` CLI / MCP server.
//!
//! Pure, synchronous and free of axum/tauri types so the `vx` binary can include it directly.
//!
//! Modules:
//! - [`util`]    – process runner & smart monorepo-aware workspace path resolution
//! - [`doc`]     – hashline document model, anchor parsing/relocation, instant typo snippets
//! - [`symbols`] – tree-sitter languages, syntax validation, outline, repo map, symbol edits
//! - [`view`]    – anchored file viewing (single/multi-range & multi-file batch specs)
//! - [`search`]  – anchor-producing search with smart glob/path recovery, `-C`, `-F`, `-l`
//! - [`edit`]    – anchor edits, file creation, atomic multi-file literal/regex `replace`, `ast`
//! - [`patch`]   – atomic multi-file/multi-hunk V4A + hashline-anchored `apply`
//! - [`check`]   – build/test runner with anchored diagnostics, rustc notes, failing test blocks

macro_rules! lazy_re {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::Regex::new($p).unwrap())
    }};
}

mod astgrep;
mod check;
mod doc;
mod edit;
mod patch;
mod patch_parse;
mod replace;
mod search;
mod symbols;
mod util;
mod view;

#[cfg(test)]
mod tests;

use patch_parse::parse_edge;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[allow(unused_imports)]
pub use util::use_bundled_tools;
#[allow(unused_imports)]
pub use check::{check, check_failed, CheckOpts};
#[allow(unused_imports)]
pub use doc::line_hash;
#[allow(unused_imports)]
pub use edit::{ast, create, edit_batch, hashline_edit};
#[allow(unused_imports)]
pub use patch::apply_patch;
#[allow(unused_imports)]
pub use replace::{replace_text, ReplaceOpts};
#[allow(unused_imports)]
pub use search::{search, SearchOpts};
#[allow(unused_imports)]
pub use symbols::{lang_for, outline, repo_map, symbol_op, symbols, Lang, Sym};
#[allow(unused_imports)]
pub use util::{resolve, set_path_provider};
#[allow(unused_imports)]
pub use view::{split_view_spec, view, view_many};

/// Short usage hint injected into external agent CLIs (kept tiny on purpose: this is the whole
/// per-turn "schema" cost of the vx tools).
pub const AGENT_HINT: &str = "Vireo `vx` is available (as MCP tools `mcp__vx__*` (load via ToolSearch if deferred) and as `vx` on PATH): token-lean code tools.\n\
Prefer `vx` over cat/rg/sed/Python patch scripts. For edits: use `vx apply` (atomic multi-file/multi-hunk patch) or `vx replace` (atomic literal/regex replace across files) instead of shell/Python scripts; use `vx edit` / `vx symbol` (or native Edit) for single-spot tweaks.\n\
- map [query] [--dir D]         ranked repo symbol map\n\
- outline <file>                symbols as [start#hash..end#hash] ranges\n\
- view <file> [symbol|A..B,C..D]  lines as N#hash|code (supports multi-range & batch `a:10..30 b:20..40`)\n\
- search <regex> [path] [-g G] [-C N] [-F] [-l]   anchored grep (smart path/glob resolution, context, literal, files-only)\n\
- apply [--file F] [--dry-run]  atomic multi-file V4A patch (`*** Update File:` + `@@ 12#ab..15#cd`/`@@ after`/`@@ delete` with `+` lines, or classic `@@` context)\n\
- replace <old> <new> [path] [-g G] [-r] [--dry-run] [--expected N]   atomic literal/regex replace across files\n\
- edit <file> A#h[..B#h] [--insert-after|--insert-before|--delete] [--text T|--file F]   (MCP: `start` may be a range `12#ab..15#cd`; `edits:[{start,end?,op?,new_text}]` batches several atomically; `symbol` instead of `start` edits a whole symbol)\n\
- Familiar names work: file_path/old_string/new_string/pattern/content/name. `search` retries as literal text when a regex finds nothing. `view a.rs:Foo` or `Foo:7` reads a symbol.\n\
- symbol <file> <name> [--replace|--insert-after|--delete] [--text T|--file F]\n\
- create <file> [--force] [--text T|--file F]\n\
- ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
- check [--raw] [--warnings] [--timeout S] [cmd ...]   build/test runner with anchored errors, rustc notes/labels, and failing test output\n\
From a shell on Windows, prefer MCP or `--file F` over piping multi-line code. Edits print new anchors and syntax errors immediately; `vx help` has details.";

fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(|v| v.as_str())
}

fn globs_from(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(arr) = args.get("globs").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(g) = item.as_str().filter(|g| !g.trim().is_empty()) {
                out.push(g.to_string());
            }
        }
    }
    if let Some(g) = s(args, "glob").filter(|g| !g.trim().is_empty()) {
        if !out.iter().any(|x| x == g) {
            out.push(g.to_string());
        }
    }
    out
}

/// Single entry point shared by the built-in agent, the `vx` CLI, and the `vx mcp` server.
pub fn run_op(ws: &Path, op: &str, a: &Value) -> Result<String, String> {
    let aliased = with_aliases(op, a);
    let a = &aliased;
    let need = |k: &str| s(a, k).ok_or_else(|| format!("missing `{k}`"));
    let cwd = s(a, "cwd").map(PathBuf::from);
    match op {
        "outline" => outline(ws, need("path")?, cwd.as_deref()),
        "view" => {
            if let Some(specs) = a.get("specs").and_then(|v| v.as_array()) {
                let list: Vec<String> = specs
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                view_many(ws, &list, cwd.as_deref())
            } else {
                // Native `Read`-style `offset`/`limit` become a line range.
                let range = match (s(a, "target"), a["offset"].as_u64()) {
                    (None, Some(off)) => {
                        let start = off.max(1);
                        Some(match a["limit"].as_u64().filter(|l| *l > 0) {
                            Some(l) => format!("{start}..{}", start + l - 1),
                            None => format!("{start}.."),
                        })
                    }
                    _ => None,
                };
                view(ws, need("path")?, range.as_deref().or_else(|| s(a, "target")), cwd.as_deref())
            }
        }
        "map" => repo_map(ws, s(a, "query"), s(a, "dir")),
        "search" => {
            let opts = SearchOpts {
                globs: globs_from(a),
                path: s(a, "path").map(str::to_string),
                cwd,
                literal: a["literal"].as_bool().unwrap_or(false),
                ignore_case: a["ignore_case"].as_bool().unwrap_or(false),
                context: a["context"].as_u64().unwrap_or(0) as usize,
                files_only: a["files_only"].as_bool().unwrap_or(false),
                limit: a["limit"].as_u64().unwrap_or(80) as usize,
            };
            search(ws, need("query")?, &opts)
        }
        "edit" => {
            if let Some(list) = a.get("edits").and_then(|v| v.as_array()) {
                return edit_batch(ws, need("path")?, list, cwd.as_deref());
            }
            // A symbol name instead of anchors: edit the whole symbol.
            if let (None, Some(sym)) = (s(a, "start"), s(a, "symbol")) {
                let mode = match s(a, "op").unwrap_or("replace") {
                    "replace" | "insert_after" | "delete" => s(a, "op").unwrap_or("replace"),
                    other => return Err(format!("op `{other}` isn't supported with `symbol` (replace, insert_after, delete)")),
                };
                return symbol_op(ws, need("path")?, sym, mode, s(a, "new_text").unwrap_or(""), cwd.as_deref());
            }
            // Native `Edit`-style call (`old_string` + `new_string`, no anchor): exact single replace.
            if s(a, "start").is_none() {
                if let Some(old) = ["old_string", "oldString", "old_text", "old"].iter().find_map(|k| s(a, k)) {
                    let opts = ReplaceOpts {
                        path: Some(need("path")?.to_string()),
                        cwd: cwd.clone(),
                        regex: false,
                        dry_run: false,
                        expected: if a["replace_all"].as_bool().unwrap_or(false) { None } else { Some(1) },
                        ..Default::default()
                    };
                    return replace_text(ws, old, s(a, "new_text").unwrap_or(""), &opts);
                }
            }
            // `top` / `bottom` / `imports` need no anchor: same path as a one-entry batch.
            if s(a, "start").and_then(parse_edge).is_some() {
                return edit_batch(ws, need("path")?, std::slice::from_ref(a), cwd.as_deref());
            }
            hashline_edit(
                ws,
                need("path")?,
                s(a, "op").unwrap_or("replace"),
                need("start")?,
                s(a, "end"),
                s(a, "new_text").unwrap_or(""),
                cwd.as_deref(),
            )
        }
        "replace" => {
            let opts = ReplaceOpts {
                path: s(a, "path").map(str::to_string),
                globs: globs_from(a),
                cwd,
                regex: a["regex"].as_bool().unwrap_or(false),
                dry_run: a["dry_run"].as_bool().unwrap_or(false),
                expected: a["expected"].as_u64().map(|n| n as usize),
                all_files: a["all_files"].as_bool().unwrap_or(false),
            };
            replace_text(ws, need("old")?, need("new")?, &opts)
        }
        "symbol" => symbol_op(
            ws,
            need("path")?,
            need("symbol")?,
            s(a, "mode").unwrap_or("read"),
            s(a, "new_code").unwrap_or(""),
            cwd.as_deref(),
        ),
        "create" => create(
            ws,
            need("path")?,
            s(a, "content").unwrap_or(""),
            a["force"].as_bool().unwrap_or(false),
            cwd.as_deref(),
        ),
        "ast" => ast(
            ws,
            need("pattern")?,
            s(a, "lang"),
            s(a, "path"),
            s(a, "rewrite"),
            a["dry_run"].as_bool().unwrap_or(false),
            cwd.as_deref(),
        ),
        "apply" => apply_patch(ws, need("patch")?, cwd.as_deref(), a["dry_run"].as_bool().unwrap_or(false)),
        "check" => check(
            ws,
            s(a, "cmd"),
            a["timeout"].as_u64().unwrap_or(180),
            CheckOpts {
                raw: a["raw"].as_bool().unwrap_or(false),
                warnings: a["warnings"].as_bool().unwrap_or(false),
                dir: s(a, "dir").map(str::to_string),
                cwd: cwd.clone(),
            },
        ),
        other => Err(format!("unknown op `{other}`")),
    }
}

/// Convenience for callers that want a plain string (errors prefixed with `Error:`).
/// Used by the in-app agent only, not by the `vx` bin.
#[allow(dead_code)]
pub fn run_op_string(ws: &Path, op: &str, a: &Value) -> String {
    match run_op(ws, op, a) {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

#[allow(dead_code)]
pub fn describe() -> Value {
    json!({ "hint": AGENT_HINT })
}

/// Accepts the parameter names other agents' tools use (Claude Code, OpenCode, Cursor, ...) so a
/// first call with a familiar name works. A canonical name always wins when both are given.
fn with_aliases(op: &str, a: &Value) -> Value {
    let Some(obj) = a.as_object() else { return a.clone() };
    let table: &[(&str, &[&str])] = match op {
        "outline" => &[("path", &["file_path", "filePath", "file"])],
        "view" => &[("path", &["file_path", "filePath", "file"]), ("target", &["range", "lines", "symbol"])],
        "search" => &[("query", &["pattern", "regex", "q"]), ("glob", &["include"]), ("path", &["dir", "directory"])],
        "map" => &[("dir", &["path"])],
        "edit" => &[
            ("path", &["file_path", "filePath", "file"]),
            ("new_text", &["new_string", "newString", "new_code", "content", "text", "replacement"]),
            ("op", &["mode"]),
        ],
        "replace" => &[
            ("path", &["file_path", "filePath", "file"]),
            ("old", &["old_string", "oldString", "old_text", "search"]),
            ("new", &["new_string", "newString", "new_text", "replacement"]),
        ],
        "symbol" => &[
            ("path", &["file_path", "filePath", "file"]),
            ("symbol", &["name"]),
            ("mode", &["op"]),
            ("new_code", &["new_text", "new_string", "newString", "content", "text"]),
        ],
        "create" => &[("path", &["file_path", "filePath", "file"]), ("content", &["new_text", "text", "file_text"])],
        "ast" => &[("pattern", &["query"]), ("rewrite", &["replacement"]), ("lang", &["language"])],
        "apply" => &[("patch", &["input", "diff"])],
        "check" => &[("cmd", &["command"])],
        _ => &[],
    };
    let mut out = obj.clone();
    for (canon, alts) in table {
        if out.get(*canon).map_or(true, Value::is_null) {
            if let Some(v) = alts.iter().find_map(|k| obj.get(*k).filter(|v| !v.is_null())) {
                out.insert((*canon).to_string(), v.clone());
            }
        }
    }
    Value::Object(out)
}
