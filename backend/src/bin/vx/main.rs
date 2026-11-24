//! `vx` — Vireo's token-lean code tools for external agent CLIs (Claude Code, Codex, ...).
//! Runs locally against the workspace (no server needed, works inside network-less sandboxes).

#[path = "../../code_intel/mod.rs"]
mod code_intel;

mod help;
mod mcp;

use help::{HELP, HELP_APPLY, USAGE};
use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::{Component, Path, PathBuf};

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
    } else if bytes.len() >= 4
        && bytes.len() % 2 == 0
        && {
            let odd_nuls = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
            let even_nuls = bytes.iter().step_by(2).filter(|b| **b == 0).count();
            odd_nuls * 10 >= bytes.len() / 2 * 9 || even_nuls * 10 >= bytes.len() / 2 * 9
        }
    {
        let le = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count()
            >= bytes.iter().step_by(2).filter(|b| **b == 0).count();
        utf16(bytes, le)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
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

fn read_payload_file(path: &str) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| fail(&format!("Error: cannot read --file `{path}`: {e}")));
    decode_input(&bytes)
}

/// Lexically resolve `.` and `..` without touching the filesystem (works for not-yet-created files).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn flat(p: &Path) -> String {
    let s = std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_lowercase()
}

fn is_inside(dir: &Path, root: &Path) -> bool {
    let (d, r) = (flat(dir), flat(root));
    d == r || d.starts_with(&format!("{r}/"))
}

