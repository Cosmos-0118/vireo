//! `vx` — Vireo's token-lean code tools for external agent CLIs (Claude Code, Codex, ...).
//! Runs locally against the workspace (no server needed, works inside network-less sandboxes).
//! See `code_intel.rs` for the operations.

#[path = "../code_intel.rs"]
mod code_intel;

use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

const USAGE: &str = "vx <map|outline|view|edit|symbol|create|search|ast|check> ...\n\
  vx map [query] [--dir D]\n\
  vx outline <file>\n\
  vx view <file> [symbol|A..B]\n\
  vx edit <file> A#h[..B#h] [--insert-after|--insert-before|--delete] [--text T]   (new code on stdin)\n\
  vx symbol <file> <name> [--replace|--insert-after|--delete] [--text T]           (code on stdin)\n\
  vx create <file> [--force]                                                       (content on stdin)\n\
  vx search <regex> [--glob G]\n\
  vx ast '<pattern>' [-r '<rewrite>'] [-l lang] [path]\n\
  vx check [--cmd C] [--timeout S]";

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
    }
    s
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else { fail(USAGE) };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ws = std::env::var_os("VIREO_WORKSPACE").map(PathBuf::from).unwrap_or_else(|| cwd.clone());
    // Paths are given relative to the shell's cwd; make them absolute so the core can map them
    // into the workspace even when the agent `cd`'d into a subfolder.
    let abs = |p: &str| -> String {
        let pb = PathBuf::from(p);
        if pb.is_absolute() { p.to_string() } else { cwd.join(pb).to_string_lossy().to_string() }
    };

    // Split flags from positionals.
    let mut pos: Vec<String> = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let valued = ["--dir", "--text", "--glob", "--cmd", "--timeout", "-r", "--rewrite", "-l", "--lang"];
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with('-') && a.len() > 1 && !a.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
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
            }),
        ),
        "check" => (
            "check",
            json!({ "cmd": flag_val(&["--cmd"]), "timeout": flag_val(&["--timeout"]).and_then(|t| t.parse::<u64>().ok()) }),
        ),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return;
        }
        _ => fail(USAGE),
    };

    match code_intel::run_op(&ws, op, &payload) {
        Ok(out) => print!("{}", if out.ends_with('\n') { out } else { format!("{out}\n") }),
        Err(e) => fail(&format!("Error: {e}")),
    }
}
