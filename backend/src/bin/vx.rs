//! `vx` — Vireo's token-lean code tools for external agent CLIs (Claude Code, Codex, ...).
//! Runs locally against the workspace (no server needed, works inside network-less sandboxes).
//! See `code_intel.rs` for the operations.

#[path = "../code_intel.rs"]
mod code_intel;

use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::{Component, Path, PathBuf};

const USAGE: &str = "vx <map|outline|view|edit|symbol|create|search|ast|check> ...   (vx help for details)\n\
  vx map [query] [--dir D]\n\
  vx outline <file>\n\
  vx view <file> [symbol|A..B]\n\
  vx edit <file> A#h[..B#h] [--insert-after|--insert-before|--delete] [--text T]   (new code on stdin)\n\
  vx symbol <file> <name> [--replace|--insert-after|--delete] [--text T]           (code on stdin)\n\
  vx create <file> [--force] [--text T]                                             (content on stdin)\n\
  vx search <regex> [--glob G]\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]   (-r edits files IN PLACE; --dry-run only lists matches)\n\
  vx check [--cmd C] [--timeout S]";

const HELP: &str = "vx - token-lean code tools: hash-anchored view/edit, symbol ops, repo map, build check\n\
\n\
  vx map [query] [--dir D]        ranked file/symbol map; a query narrows by file or symbol name\n\
  vx outline <file>               symbols as [start#hash..end#hash] ranges (markdown: headings)\n\
  vx view <file> [symbol|A..B]    lines as N#hash|code; target is a function/class name or a line range\n\
  vx search <regex> [--glob G]    regex search, hits as file:N#hash| text (use `--` before a pattern starting with -)\n\
  vx edit <file> A#h[..B#h]       replace those lines with new code from stdin or --text\n\
         [--insert-after|--insert-before|--delete]   (--delete and the inserts take the anchor only)\n\
  vx symbol <file> <name>         print a whole function/class; add --replace|--insert-after|--delete to change it\n\
  vx create <file> [--force]      new file from stdin or --text (--force overwrites)\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
                                  ast-grep structural search; -r rewrites files IN PLACE, --dry-run only lists\n\
  vx check [--cmd C] [--timeout S]   cargo / tsc / go build with deduplicated, anchored diagnostics\n\
\n\
Workflow: map or search -> outline / view (copy the N#hh anchors) -> edit or symbol.\n\
Anchors come from view, outline, search and check output. A stale anchor is re-located when it still\n\
matches uniquely, otherwise the edit is rejected and fresh lines are printed. Edits report the new\n\
anchors and flag new syntax errors, so there is no need to re-read after editing.\n\
Paths are relative to the current directory or the repo root. Errors go to stderr with exit code 1.";

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn stdin_text() -> String {
    let mut s = String::new();
    if !std::io::stdin().is_terminal() {
        let mut bytes = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut bytes);
        s = String::from_utf8_lossy(&bytes).to_string();
        // Windows PowerShell pipes a BOM; it is never part of the code being written.
        if let Some(rest) = s.strip_prefix('\u{feff}') {
            s = rest.to_string();
        }
    }
    s
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        println!("{HELP}");
        return;
    };
    if matches!(cmd, "-h" | "--help" | "help") || args.iter().skip(1).any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
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

    // Split flags from positionals; a bare `--` makes everything after it positional.
    let mut pos: Vec<String> = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let valued = ["--dir", "--text", "--glob", "--cmd", "--timeout", "-r", "--rewrite", "-l", "--lang"];
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
        } else if a.starts_with('-') && a.len() > 1 && !a.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            if valued.contains(&a.as_str()) && i + 1 < args.len() {
                flags.push((a.clone(), Some(args[i + 1].clone())));
                i += 2;
            } else {
                flags.push((a.clone(), None));
                i += 1;
            }
        } else {
            pos.push(a.clone());
            i += 1;
        }
    }
    let flag = |names: &[&str]| -> Option<Option<String>> {
        flags.iter().find(|(k, _)| names.contains(&k.as_str())).map(|(_, v)| v.clone())
    };
    let flag_val = |names: &[&str]| -> Option<String> { flag(names).flatten() };
    let has = |n: &str| flag(&[n]).is_some();
    let need = |idx: usize, what: &str| -> String {
        pos.get(idx).cloned().unwrap_or_else(|| fail(&format!("missing {what}\n{USAGE}")))
    };

    let (op, payload): (&str, Value) = match cmd {
        "outline" => ("outline", json!({ "path": abs(&need(0, "<file>")) })),
        "view" => ("view", json!({ "path": abs(&need(0, "<file>")), "target": pos.get(1) })),
        "map" => (
            "map",
            json!({ "query": if pos.is_empty() { Value::Null } else { json!(pos.join(" ")) }, "dir": flag_val(&["--dir"]).map(|d| abs(&d)) }),
        ),
        "search" => ("search", json!({ "query": need(0, "<regex>"), "glob": flag_val(&["--glob"]) })),
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
            let text = if op == "delete" { String::new() } else { flag_val(&["--text"]).unwrap_or_else(stdin_text) };
            // An empty replacement would silently delete the lines (e.g. stdin was empty or never
            // piped); deleting must be explicit.
            if op == "replace" && text.is_empty() {
                fail("Error: empty replacement text (nothing on stdin and no --text). Pass the new code, or use --delete to remove the lines.");
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
            let code = if matches!(mode, "replace" | "insert_after") { flag_val(&["--text"]).unwrap_or_else(stdin_text) } else { String::new() };
            ("symbol", json!({ "path": abs(&need(0, "<file>")), "symbol": need(1, "<name>"), "mode": mode, "new_code": code }))
        }
        "create" => {
            let content = flag_val(&["--text"]).unwrap_or_else(stdin_text);
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
        "check" => (
            "check",
            json!({ "cmd": flag_val(&["--cmd"]), "timeout": flag_val(&["--timeout"]).and_then(|t| t.parse::<u64>().ok()) }),
        ),
        other => fail(&format!("unknown command `{other}`\n{USAGE}")),
    };

    match code_intel::run_op(&ws, op, &payload) {
        Ok(out) => print!("{}", if out.ends_with('\n') { out } else { format!("{out}\n") }),
        Err(e) => fail(&format!("Error: {e}")),
    }
}
