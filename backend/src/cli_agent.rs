//! Runs an installed, already-logged-in agent CLI (Claude Code, Codex) headlessly and translates
//! its NDJSON event stream into Vireo's agent SSE events (`text`, `tool_call`, `tool_result`,
//! `checkpoint`, `error`). The CLI keeps using its own credentials and config; Vireo only passes
//! flags on the command line and never reads or writes the CLI's auth files.

use crate::agent::{send_sse, AgentChatReq, ChatMessage};
use crate::git::create_checkpoint_internal;
use crate::state::{background_command, AppState};
use axum::response::sse::Event;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Largest slice of earlier conversation sent along with the new request.
const MAX_HISTORY_CHARS: usize = 40_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CliKind {
    Claude,
    Codex,
}

impl CliKind {
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "claude-cli" => Some(Self::Claude),
            "codex-cli" => Some(Self::Codex),
            _ => None,
        }
    }

    fn cmd(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    fn install_hint(self) -> &'static str {
        match self {
            Self::Claude => "irm https://claude.ai/install.ps1 | iex   (macOS/Linux: curl -fsSL https://claude.ai/install.sh | bash)",
            Self::Codex => "npm install -g @openai/codex",
        }
    }
}

/// One normalized event destined for the chat timeline.
#[derive(Debug, PartialEq)]
pub enum Out {
    Text(String),
    ToolCall { id: String, name: String, args: Value },
    ToolResult { id: String, name: String, result: String },
    Error(String),
}

/// Stateful translator from a CLI's JSON lines to [`Out`] events.
pub struct Normalizer {
    kind: CliKind,
    ws: PathBuf,
    /// tool-call id -> normalized tool name (so results can be labelled).
    names: HashMap<String, String>,
    started: HashSet<String>,
    /// The CLI reported a final result / turn completion.
    pub finished: bool,
    /// An error has already been surfaced to the user.
    pub errored: bool,
}

impl Normalizer {
    pub fn new(kind: CliKind, ws: &Path) -> Self {
        Self {
            kind,
            ws: ws.to_path_buf(),
            names: HashMap::new(),
            started: HashSet::new(),
            finished: false,
            errored: false,
        }
    }

    pub fn feed(&mut self, v: &Value) -> Vec<Out> {
        let out = match self.kind {
            CliKind::Claude => self.feed_claude(v),
            CliKind::Codex => self.feed_codex(v),
        };
        if out.iter().any(|o| matches!(o, Out::Error(_))) {
            self.errored = true;
        }
        out
    }