/// The workspace is `VIREO_WORKSPACE` when the shell is inside it, else the enclosing git repo,
/// else the current directory. This lets `vx` work from any subfolder of a repo.
fn find_workspace(cwd: &Path) -> PathBuf {
    if let Some(w) = std::env::var_os("VIREO_WORKSPACE")
        .map(PathBuf::from)
        .filter(|w| !w.as_os_str().is_empty())
    {
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

/// True when `a` is shaped like a CLI flag (`-x`, `--long-name`, `--name=value`). Dash-prefixed
/// text containing regex or punctuation (`--foo|--bar`, `-\d+`) is a pattern, not a flag.
fn looks_like_flag(a: &str) -> bool {
    let name = a.split_once('=').map_or(a, |(k, _)| k);
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn flag_takes_value(cmd: &str, flag: &str) -> bool {
    match (cmd, flag) {
        ("check", "-w") => false,
        ("search", "-l") => false,
        ("replace", "-r") => false,
        _ => matches!(
            flag,
            "--dir"
                | "--text"
                | "--file"
                | "-f"
                | "--glob"
                | "-g"
                | "--path"
                | "-p"
                | "--context"
                | "-C"
                | "--limit"
                | "--max"
                | "--expected"
                | "--old-file"
                | "--new-file"
                | "--cmd"
                | "--timeout"
                | "-r"
                | "--rewrite"
                | "-l"
                | "--lang"
                | "--workspace"
                | "-w"
        ),
    }
}

fn main() {
    // `vx` also runs from plain terminals (installer PATH task), so find the bundled rg/git too.
    code_intel::use_bundled_tools();
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
        } else if a.starts_with('-')
            && a.len() > 1
            && !a.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false)
            && looks_like_flag(a)
        {
            // Support `--flag=value` syntax too!
            if let Some((k, v)) = a.split_once('=') {
                flags.push((k.to_string(), Some(v.to_string())));
                i += 1;
            } else if flag_takes_value(cmd, a) && i + 1 < args.len() {
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
            "-g" => "--glob",
            "-p" => "--path",
            "--dir" if matches!(cmd, "search" | "replace") => "--path",
            "-F" | "--fixed-strings" => "--literal",
            "-i" => "--ignore-case",
            "-C" => "--context",
            "-l" | "--files-with-matches" if cmd == "search" => "--files",
            "--max" => "--limit",
            "-r" if cmd == "replace" => "--regex",
            "-w" if cmd == "check" => "--warnings",
            "-w" if cmd == "mcp" => "--workspace",
            _ => continue,
        };
        *name = canon.to_string();
    }

    let allowed: &[&str] = match cmd {
        "outline" | "view" => &[],
        "map" => &["--dir"],
        "search" => &["--glob", "--path", "--literal", "--ignore-case", "--context", "--files", "--limit"],
        "apply" => &["--file", "--dry-run"],
        "replace" => &["--path", "--glob", "--regex", "--dry-run", "--expected", "--all-files", "--old-file", "--new-file"],
        "edit" => &["--text", "--file", "--delete", "--insert-after", "--insert-before"],
        "symbol" => &["--text", "--file", "--replace", "--insert-after", "--delete"],
        "create" => &["--text", "--file", "--force"],
        "ast" => &["-r", "--rewrite", "-l", "--lang", "--dry-run"],
        "check" => &["--cmd", "--timeout", "--raw", "--warnings", "--dir"],
        "mcp" => &["--workspace", "--read-only"],
        _ => &[],
    };
    for (name, _) in &flags {
        if !allowed.contains(&name.as_str()) {
            let hint = allowed
                .iter()
                .copied()
                .find(|a| a.trim_start_matches('-').starts_with(name.trim_start_matches('-')) && name.len() > 2);
            let list = if allowed.is_empty() { "no flags".to_string() } else { allowed.join(" ") };
            let did = hint.map(|h| format!(" Did you mean `{h}`?")).unwrap_or_default();
            let dashes = if matches!(cmd, "search" | "replace") {
                " To search for text that starts with dashes, put `--` before it."
            } else {
                ""
            };
            fail(&format!(
                "Error: unknown flag `{name}` for `vx {cmd}` (accepts: {list}).{did}{dashes} Nothing was changed.\n{USAGE}"
            ));
        }
    }
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
    let flag_vals = |name: &str| -> Vec<String> {
        flags
            .iter()
            .filter(|(k, _)| k == name)
            .filter_map(|(_, v)| v.clone())
            .collect()
    };
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
        // An MCP server's cwd is whatever the host launched it from (often the IDE's install
        // folder), so `VIREO_WORKSPACE` wins here even when cwd isn't inside it.
        let target_ws = flag_val(&["--workspace"])
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("VIREO_WORKSPACE").map(PathBuf::from).filter(|w| w.is_dir()))
            .unwrap_or(ws);
        mcp::run_mcp(&target_ws, has("--read-only"));
        return;
    }

    let cwd_s = cwd.to_string_lossy().to_string();

    let (op, payload): (&str, Value) = match cmd {
        "outline" => ("outline", json!({ "path": need(0, "<file>"), "cwd": cwd_s })),
        "view" => {
            if pos.is_empty() {
                fail(&format!("missing <file>\n{USAGE}"));
            }
            if pos.len() == 1 {
                let (p, t) = code_intel::split_view_spec(&pos[0]);
                ("view", json!({ "path": p, "target": t, "cwd": cwd_s }))
            } else if pos.len() == 2 {
                let (_, t0) = code_intel::split_view_spec(&pos[0]);
                let (p1, t1) = code_intel::split_view_spec(&pos[1]);
                let second_is_file = t1.is_some()
                    || cwd.join(p1).is_file()
                    || ws.join(p1).is_file();
                if t0.is_none() && !second_is_file {
                    ("view", json!({ "path": pos[0], "target": pos[1], "cwd": cwd_s }))
                } else {
                    ("view", json!({ "specs": pos, "cwd": cwd_s }))
                }
            } else {
                ("view", json!({ "specs": pos, "cwd": cwd_s }))
            }
        }
        "map" => (
            "map",
            json!({
                "query": if pos.is_empty() { Value::Null } else { json!(pos.join(" ")) },
                "dir": flag_val(&["--dir"]),
            }),
        ),
        "search" => {
            let query = need(0, "<regex>");
            let path = flag_val(&["--path"]).or_else(|| pos.get(1).cloned());
            let globs = flag_vals("--glob");
            (
                "search",
                json!({
                    "query": query,
                    "path": path,
                    "globs": globs,
                    "cwd": cwd_s,
                    "literal": has("--literal"),
                    "ignore_case": has("--ignore-case"),
                    "context": flag_val(&["--context"]).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0),
                    "files_only": has("--files"),
                    "limit": flag_val(&["--limit"]).and_then(|v| v.parse::<u64>().ok()).unwrap_or(80),
                }),
            )
        }
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
            ("apply", json!({ "patch": patch, "cwd": cwd_s, "dry_run": has("--dry-run") }))
        }
        "replace" => {
            let mut pi = 0usize;
            let old = if let Some(opt) = flag(&["--old-file"]) {
                let p = opt.unwrap_or_else(|| fail("Error: --old-file requires a path."));
                read_payload_file(&p)
            } else {
                let v = need(pi, "<old>");
                pi += 1;
                v
            };
            let new = if let Some(opt) = flag(&["--new-file"]) {
                let p = opt.unwrap_or_else(|| fail("Error: --new-file requires a path."));
                read_payload_file(&p)
            } else {
                let v = need(pi, "<new>");
                pi += 1;
                v
            };
            let path = flag_val(&["--path"]).or_else(|| pos.get(pi).cloned());
            let globs = flag_vals("--glob");
            (
                "replace",
                json!({
                    "old": old,
                    "new": new,
                    "path": path,
                    "globs": globs,
                    "cwd": cwd_s,
                    "regex": has("--regex"),
                    "dry_run": has("--dry-run"),
                    "expected": flag_val(&["--expected"]).and_then(|v| v.parse::<u64>().ok()),
                    "all_files": has("--all-files"),
                }),
            )
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
                    "dir": flag_val(&["--dir"]),
                    "cwd": cwd_s,
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
    use super::mcp::handle_mcp_request;
    use super::*;

    #[test]
    fn decode_input_handles_utf8_bom_and_utf16() {
        assert_eq!(decode_input(b"\xEF\xBB\xBFhello\n"), "hello\n");
        let mut le_bom = vec![0xFF, 0xFE];
        for u in "hi—✓".encode_utf16() {
            le_bom.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_input(&le_bom), "hi—✓");
        let mut be_bom = vec![0xFE, 0xFF];
        for u in "abc".encode_utf16() {
            be_bom.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_input(&be_bom), "abc");
        let mut le_raw = Vec::new();
        for u in "fn main() {}".encode_utf16() {
            le_raw.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_input(&le_raw), "fn main() {}");
    }

    #[test]
    fn dash_prefixed_regex_is_not_a_flag() {
        assert!(looks_like_flag("--glob"));
        assert!(looks_like_flag("-C"));
        assert!(looks_like_flag("--limit=5"));
        assert!(!looks_like_flag("--success|--warning"));
        assert!(!looks_like_flag("-(a|b)+"));
        assert!(!looks_like_flag("--foo.bar"));
    }
    #[test]
    fn mcp_create_ast_outline_check_and_aliases() {
        let ws = std::env::temp_dir().join(format!("vireo-mcp2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let call = |id: u64, name: &str, args: serde_json::Value| {
            let r = handle_mcp_request(
                &ws,
                false,
                &json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}),
            )
            .unwrap();
            (r["result"]["isError"].as_bool().unwrap(), r["result"]["content"][0]["text"].as_str().unwrap().to_string())
        };
        let list = handle_mcp_request(&ws, false, &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"create") && names.contains(&"ast") && names.contains(&"symbol"), "{names:?}");

        // create, using agent-style parameter names.
        let (err, t) = call(2, "create", json!({"file_path":"m.rs","content":"fn old_name() {\n    old(1);\n}\n"}));
        assert!(!err, "{t}");
        // outline through `view`.
        let (err, t) = call(3, "view", json!({"path":"m.rs","outline":true}));
        assert!(!err && t.contains("old_name"), "{t}");
        // ast rewrite.
        let (err, t) = call(4, "ast", json!({"pattern":"old($X)","lang":"rust","rewrite":"new($X)"}));
        assert!(!err, "{t}");
        assert!(std::fs::read_to_string(ws.join("m.rs")).unwrap().contains("new(1)"));
        // symbol read, familiar `name` alias.
        let (err, t) = call(5, "symbol", json!({"path":"m.rs","name":"old_name"}));
        assert!(!err && t.contains("new(1)"), "{t}");
        // failing custom check surfaces isError.
        let (err, _) = call(6, "check", json!({"cmd":"exit 3"}));
        assert!(err);
        // unknown tool and missing params are errors, not panics.
        assert!(call(7, "nope", json!({})).0);
        assert!(call(8, "view", json!({"path":"missing.rs"})).0);
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
        assert!(handle_mcp_request(&ws, false, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());

        let list = handle_mcp_request(&ws, false, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(
            names.contains(&"view")
                && names.contains(&"apply")
                && names.contains(&"replace")
                && names.contains(&"check"),
            "{names:?}"
        );

        let ro_list = handle_mcp_request(&ws, true, &json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})).unwrap();
        let ro_names: Vec<&str> = ro_list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(!ro_names.contains(&"apply") && !ro_names.contains(&"replace") && ro_names.contains(&"view"), "{ro_names:?}");

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

        // Replace via MCP works cleanly.
        let r = handle_mcp_request(
            &ws,
            false,
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"replace","arguments":{"old":"42","new":"99","path":"a.rs"}}}),
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("99"));

        let blocked = handle_mcp_request(
            &ws,
            true,
            &json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"replace","arguments":{"old":"a","new":"b","path":"a.rs"}}}),
        )
        .unwrap();
        assert_eq!(blocked["result"]["isError"], true);
    }
}
