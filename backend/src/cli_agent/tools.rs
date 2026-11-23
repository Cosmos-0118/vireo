//! Maps each CLI's tool vocabulary onto the tool names the chat UI already renders.

use serde_json::{json, Value};
use std::path::Path;

/// Tool-result `content` is either a string or an array of `{type:"text", text}` blocks.
pub(crate) fn flatten_content(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Workspace-relative, forward-slash path (absolute paths outside the workspace are kept as-is).
pub(crate) fn rel_path(ws: &Path, p: &str) -> String {
    let norm = p.replace('\\', "/");
    let norm = norm.strip_prefix("//?/").unwrap_or(&norm);
    let ws_s = ws.to_string_lossy().replace('\\', "/");
    let ws_s = ws_s.strip_prefix("//?/").unwrap_or(&ws_s).trim_end_matches('/').to_string();
    if !ws_s.is_empty() {
        if let Some(head) = norm.get(..ws_s.len()) {
            if head.eq_ignore_ascii_case(&ws_s) && norm.as_bytes().get(ws_s.len()) == Some(&b'/') {
                return norm[ws_s.len() + 1..].to_string();
            }
        }
    }
    norm.to_string()
}

pub(crate) fn first_path(input: &Value, keys: &[&str], ws: &Path) -> String {
    keys.iter()
        .find_map(|k| input[*k].as_str())
        .filter(|s| !s.is_empty())
        .map(|p| rel_path(ws, p))
        .unwrap_or_default()
}

/// Map Claude Code (and Amp) tool vocabulary onto the names the chat UI already knows
/// (`read_file`, `edit_file`, …) so "View diff" and file refresh keep working.
pub(crate) fn map_claude_tool(name: &str, input: &Value, ws: &Path) -> (String, Value) {
    if let Some(vx_tool) = name.strip_prefix("mcp__vx__") {
        return map_vx_mcp_tool(vx_tool, input, ws);
    }
    let path_of = |key: &str| input[key].as_str().map(|p| rel_path(ws, p)).unwrap_or_default();
    match name {
        "Read" | "read_file" => {
            let p = first_path(input, &["file_path", "path"], ws);
            let mut args = json!({ "path": p });
            if let Some(start) = input["offset"].as_u64() {
                args["start_line"] = json!(start);
                if let Some(limit) = input["limit"].as_u64() {
                    args["end_line"] = json!(start + limit.saturating_sub(1));
                }
            }
            ("read_file".to_string(), args)
        }
        "Edit" | "MultiEdit" | "edit_file" => {
            ("edit_file".to_string(), json!({ "path": first_path(input, &["file_path", "path"], ws) }))
        }
        "NotebookEdit" => ("edit_file".to_string(), json!({ "path": path_of("notebook_path") })),
        "Write" | "write_file" | "create_file" => {
            ("write_file".to_string(), json!({ "path": first_path(input, &["file_path", "path"], ws) }))
        }
        // Claude Code runs shell commands through `PowerShell` on Windows and `Bash` elsewhere.
        "Bash" | "PowerShell" | "bash" => {
            let cmd = input["command"].as_str().or_else(|| input["cmd"].as_str()).unwrap_or("");
            map_vx(cmd, ws).unwrap_or_else(|| {
                let mut args = json!({ "command": cmd });
                // Claude's human-readable intent for the command ("Show working tree status").
                if let Some(d) = input["description"].as_str().map(str::trim).filter(|d| !d.is_empty()) {
                    args["description"] = json!(d);
                }
                ("run_command".to_string(), args)
            })
        }
        "Grep" | "grep" => (
            "search_code".to_string(),
            json!({ "query": input["pattern"].as_str().or_else(|| input["query"].as_str()).unwrap_or("") }),
        ),
        "Glob" | "glob" | "finder" => (
            "list_files".to_string(),
            json!({ "dir": input["pattern"].as_str().or_else(|| input["query"].as_str()).unwrap_or("") }),
        ),
        "LS" => ("list_files".to_string(), json!({ "dir": path_of("path") })),
        other => {
            let mut args = if input.is_object() { input.clone() } else { json!({}) };
            let p = first_path(input, &["file_path", "path"], ws);
            if !p.is_empty() {
                args["path"] = json!(p);
            }
            (other.to_string(), args)
        }
    }
}

/// Map a `vx mcp` tool call onto the chat UI's tool vocabulary (so file refresh, diff view, and
/// checkpoints work identically to native tools).
pub(crate) fn map_vx_mcp_tool(tool: &str, input: &Value, ws: &Path) -> (String, Value) {
    let p = || first_path(input, &["path", "file_path"], ws);
    match tool {
        "map" => ("repo_map".into(), json!({ "query": input["query"].as_str().unwrap_or("") })),
        "view" if input["outline"].as_bool() == Some(true) => ("outline".into(), json!({ "path": p() })),
        "view" => {
            let mut a = json!({ "path": p() });
            if let Some(t) = input["target"].as_str().filter(|s| !s.is_empty()) {
                if let Some((s, e)) = t.split_once("..") {
                    if let Ok(s) = s.parse::<u64>() {
                        a["start_line"] = json!(s);
                    }
                    if let Ok(e) = e.parse::<u64>() {
                        a["end_line"] = json!(e);
                    }
                } else {
                    a["symbol"] = json!(t);
                }
            }
            ("read_file".into(), a)
        }
        "search" => ("search_code".into(), json!({ "query": input["query"].as_str().unwrap_or("") })),
        "edit" => ("edit_file".into(), json!({ "path": p() })),
        // Empty path marks the turn opaque for checkpoints so every file in a multi-file patch/replace is captured.
        "apply" => ("edit_file".into(), json!({ "path": "" })),
        "replace" => ("edit_file".into(), json!({ "path": "" })),
        "symbol" => {
            let mode = input["mode"].as_str().unwrap_or("read");
            let sym = input["symbol"].as_str().unwrap_or("");
            if matches!(mode, "replace" | "insert_after" | "delete") {
                ("symbol_edit".into(), json!({ "path": p(), "symbol": sym, "mode": mode }))
            } else {
                ("read_file".into(), json!({ "path": p(), "symbol": sym }))
            }
        }
        "check" => ("verify".into(), json!({ "command": input["cmd"].as_str().unwrap_or("") })),
        other => (format!("mcp__vx__{other}"), input.clone()),
    }
}

/// Recognise a `vx ...` shell call and present it with the chat UI's tool vocabulary.
pub(crate) fn map_vx(command: &str, ws: &Path) -> Option<(String, Value)> {
    static VX_CALL: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let re = VX_CALL
        .get_or_init(|| {
            regex::Regex::new(r"(?:^|\|\s*|&&\s*|;\s*)vx(?:\.exe)?\s+(\w+)([^\r\n]*)").ok()
        })
        .as_ref()?;
    let c = re.captures(command)?;
    let sub = c.get(1)?.as_str();
    let rest = c.get(2)?.as_str();
    let toks: Vec<String> = rest
        .split_whitespace()
        .map(|t| t.trim_matches(|ch| ch == '"' || ch == '\'').to_string())
        .take_while(|t| !t.starts_with("<<"))
        .collect();
    let pos: Vec<&String> = toks.iter().filter(|t| !t.starts_with('-')).collect();
    let flag = |f: &str| toks.iter().any(|t| t == f);
    let path = |i: usize| pos.get(i).map(|p| rel_path(ws, p)).unwrap_or_default();
    Some(match sub {
        "map" => ("repo_map".into(), json!({ "query": pos.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ") })),
        "outline" => ("outline".into(), json!({ "path": path(0) })),
        "view" => {
            let mut a = json!({ "path": path(0) });
            if let Some(t) = pos.get(1) {
                if let Some((s, e)) = t.split_once("..") {
                    if let Ok(s) = s.parse::<u64>() {
                        a["start_line"] = json!(s);
                    }
                    if let Ok(e) = e.parse::<u64>() {
                        a["end_line"] = json!(e);
                    }
                } else {
                    a["symbol"] = json!(t);
                }
            }
            ("read_file".into(), a)
        }
        "edit" => ("edit_file".into(), json!({ "path": path(0) })),
        "apply" | "replace" => ("edit_file".into(), json!({ "path": "" })),
        "symbol" if flag("--replace") || flag("--insert-after") || flag("--delete") => {
            let mode = if flag("--replace") { "replace" } else if flag("--delete") { "delete" } else { "insert_after" };
            ("symbol_edit".into(), json!({ "path": path(0), "symbol": pos.get(1).map(|s| s.as_str()).unwrap_or(""), "mode": mode }))
        }
        "symbol" => ("read_file".into(), json!({ "path": path(0), "symbol": pos.get(1).map(|s| s.as_str()).unwrap_or("") })),
        "create" => ("write_file".into(), json!({ "path": path(0) })),
        "search" => ("search_code".into(), json!({ "query": pos.first().map(|s| s.as_str()).unwrap_or("") })),
        "ast" => {
            let mut a = json!({ "pattern": pos.first().map(|s| s.as_str()).unwrap_or("") });
            if flag("-r") || flag("--rewrite") {
                a["rewrite"] = json!("…");
            }
            ("ast".into(), a)
        }
        "check" => ("verify".into(), json!({ "command": pos.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ") })),
        _ => return None,
    })
}

/// Codex reports the full shell invocation (`pwsh.exe -Command "<script>"`); show just the script.
pub(crate) fn simplify_command(cmd: &str) -> String {
    for marker in [" -Command ", " -command ", " -lc ", " /c "] {
        if let Some(i) = cmd.find(marker) {
            let rest = cmd[i + marker.len()..].trim();
            let rest = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .unwrap_or(rest);
            return rest.replace("\\\"", "\"");
        }
    }
    cmd.to_string()
}

pub(crate) fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

/// Text of a slash command's local output (`<local-command-stdout>…</local-command-stdout>`),
/// which Claude Code echoes back as a user message.
pub(crate) fn local_command_output(content: &Value) -> Option<String> {
    let raw = match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    for tag in ["local-command-stdout", "local-command-stderr"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let Some((_, rest)) = raw.split_once(&open) {
            let inner = rest.split_once(&close).map_or(rest, |(i, _)| i).trim();
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
        }
    }
    None
}