    fn feed_claude(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("assistant") => {
                let msg = &v["message"];
                // Claude Code fabricates "synthetic" assistant messages for errors such as
                // "Not logged in"; the matching `result` event reports them, so skip the echo.
                if msg["model"].as_str() == Some("<synthetic>") {
                    return out;
                }
                for block in msg["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                out.push(Out::Text(t.to_string()));
                            }
                        }
                        Some("tool_use") => {
                            let id = block["id"].as_str().unwrap_or("tool").to_string();
                            let (name, args) = map_claude_tool(
                                block["name"].as_str().unwrap_or(""),
                                &block["input"],
                                &self.ws,
                            );
                            self.names.insert(id.clone(), name.clone());
                            out.push(Out::ToolCall { id, name, args });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"].as_str() != Some("tool_result") {
                        continue;
                    }
                    let id = block["tool_use_id"].as_str().unwrap_or("tool").to_string();
                    let name = self.names.get(&id).cloned().unwrap_or_else(|| "tool".to_string());
                    let mut text = flatten_content(&block["content"]);
                    if block["is_error"].as_bool() == Some(true) && !text.starts_with("Error") {
                        text = format!("Error: {text}");
                    }
                    out.push(Out::ToolResult { id, name, result: text });
                }
            }
            Some("result") => {
                self.finished = true;
                if v["is_error"].as_bool() == Some(true) {
                    let msg = v["result"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .or_else(|| v["subtype"].as_str().map(|s| format!("Claude Code stopped: {s}")))
                        .unwrap_or_else(|| "Claude Code reported an error.".to_string());
                    out.push(Out::Error(msg));
                }
            }
            _ => {}
        }
        out
    }

    fn feed_codex(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("item.started") | Some("item.completed") => {
                let completed = v["type"].as_str() == Some("item.completed");
                let item = &v["item"];
                let id = item["id"].as_str().unwrap_or("item").to_string();
                match item["type"].as_str() {
                    Some("agent_message") if completed => {
                        if let Some(t) = item["text"].as_str().filter(|t| !t.trim().is_empty()) {
                            out.push(Out::Text(t.to_string()));
                        }
                    }
                    Some("command_execution") => {
                        if self.started.insert(id.clone()) {
                            let command = simplify_command(item["command"].as_str().unwrap_or(""));
                            let (name, args) = map_vx(&command, &self.ws)
                                .unwrap_or_else(|| ("run_command".to_string(), json!({ "command": command })));
                            self.names.insert(id.clone(), name.clone());
                            out.push(Out::ToolCall { id: id.clone(), name, args });
                        }
                        if completed {
                            let code = item["exit_code"].as_i64().unwrap_or(-1);
                            let output = item["aggregated_output"].as_str().unwrap_or("");
                            let name = self.names.get(&id).cloned().unwrap_or_else(|| "run_command".to_string());
                            let result = if name == "run_command" {
                                format!("exit={code}
{output}")
                            } else if code != 0 && !output.starts_with("Error") {
                                format!("Error: {output}")
                            } else {
                                output.to_string()
                            };
                            out.push(Out::ToolResult { id, name, result });
                        }
                    }
                    Some("file_change") if completed => {
                        let ok = item["status"].as_str() != Some("failed");
                        for (i, ch) in item["changes"].as_array().into_iter().flatten().enumerate() {
                            let kind = ch["kind"].as_str().unwrap_or("update");
                            let name = if kind == "add" { "write_file" } else { "edit_file" };
                            let path = rel_path(&self.ws, ch["path"].as_str().unwrap_or(""));
                            let call_id = format!("{id}:{i}");
                            out.push(Out::ToolCall {
                                id: call_id.clone(),
                                name: name.to_string(),
                                args: json!({ "path": path, "kind": kind }),
                            });
                            out.push(Out::ToolResult {
                                id: call_id,
                                name: name.to_string(),
                                result: if ok {
                                    format!("{} {}", kind_verb(kind), path)
                                } else {
                                    format!("Error: change to {path} failed")
                                },
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("turn.completed") => self.finished = true,
            Some("turn.failed") => {
                self.finished = true;
                let msg = v["error"]["message"].as_str().unwrap_or("Codex turn failed.");
                out.push(Out::Error(msg.to_string()));
            }
            // A bare `error` event is often a transient "Reconnecting…" notice; `turn.failed`
            // carries the terminal failure.
            _ => {}
        }
        out
    }
}

fn kind_verb(kind: &str) -> &'static str {
    match kind {
        "add" => "Created",
        "delete" => "Deleted",
        _ => "Updated",
    }
}

/// Tool-result `content` is either a string or an array of `{type:"text", text}` blocks.
fn flatten_content(c: &Value) -> String {
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
fn rel_path(ws: &Path, p: &str) -> String {
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

/// Map Claude Code's tool vocabulary onto the names the chat UI already knows
/// (`read_file`, `edit_file`, …) so "View diff" and file refresh keep working.
fn map_claude_tool(name: &str, input: &Value, ws: &Path) -> (String, Value) {
    let path_of = |key: &str| input[key].as_str().map(|p| rel_path(ws, p)).unwrap_or_default();
    match name {
        "Read" => {
            let mut args = json!({ "path": path_of("file_path") });
            if let Some(start) = input["offset"].as_u64() {
                args["start_line"] = json!(start);
                if let Some(limit) = input["limit"].as_u64() {
                    args["end_line"] = json!(start + limit.saturating_sub(1));
                }
            }
            ("read_file".to_string(), args)
        }
        "Edit" | "MultiEdit" => ("edit_file".to_string(), json!({ "path": path_of("file_path") })),
        "NotebookEdit" => ("edit_file".to_string(), json!({ "path": path_of("notebook_path") })),
        "Write" => ("write_file".to_string(), json!({ "path": path_of("file_path") })),
        "Bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            map_vx(cmd, ws).unwrap_or_else(|| ("run_command".to_string(), json!({ "command": cmd })))
        }
        "Grep" => (
            "search_code".to_string(),
            json!({ "query": input["pattern"].as_str().unwrap_or("") }),
        ),
        "Glob" => (
            "list_files".to_string(),
            json!({ "dir": input["pattern"].as_str().unwrap_or("") }),
        ),
        "LS" => ("list_files".to_string(), json!({ "dir": path_of("path") })),
        other => {
            let mut args = if input.is_object() { input.clone() } else { json!({}) };
            let p = path_of("file_path");
            if !p.is_empty() {
                args["path"] = json!(p);
            }
            (other.to_string(), args)
        }
    }
}

/// Recognise a `vx ...` shell call and present it with the chat UI's tool vocabulary.
fn map_vx(command: &str, ws: &Path) -> Option<(String, Value)> {
    let re = regex::Regex::new(r"(?:^|\|\s*|&&\s*|;\s*)vx(?:\.exe)?\s+(\w+)([^\r\n]*)").ok()?;
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
        "check" => ("verify".into(), json!({ "command": "" })),
        _ => return None,
    })
}

/// Codex reports the full shell invocation (`pwsh.exe -Command "<script>"`); show just the script.
fn simplify_command(cmd: &str) -> String {
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

/// Earlier turns are replayed as plain text; the CLI runs statelessly per request.
fn build_prompt(messages: &[ChatMessage]) -> String {
    let Some((last, earlier)) = messages.split_last() else {
        return String::new();
    };
    if earlier.is_empty() {
        return last.content.clone();
    }
    let mut history: Vec<String> = Vec::new();
    let mut used = 0usize;
    for m in earlier.iter().rev() {
        let who = if m.role == "assistant" { "Assistant" } else { "User" };
        let entry = format!("{who}: {}", m.content.trim());
        used += entry.len();
        if used > MAX_HISTORY_CHARS && !history.is_empty() {
            break;
        }
        history.push(entry);
    }
    history.reverse();
    format!(
        "Earlier in this conversation:\n\n{}\n\nCurrent request:\n\n{}",
        history.join("\n\n"),
        last.content
    )
}

fn cli_args(kind: CliKind, ws: &Path, model: &str) -> Vec<String> {
    let model = model.trim();
    let want_model = !model.is_empty() && !model.eq_ignore_ascii_case("default");
    let mut args: Vec<String> = match kind {
        // Edits are auto-accepted (there is no approval UI in headless mode); other tools that
        // need approval are denied and reported back as tool errors.
        CliKind::Claude => [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "acceptEdits",
            "--no-session-persistence",
        ]
        .map(String::from)
        .to_vec(),
        CliKind::Codex => [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write",
            "-C",
        ]
        .map(String::from)
        .into_iter()
        .chain(std::iter::once(ws.to_string_lossy().to_string()))
        .collect(),
    };
    // Give the CLI the `vx` tools (found on PATH) without touching its own config: the usage
    // hint and, for Claude's headless mode, permission to run `vx` come from command-line flags.
    if crate::pty::vx_dir().is_some() {
        match kind {
            CliKind::Claude => {
                args.push("--append-system-prompt".to_string());
                args.push(crate::code_intel::AGENT_HINT.to_string());
                args.push("--allowedTools".to_string());
                args.push("Bash(vx:*)".to_string());
            }
            CliKind::Codex => {
                args.push("-c".to_string());
                let quoted = serde_json::to_string(crate::code_intel::AGENT_HINT).unwrap_or_default();
                args.push(format!("developer_instructions={quoted}"));
            }
        }
    }
    if want_model {
        args.push(if kind == CliKind::Codex { "-m" } else { "--model" }.to_string());
        args.push(model.to_string());
    }
    args
}

#[cfg(windows)]
async fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = background_command("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .await;
    }
}

#[cfg(not(windows))]
async fn kill_tree(_pid: Option<u32>) {}

pub async fn run(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    kind: CliKind,
    auto_cp: bool,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let cmd = kind.cmd();
    let Some(exe) = tokio::task::spawn_blocking(move || crate::pty::resolve_cli_tool(cmd))
        .await
        .ok()
        .flatten()
    else {
        send_sse(
            tx,
            "error",
            json!({ "message": format!(
                "{} was not found. Install it with: {}",
                kind.label(),
                kind.install_hint()
            ) }),
        )
        .await;
        return;
    };

    // The CLI edits files on its own, so snapshot once up front (per-edit hooks aren't available).
    if auto_cp {
        if let Ok(cp) = create_checkpoint_internal(state, "Before agent edit".to_string()).await {
            send_sse(tx, "checkpoint", serde_json::to_value(&cp).unwrap_or(Value::Null)).await;
        }
    }

    let mut command = background_command(&exe.to_string_lossy());
    command
        .args(cli_args(kind, ws, &req.model))
        .current_dir(ws)
        .env("PATH", crate::pty::augmented_path_env())
        .env("VIREO_WORKSPACE", ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if kind == CliKind::Claude {
        // Allow launching from inside another Claude Code session.
        command.env_remove("CLAUDECODE");
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            send_sse(
                tx,
                "error",
                json!({ "message": format!("Failed to start {} ({}): {e}", kind.label(), exe.display()) }),
            )
            .await;
            return;
        }
    };
    let pid = child.id();

    // The prompt goes over stdin: avoids Windows' command-line length limit and quoting issues.
    if let Some(mut stdin) = child.stdin.take() {
        let prompt = build_prompt(&req.messages);
        tokio::spawn(async move {
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let stderr_task = child.stderr.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = (&mut s).take(16 * 1024).read_to_end(&mut buf).await;
            // Keep draining so the child never blocks on a full pipe.
            let _ = tokio::io::copy(&mut s, &mut tokio::io::sink()).await;
            String::from_utf8_lossy(&buf).to_string()
        })
    });

    let Some(stdout) = child.stdout.take() else { return };
    let mut lines = BufReader::new(stdout).lines();
    let mut norm = Normalizer::new(kind, ws);

    loop {
        tokio::select! {
            _ = tx.closed() => {
                // Client pressed Stop / went away: end the CLI and anything it spawned.
                kill_tree(pid).await;
                let _ = child.start_kill();
                return;
            }
            line = lines.next_line() => {
                let line = match line { Ok(Some(l)) => l, _ => break };
                let line = line.trim();
                if line.is_empty() { continue; }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                for ev in norm.feed(&v) {
                    emit(state, tx, ev).await;
                }
            }
        }
    }

    let status = child.wait().await.ok();
    let stderr_text = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    let failed = status.map(|s| !s.success()).unwrap_or(true);
    if !norm.errored && (failed || !norm.finished) {
        let detail = stderr_text.trim();
        let code = status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        let msg = if detail.is_empty() {
            format!("{} exited unexpectedly (code {code}).", kind.label())
        } else {
            format!("{} exited with code {code}: {}", kind.label(), tail_chars(detail, 800))
        };
        send_sse(tx, "error", json!({ "message": msg })).await;
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

async fn emit(state: &AppState, tx: &mpsc::Sender<Result<Event, Infallible>>, ev: Out) {
    match ev {
        Out::Text(text) => send_sse(tx, "text", json!({ "text": text })).await,
        Out::ToolCall { id, name, args } => {
            send_sse(tx, "tool_call", json!({ "id": id, "name": name, "args": args })).await
        }
        Out::ToolResult { id, name, result } => {
            if matches!(name.as_str(), "write_file" | "edit_file" | "run_command") {
                state.notify_fs_changed(vec!["cli".to_string()]);
            }
            let result: String = result.chars().take(1500).collect();
            send_sse(tx, "tool_result", json!({ "id": id, "name": name, "result": result })).await
        }
        Out::Error(message) => send_sse(tx, "error", json!({ "message": message })).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(kind: CliKind, ws: &str, lines: &[&str]) -> Vec<Out> {
        let mut n = Normalizer::new(kind, Path::new(ws));
        lines
            .iter()
            .flat_map(|l| n.feed(&serde_json::from_str::<Value>(l).unwrap()))
            .collect()
    }

    #[test]
    fn codex_real_stream() {
        // Captured from codex-cli 0.167 `exec --json`.
        let ws = r"C:\proj";
        let out = feed_all(
            CliKind::Codex,
            ws,
            &[
                r#"{"type":"thread.started","thread_id":"t"}"#,
                r#"{"type":"turn.started"}"#,
                r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Reading."}}"#,
                r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"\"C:\\\\x\\\\pwsh.exe\" -Command \"Get-Content a.txt\"","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
                r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"\"C:\\\\x\\\\pwsh.exe\" -Command \"Get-Content a.txt\"","aggregated_output":"hello","exit_code":0,"status":"completed"}}"#,
                r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"C:\\proj\\src\\b.txt","kind":"add"}],"status":"completed"}}"#,
                r#"{"type":"turn.completed","usage":{}}"#,
            ],
        );
        assert_eq!(out[0], Out::Text("Reading.".into()));
        assert_eq!(
            out[1],
            Out::ToolCall { id: "item_1".into(), name: "run_command".into(), args: json!({"command": "Get-Content a.txt"}) }
        );
        assert_eq!(
            out[2],
            Out::ToolResult { id: "item_1".into(), name: "run_command".into(), result: "exit=0\nhello".into() }
        );
        assert_eq!(
            out[3],
            Out::ToolCall { id: "item_2:0".into(), name: "write_file".into(), args: json!({"path": "src/b.txt", "kind": "add"}) }
        );
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn codex_turn_failed_is_one_error() {
        let ws = r"C:\proj";
        let mut n = Normalizer::new(CliKind::Codex, Path::new(ws));
        let a = n.feed(&json!({"type":"error","message":"model x unsupported"}));
        let b = n.feed(&json!({"type":"turn.failed","error":{"message":"model x unsupported"}}));
        assert!(a.is_empty());
        assert_eq!(b, vec![Out::Error("model x unsupported".into())]);
        assert!(n.errored && n.finished);
    }

    #[test]
    fn claude_tools_are_normalized() {
        let out = feed_all(
            CliKind::Claude,
            r"C:\proj",
            &[
                r#"{"type":"system","subtype":"init"}"#,
                r#"{"type":"assistant","message":{"model":"claude-x","content":[{"type":"text","text":"Editing."},{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"C:\\proj\\src\\a.ts","old_string":"a","new_string":"b"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"text","text":"ok"}]}]}}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#,
            ],
        );
        assert_eq!(out[0], Out::Text("Editing.".into()));
        assert_eq!(
            out[1],
            Out::ToolCall { id: "tu1".into(), name: "edit_file".into(), args: json!({"path": "src/a.ts"}) }
        );
        assert_eq!(
            out[2],
            Out::ToolResult { id: "tu1".into(), name: "edit_file".into(), result: "ok".into() }
        );
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn claude_not_logged_in_is_single_error() {
        // Captured from claude 2.1.328 when no login exists.
        let out = feed_all(
            CliKind::Claude,
            r"C:\proj",
            &[
                r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"Not logged in · Please run /login"}]}}"#,
                r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
            ],
        );
        assert_eq!(out, vec![Out::Error("Not logged in · Please run /login".into())]);
    }

    #[test]
    fn prompt_includes_history() {
        let m = |r: &str, c: &str| ChatMessage { role: r.into(), content: c.into() };
        assert_eq!(build_prompt(&[m("user", "hi")]), "hi");
        let p = build_prompt(&[m("user", "a"), m("assistant", "b"), m("user", "c")]);
        assert!(p.contains("User: a") && p.contains("Assistant: b") && p.ends_with("c"));
    }

    #[test]
    fn args_skip_default_model() {
        let ws = Path::new("C:/proj");
        assert!(!cli_args(CliKind::Codex, ws, "default").contains(&"-m".to_string()));
        assert!(cli_args(CliKind::Codex, ws, "gpt-5.6-luna").contains(&"-m".to_string()));
        assert!(cli_args(CliKind::Claude, ws, "sonnet").contains(&"--model".to_string()));
    }

    #[test]
    fn vx_shell_calls_map_to_chat_tools() {
        let ws = Path::new("C:/w");
        let (n, a) = map_vx("vx edit C:/w/src/a.rs 5#ab..9#cd <<'EOF'
new
EOF", ws).unwrap();
        assert_eq!((n.as_str(), a["path"].as_str()), ("edit_file", Some("src/a.rs")));
        let (n, a) = map_vx("@'
x
'@ | vx symbol src/a.rs Foo::bar --replace", ws).unwrap();
        assert_eq!((n.as_str(), a["mode"].as_str()), ("symbol_edit", Some("replace")));
        let (n, a) = map_vx("vx view src/a.rs 10..20", ws).unwrap();
        assert_eq!((n.as_str(), a["start_line"].as_u64(), a["end_line"].as_u64()), ("read_file", Some(10), Some(20)));
        assert!(map_vx("cargo build", ws).is_none());
    }
}
