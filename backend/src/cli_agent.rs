//! Runs an installed, already-logged-in agent CLI (Antigravity, Claude Code, Codex, Gemini CLI,
//! OpenCode, Cursor CLI, GitHub Copilot, Amp, Qwen Code) headlessly and translates its NDJSON
//! event stream into Vireo's agent SSE events (`text`, `tool_call`, `tool_result`, `checkpoint`,
//! `error`). The CLI keeps using its own credentials and config; Vireo only passes flags on the
//! command line and never reads or writes the CLI's auth files.

use crate::agent::{send_sse, AgentChatReq, ChatMessage};
use crate::checkpoint::TurnTracker;
use crate::state::{background_command, AppState, PermissionDecision};
use axum::response::sse::Event;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Drops this run's unanswered permission prompts however `run` exits.
struct PendingGuard {
    state: AppState,
    turn_id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.state.pending_permissions.lock() {
            m.retain(|(t, _), _| t != &self.turn_id);
        }
    }
}

/// `POST /api/agent/permission`: the user's answer to an in-chat permission card.
#[derive(serde::Deserialize)]
pub struct PermissionReq {
    pub turn_id: String,
    pub request_id: String,
    pub allow: bool,
    #[serde(default)]
    pub remember: bool,
}

pub async fn answer_permission(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(req): axum::Json<PermissionReq>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, String)> {
    let tx = state
        .pending_permissions
        .lock()
        .ok()
        .and_then(|mut m| m.remove(&(req.turn_id, req.request_id)));
    match tx {
        Some(tx) => {
            let _ = tx.send(PermissionDecision { allow: req.allow, remember: req.remember });
            Ok(axum::Json(json!({ "ok": true })))
        }
        None => Err((axum::http::StatusCode::NOT_FOUND, "No such pending permission request".to_string())),
    }
}

/// Largest slice of earlier conversation sent along with the new request via stdin.
const MAX_HISTORY_CHARS: usize = 40_000;
/// Cap for CLIs that receive the prompt as a command-line argument (avoids Windows' 32K arg limit).
const MAX_ARG_PROMPT_CHARS: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CliKind {
    Antigravity,
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Cursor,
    Copilot,
    Amp,
    Qwen,
}

impl CliKind {
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "antigravity-cli" => Some(Self::Antigravity),
            "claude-cli" => Some(Self::Claude),
            "codex-cli" => Some(Self::Codex),
            "gemini-cli" => Some(Self::Gemini),
            "opencode-cli" => Some(Self::OpenCode),
            "cursor-cli" => Some(Self::Cursor),
            "copilot-cli" => Some(Self::Copilot),
            "amp-cli" => Some(Self::Amp),
            "qwen-cli" => Some(Self::Qwen),
            _ => None,
        }
    }

    fn cmds(self) -> &'static [&'static str] {
        match self {
            Self::Antigravity => &["agy"],
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Gemini => &["gemini"],
            Self::OpenCode => &["opencode"],
            Self::Cursor => &["cursor-agent", "agent"],
            Self::Copilot => &["copilot"],
            Self::Amp => &["amp"],
            Self::Qwen => &["qwen"],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor CLI",
            Self::Copilot => "GitHub Copilot",
            Self::Amp => "Amp",
            Self::Qwen => "Qwen Code",
        }
    }

    fn install_hint(self) -> &'static str {
        match self {
            Self::Antigravity => "irm https://antigravity.google/cli/install.ps1 | iex   (macOS/Linux: curl -fsSL https://antigravity.google/cli/install.sh | bash)",
            Self::Claude => "irm https://claude.ai/install.ps1 | iex   (macOS/Linux: curl -fsSL https://claude.ai/install.sh | bash)",
            Self::Codex => "npm install -g @openai/codex",
            Self::Gemini => "npm install -g @google/gemini-cli",
            Self::OpenCode => "npm install -g opencode-ai   (macOS/Linux: curl -fsSL https://opencode.ai/install | bash)",
            Self::Cursor => "irm 'https://cursor.com/install?win32=true' | iex   (macOS/Linux: curl https://cursor.com/install -fsS | bash)",
            Self::Copilot => "winget install GitHub.Copilot   (or: npm install -g @github/copilot)",
            Self::Amp => "npm install -g @ampcode/cli   (macOS/Linux: curl -fsSL https://ampcode.com/install.sh | bash)",
            Self::Qwen => "npm install -g @qwen-code/qwen-code@latest",
        }
    }
}

/// How much the CLI may do without asking, chosen per request in the composer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PermMode {
    /// Ask before edits and shell commands.
    Ask,
    /// Auto-accept workspace edits; ask for anything else (default).
    AutoEdit,
    /// No prompts, no sandbox.
    FullAccess,
}

impl PermMode {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            Some("ask") => Self::Ask,
            Some("full_access") => Self::FullAccess,
            _ => Self::AutoEdit,
        }
    }
}

/// One normalized event destined for the chat timeline.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Claude wants to use a tool that needs the user's approval.
    PermissionRequest { request_id: String, tool: String, input: Value },
    /// Claude withdrew a pending permission request.
    PermissionCancel { request_id: String },
    /// A control request we do not implement; must be answered with an error or the CLI waits.
    UnsupportedControl { request_id: String },
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
    /// Buffered text deltas for CLIs that stream token-by-token (`agy`, `gemini`, `qwen`, `opencode`).
    text_buf: String,
    /// Whether at least one `Out::Text` has been emitted during this turn.
    pub emitted_text: bool,
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
            text_buf: String::new(),
            emitted_text: false,
            finished: false,
            errored: false,
        }
    }

    fn flush_text(&mut self, out: &mut Vec<Out>) {
        if self.text_buf.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut self.text_buf);
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            self.emitted_text = true;
            out.push(Out::Text(trimmed.to_string()));
        }
    }

    /// Flush any buffered text when the child closes stdout.
    pub fn finish_eof(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        self.flush_text(&mut out);
        out
    }

    pub fn feed(&mut self, v: &Value) -> Vec<Out> {
        let out = match self.kind {
            CliKind::Antigravity => self.feed_antigravity(v),
            CliKind::Claude | CliKind::Amp => self.feed_claude(v),
            CliKind::Codex => self.feed_codex(v),
            CliKind::Gemini | CliKind::Qwen => self.feed_gemini(v),
            CliKind::OpenCode => self.feed_opencode(v),
            CliKind::Cursor => self.feed_cursor(v),
            CliKind::Copilot => self.feed_copilot(v),
        };
        if out.iter().any(|o| matches!(o, Out::Text(_))) {
            self.emitted_text = true;
        }
        if out.iter().any(|o| matches!(o, Out::Error(_))) {
            self.errored = true;
        }
        out
    }

    fn feed_antigravity(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["event"].as_str() {
            Some("step_update") => {
                let su = if v["step_update"].is_object() { &v["step_update"] } else { v };
                let step_type = su["step_type"].as_str();
                let state = su["state"].as_str();
                match step_type {
                    Some("agent_response") => {
                        let delta = su["text_delta"]
                            .as_str()
                            .or_else(|| su["details"]["text_delta"].as_str());
                        if let Some(d) = delta {
                            self.text_buf.push_str(d);
                        }
                        if state == Some("DONE") {
                            self.flush_text(&mut out);
                        }
                    }
                    Some("tool") => {
                        self.flush_text(&mut out);
                        let id = su["step_index"]
                            .as_u64()
                            .map(|i| format!("step_{i}"))
                            .or_else(|| su["tool_info"]["id"].as_str().map(str::to_string))
                            .unwrap_or_else(|| "tool".to_string());
                        let info = &su["tool_info"];
                        let raw_name = su["tool_name"]
                            .as_str()
                            .or_else(|| info["name"].as_str())
                            .or_else(|| su["details"]["tool_name"].as_str())
                            .unwrap_or("tool");
                        let params = if !info["parameters"].is_null() {
                            &info["parameters"]
                        } else if !su["details"].is_null() {
                            &su["details"]
                        } else {
                            info
                        };
                        if self.started.insert(id.clone()) {
                            let (name, args) = map_antigravity_tool(raw_name, params, &self.ws);
                            self.names.insert(id.clone(), name.clone());
                            out.push(Out::ToolCall { id: id.clone(), name, args });
                        }
                        if matches!(state, Some("DONE") | Some("ERROR") | Some("FAILED") | Some("CANCELED")) {
                            let name = self.names.get(&id).cloned().unwrap_or_else(|| raw_name.to_string());
                            let err = info["error"]["message"]
                                .as_str()
                                .or_else(|| info["error"].as_str())
                                .filter(|s| !s.trim().is_empty());
                            let mut text = if let Some(e) = err {
                                if e.starts_with("Error") { e.to_string() } else { format!("Error: {e}") }
                            } else {
                                flatten_value(&info["output"])
                            };
                            if matches!(state, Some("ERROR") | Some("FAILED")) && !text.starts_with("Error") {
                                text = if text.is_empty() {
                                    "Error: tool failed".to_string()
                                } else {
                                    format!("Error: {text}")
                                };
                            }
                            out.push(Out::ToolResult { id, name, result: text });
                        }
                    }
                    _ => {}
                }
            }
            Some("result") => {
                self.flush_text(&mut out);
                self.finished = true;
                let res = if v["result"].is_object() { &v["result"] } else { v };
                let status = res["status"].as_str().unwrap_or("");
                let err_msg = res["error"]["message"]
                    .as_str()
                    .or_else(|| res["error"].as_str())
                    .filter(|s| !s.trim().is_empty());
                if status.eq_ignore_ascii_case("ERROR") || err_msg.is_some() {
                    let msg = err_msg
                        .map(str::to_string)
                        .unwrap_or_else(|| "Antigravity reported an error.".to_string());
                    out.push(Out::Error(msg));
                } else if !self.emitted_text {
                    if let Some(resp) = res["response"].as_str().filter(|s| !s.trim().is_empty()) {
                        out.push(Out::Text(resp.trim().to_string()));
                    } else if let Some(denied) = res["denied_actions"].as_array().filter(|a| !a.is_empty()) {
                        let actions: Vec<&str> = denied
                            .iter()
                            .filter_map(|d| d["display_name"].as_str().or_else(|| d["action"].as_str()))
                            .collect();
                        let what = if actions.is_empty() { "a tool".to_string() } else { actions.join(", ") };
                        out.push(Out::Error(format!(
                            "Antigravity denied permission for {what} in headless mode. Switch the permission mode to Auto-edit or Full access to allow it."
                        )));
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn feed_claude(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("control_request") => {
                let request_id = v["request_id"].as_str().unwrap_or("").to_string();
                let req = &v["request"];
                if req["subtype"].as_str() == Some("can_use_tool") && !request_id.is_empty() {
                    out.push(Out::PermissionRequest {
                        request_id,
                        tool: req["tool_name"].as_str().unwrap_or("tool").to_string(),
                        input: req["input"].clone(),
                    });
                } else if !request_id.is_empty() {
                    out.push(Out::UnsupportedControl { request_id });
                }
            }
            Some("control_cancel_request") => {
                if let Some(id) = v["request_id"].as_str() {
                    out.push(Out::PermissionCancel { request_id: id.to_string() });
                }
            }
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
                    let label = self.kind.label();
                    let msg = v["result"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .or_else(|| v["subtype"].as_str().map(|s| format!("{label} stopped: {s}")))
                        .unwrap_or_else(|| format!("{label} reported an error."));
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

    fn feed_gemini(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        let kind = v["type"].as_str().or_else(|| v["event"].as_str());
        match kind {
            Some("message") => {
                if v["role"].as_str() != Some("assistant") {
                    return out;
                }
                if let Some(d) = v["delta"].as_str() {
                    self.text_buf.push_str(d);
                } else if let Some(content) = v["content"].as_str() {
                    if v["delta"].as_bool() == Some(true) {
                        self.text_buf.push_str(content);
                    } else {
                        self.flush_text(&mut out);
                        if !content.trim().is_empty() {
                            out.push(Out::Text(content.trim().to_string()));
                        }
                    }
                }
            }
            Some("tool_use") => {
                self.flush_text(&mut out);
                let id = v["tool_id"]
                    .as_str()
                    .or_else(|| v["call_id"].as_str())
                    .or_else(|| v["id"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                let raw_name = v["tool_name"]
                    .as_str()
                    .or_else(|| v["tool"].as_str())
                    .or_else(|| v["name"].as_str())
                    .unwrap_or("tool");
                let params = if !v["parameters"].is_null() {
                    &v["parameters"]
                } else if !v["arguments"].is_null() {
                    &v["arguments"]
                } else {
                    &v["input"]
                };
                let (name, args) = map_generic_tool(raw_name, params, &self.ws);
                self.names.insert(id.clone(), name.clone());
                self.started.insert(id.clone());
                out.push(Out::ToolCall { id, name, args });
            }
            Some("tool_result") => {
                let id = v["tool_id"]
                    .as_str()
                    .or_else(|| v["call_id"].as_str())
                    .or_else(|| v["id"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                let name = self.names.get(&id).cloned().unwrap_or_else(|| "tool".to_string());
                let mut text = flatten_value(&v["output"]);
                if text.is_empty() {
                    text = flatten_value(&v["error"]["message"]);
                }
                if v["status"].as_str() == Some("error") && !text.starts_with("Error") {
                    text = format!("Error: {text}");
                }
                out.push(Out::ToolResult { id, name, result: text });
            }
            Some("error") => {
                if v["severity"].as_str() != Some("warning") {
                    if let Some(msg) = v["message"].as_str().filter(|s| !s.trim().is_empty()) {
                        out.push(Out::Error(msg.to_string()));
                    }
                }
            }
            Some("result") => {
                self.flush_text(&mut out);
                self.finished = true;
                if v["status"].as_str() == Some("error") {
                    let msg = v["error"]["message"]
                        .as_str()
                        .or_else(|| v["error"].as_str())
                        .or_else(|| v["message"].as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{} reported an error.", self.kind.label()));
                    out.push(Out::Error(msg));
                } else if !self.emitted_text {
                    if let Some(resp) = v["response"].as_str().filter(|s| !s.trim().is_empty()) {
                        out.push(Out::Text(resp.trim().to_string()));
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn feed_opencode(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("text") => {
                if let Some(t) = v["part"]["text"].as_str() {
                    self.text_buf.push_str(t);
                }
            }
            Some("tool_use") => {
                self.flush_text(&mut out);
                let part = &v["part"];
                let id = part["callID"]
                    .as_str()
                    .or_else(|| part["id"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                let raw_name = part["tool"].as_str().unwrap_or("tool");
                let state = &part["state"];
                let status = state["status"].as_str().unwrap_or("");
                if self.started.insert(id.clone()) {
                    let (name, args) = map_generic_tool(raw_name, &state["input"], &self.ws);
                    self.names.insert(id.clone(), name.clone());
                    out.push(Out::ToolCall { id: id.clone(), name, args });
                }
                if matches!(status, "completed" | "error") {
                    let name = self.names.get(&id).cloned().unwrap_or_else(|| raw_name.to_string());
                    let output = flatten_value(&state["output"]);
                    let result = if status == "error" {
                        let err = state["error"].as_str().unwrap_or(&output);
                        if err.starts_with("Error") { err.to_string() } else { format!("Error: {err}") }
                    } else if name == "run_command" {
                        if let Some(code) = state["metadata"]["exitCode"].as_i64() {
                            format!("exit={code}\n{output}")
                        } else {
                            output
                        }
                    } else {
                        output
                    };
                    out.push(Out::ToolResult { id, name, result });
                }
            }
            Some("step_finish") => {
                self.flush_text(&mut out);
                if v["part"]["reason"].as_str() != Some("tool-calls") {
                    self.finished = true;
                }
            }
            Some("error") => {
                self.flush_text(&mut out);
                self.finished = true;
                let msg = v["error"]["data"]["message"]
                    .as_str()
                    .or_else(|| v["error"]["message"].as_str())
                    .or_else(|| v["error"]["name"].as_str())
                    .or_else(|| v["message"].as_str())
                    .unwrap_or("OpenCode reported an error.");
                out.push(Out::Error(msg.to_string()));
            }
            _ => {}
        }
        out
    }

    fn feed_cursor(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("assistant") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"].as_str() == Some("text") {
                        if let Some(t) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                            out.push(Out::Text(t.to_string()));
                        }
                    }
                }
            }
            Some("tool_call") => {
                let subtype = v["subtype"].as_str().unwrap_or("");
                let id = v["call_id"].as_str().unwrap_or("tool").to_string();
                let tc_obj = v["tool_call"].as_object();
                let (raw_key, inner) = tc_obj
                    .and_then(|m| m.iter().next())
                    .map(|(k, val)| (k.as_str(), val))
                    .unwrap_or(("tool", &Value::Null));
                if self.started.insert(id.clone()) {
                    let (name, args) = map_generic_tool(raw_key, &inner["args"], &self.ws);
                    self.names.insert(id.clone(), name.clone());
                    out.push(Out::ToolCall { id: id.clone(), name, args });
                }
                if subtype == "completed" {
                    let name = self.names.get(&id).cloned().unwrap_or_else(|| raw_key.to_string());
                    let res = if !inner["result"].is_null() { &inner["result"] } else { &v["result"] };
                    let text = if let Some(err) = res["rejected"]["reason"]
                        .as_str()
                        .or_else(|| res["error"].as_str())
                    {
                        format!("Error: {err}")
                    } else if let Some(c) = res["success"]["content"].as_str().or_else(|| res["content"].as_str()) {
                        c.to_string()
                    } else if res["success"].is_object() {
                        flatten_value(&res["success"])
                    } else {
                        flatten_value(res)
                    };
                    out.push(Out::ToolResult { id, name, result: text });
                }
            }
            Some("result") => {
                self.finished = true;
                if v["is_error"].as_bool() == Some(true) {
                    let msg = v["result"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or("Cursor CLI reported an error.");
                    out.push(Out::Error(msg.to_string()));
                } else if !self.emitted_text {
                    if let Some(r) = v["result"].as_str().filter(|s| !s.trim().is_empty()) {
                        out.push(Out::Text(r.trim().to_string()));
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn feed_copilot(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        let data = &v["data"];
        match v["type"].as_str() {
            Some("assistant.message") => {
                if let Some(t) = data["content"].as_str().filter(|t| !t.trim().is_empty()) {
                    out.push(Out::Text(t.to_string()));
                }
            }
            Some("tool.execution_start") => {
                let id = data["toolCallId"]
                    .as_str()
                    .or_else(|| data["callId"].as_str())
                    .or_else(|| data["id"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                let raw_name = data["toolName"]
                    .as_str()
                    .or_else(|| data["tool"].as_str())
                    .or_else(|| data["name"].as_str())
                    .unwrap_or("tool");
                let params = if !data["arguments"].is_null() { &data["arguments"] } else { &data["args"] };
                if self.started.insert(id.clone()) {
                    let (name, args) = map_generic_tool(raw_name, params, &self.ws);
                    self.names.insert(id.clone(), name.clone());
                    out.push(Out::ToolCall { id, name, args });
                }
            }
            Some("tool.execution_complete") => {
                let id = data["toolCallId"]
                    .as_str()
                    .or_else(|| data["callId"].as_str())
                    .or_else(|| data["id"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                let name = self.names.get(&id).cloned().unwrap_or_else(|| "tool".to_string());
                let mut text = if let Some(c) = data["result"]["content"].as_str() {
                    c.to_string()
                } else if !data["result"].is_null() {
                    flatten_value(&data["result"])
                } else if !data["output"].is_null() {
                    flatten_value(&data["output"])
                } else {
                    flatten_value(&data["error"])
                };
                if data["success"].as_bool() == Some(false) && !text.starts_with("Error") {
                    text = format!("Error: {text}");
                }
                out.push(Out::ToolResult { id, name, result: text });
            }
            Some("session.task_complete") | Some("session.shutdown") | Some("workflow.result") | Some("result") => {
                self.finished = true;
                let err = data["error"]
                    .as_str()
                    .or_else(|| v["error"]["message"].as_str())
                    .or_else(|| v["error"].as_str())
                    .filter(|s| !s.trim().is_empty());
                if let Some(msg) = err {
                    out.push(Out::Error(msg.to_string()));
                } else if !self.emitted_text {
                    if let Some(s) = data["summary"]
                        .as_str()
                        .or_else(|| v["result"].as_str())
                        .filter(|s| !s.trim().is_empty())
                    {
                        out.push(Out::Text(s.trim().to_string()));
                    }
                }
            }
            Some("session.error") | Some("error") => {
                self.finished = true;
                let msg = data["message"]
                    .as_str()
                    .or_else(|| v["message"].as_str())
                    .unwrap_or("GitHub Copilot reported an error.");
                out.push(Out::Error(msg.to_string()));
            }
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

/// Stringify arbitrary tool output (string, `{type:"text"}` array, or JSON object).
fn flatten_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            let texts: Vec<&str> = a
                .iter()
                .filter_map(|b| b.as_str().or_else(|| b["text"].as_str()))
                .collect();
            if !texts.is_empty() {
                texts.join("\n")
            } else {
                v.to_string()
            }
        }
        other => other.to_string(),
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

fn first_path(input: &Value, keys: &[&str], ws: &Path) -> String {
    keys.iter()
        .find_map(|k| input[*k].as_str())
        .filter(|s| !s.is_empty())
        .map(|p| rel_path(ws, p))
        .unwrap_or_default()
}

/// Map Claude Code (and Amp) tool vocabulary onto the names the chat UI already knows
/// (`read_file`, `edit_file`, …) so "View diff" and file refresh keep working.
fn map_claude_tool(name: &str, input: &Value, ws: &Path) -> (String, Value) {
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
            map_vx(cmd, ws).unwrap_or_else(|| ("run_command".to_string(), json!({ "command": cmd })))
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

/// Map Antigravity CLI (`agy`) tool names onto Vireo's UI tool vocabulary.
fn map_antigravity_tool(name: &str, params: &Value, ws: &Path) -> (String, Value) {
    match name {
        "read_file" | "view_file" => {
            let mut args = json!({ "path": first_path(params, &["Path", "AbsolutePath", "path", "file_path"], ws) });
            if let Some(s) = params["StartLine"].as_u64().or_else(|| params["start_line"].as_u64()) {
                args["start_line"] = json!(s);
            }
            if let Some(e) = params["EndLine"].as_u64().or_else(|| params["end_line"].as_u64()) {
                args["end_line"] = json!(e);
            }
            ("read_file".to_string(), args)
        }
        "edit_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "sed_file"
        | "notebook_edit" => (
            "edit_file".to_string(),
            json!({ "path": first_path(params, &["Path", "TargetFile", "path", "file_path"], ws) }),
        ),
        "write_file" | "write_to_file" => (
            "write_file".to_string(),
            json!({ "path": first_path(params, &["Path", "TargetFile", "path", "file_path"], ws) }),
        ),
        "run_command" | "shell" => {
            let cmd = params["CommandLine"]
                .as_str()
                .or_else(|| params["command"].as_str())
                .unwrap_or("");
            map_vx(cmd, ws).unwrap_or_else(|| ("run_command".to_string(), json!({ "command": cmd })))
        }
        "grep_search" => (
            "search_code".to_string(),
            json!({ "query": params["Query"].as_str().or_else(|| params["query"].as_str()).unwrap_or("") }),
        ),
        "find_by_name" | "list_dir" => {
            let dir = params["Pattern"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| first_path(params, &["SearchDirectory", "DirectoryPath", "path", "dir"], ws));
            ("list_files".to_string(), json!({ "dir": dir }))
        }
        other => {
            let mut args = if params.is_object() { params.clone() } else { json!({}) };
            let p = first_path(params, &["Path", "TargetFile", "AbsolutePath", "path", "file_path"], ws);
            if !p.is_empty() {
                args["path"] = json!(p);
            }
            (other.to_string(), args)
        }
    }
}

/// Map Gemini CLI, Qwen Code, OpenCode, Cursor CLI, and Copilot CLI tool names onto Vireo's UI vocabulary.
fn map_generic_tool(name: &str, params: &Value, ws: &Path) -> (String, Value) {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix("toolcall").unwrap_or(&lower);
    let path = || {
        let p = first_path(params, &["file_path", "path", "target_file", "filePath", "filename"], ws);
        if !p.is_empty() {
            return p;
        }
        params["paths"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(|p| rel_path(ws, p))
            .unwrap_or_default()
    };
    match stem {
        "read" | "read_file" | "read_many_files" | "view" | "view_file" => {
            let mut args = json!({ "path": path() });
            if let Some(start) = params["offset"]
                .as_u64()
                .or_else(|| params["start_line"].as_u64())
                .or_else(|| params["line_start"].as_u64())
            {
                args["start_line"] = json!(start);
                if let Some(end) = params["end_line"].as_u64().or_else(|| params["line_end"].as_u64()) {
                    args["end_line"] = json!(end);
                } else if let Some(limit) = params["limit"].as_u64() {
                    args["end_line"] = json!(start + limit.saturating_sub(1));
                }
            }
            ("read_file".to_string(), args)
        }
        "edit"
        | "edit_file"
        | "replace"
        | "patch"
        | "apply_patch"
        | "str_replace_editor"
        | "multi_edit" => ("edit_file".to_string(), json!({ "path": path() })),
        "write" | "write_file" | "create" | "create_file" => {
            ("write_file".to_string(), json!({ "path": path() }))
        }
        "delete" | "delete_file" => (
            "edit_file".to_string(),
            json!({ "path": path(), "kind": "delete" }),
        ),
        "bash" | "shell" | "run_shell_command" | "run_command" | "powershell" | "terminal" => {
            let cmd = params["command"]
                .as_str()
                .or_else(|| params["cmd"].as_str())
                .unwrap_or("");
            map_vx(cmd, ws).unwrap_or_else(|| ("run_command".to_string(), json!({ "command": cmd })))
        }
        "grep" | "grep_search" | "search" | "search_file_content" | "rg" => (
            "search_code".to_string(),
            json!({
                "query": params["pattern"]
                    .as_str()
                    .or_else(|| params["query"].as_str())
                    .or_else(|| params["regex"].as_str())
                    .unwrap_or("")
            }),
        ),
        "glob" | "list" | "ls" | "list_directory" | "list_dir" | "find_files" => {
            let dir = params["pattern"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| first_path(params, &["path", "dir", "dir_path", "directory"], ws));
            ("list_files".to_string(), json!({ "dir": dir }))
        }
        _ => {
            let mut args = if params.is_object() { params.clone() } else { json!({}) };
            let p = path();
            if !p.is_empty() {
                args["path"] = json!(p);
            }
            (name.to_string(), args)
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

/// Path-like argument of a tool request, if any.
fn input_path(input: &Value) -> Option<&str> {
    ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .filter(|s| !s.is_empty())
}

fn norm_path(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

fn is_shell_tool(tool: &str) -> bool {
    matches!(tool, "Bash" | "PowerShell")
}

fn has_shell_meta(cmd: &str) -> bool {
    cmd.contains(['>', '<', '|', ';', '&', '`', '\n', '\r']) || cmd.contains("$(")
}

/// Rule saved by "always allow in this thread". File tools: the containing directory; shell
/// tools: the program name; anything else: just the tool. Rules are enforced by Vireo itself.
pub fn permission_rule(tool: &str, input: &Value) -> String {
    if is_shell_tool(tool) {
        let first = input["command"].as_str().unwrap_or("").split_whitespace().next().unwrap_or("");
        return format!("{tool}:cmd:{}", first.to_lowercase());
    }
    if let Some(p) = input_path(input) {
        let n = norm_path(p);
        let dir = n.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        return format!("{tool}:dir:{dir}");
    }
    tool.to_string()
}

/// Does a saved rule cover this request?
pub fn rule_allows(rule: &str, tool: &str, input: &Value) -> bool {
    let Some(rest) = rule.strip_prefix(tool).and_then(|r| r.strip_prefix(':')) else {
        return rule == tool && input_path(input).is_none() && !is_shell_tool(tool);
    };
    if let Some(prog) = rest.strip_prefix("cmd:") {
        let cmd = input["command"].as_str().unwrap_or("");
        return is_shell_tool(tool)
            && !prog.is_empty()
            && !has_shell_meta(cmd)
            && cmd.split_whitespace().next().map(str::to_lowercase).as_deref() == Some(prog);
    }
    if let Some(dir) = rest.strip_prefix("dir:") {
        let Some(p) = input_path(input) else { return false };
        let p = norm_path(p);
        return !dir.is_empty()
            && !p.split('/').any(|c| c == "..")
            && p.strip_prefix(dir).map_or(false, |r| r.starts_with('/'));
    }
    false
}

fn permission_summary(tool: &str, input: &Value) -> String {
    let detail = if is_shell_tool(tool) {
        input["command"].as_str().map(str::to_string)
    } else {
        input_path(input).map(str::to_string)
    };
    match detail {
        Some(d) => format!("{tool} {}", tail_chars(&d, 300)),
        None => tool.to_string(),
    }
}

fn user_message_line(prompt: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": prompt }] }
    })
    .to_string()
}

fn control_response(request_id: &str, allow: bool, input: &Value) -> String {
    let body = if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "The user denied this action in Vireo." })
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": body }
    })
    .to_string()
}

fn control_error(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": "Unsupported control request" }
    })
    .to_string()
}

fn agy_user_message_line(prompt: &str) -> String {
    let mut line = json!({
        "event": "user",
        "message": { "content": prompt }
    })
    .to_string();
    line.push('\n');
    line
}

/// Earlier turns are replayed as plain text; the CLI runs statelessly per request.
fn build_prompt(messages: &[ChatMessage]) -> String {
    build_prompt_capped(messages, MAX_HISTORY_CHARS)
}

fn build_prompt_capped(messages: &[ChatMessage], max_history: usize) -> String {
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
        if used > max_history && !history.is_empty() {
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

fn with_vx_hint(kind: CliKind, prompt: String, has_vx: bool) -> String {
    // Claude and Codex get `AGENT_HINT` via CLI flags in `cli_args`. Antigravity (`agy`) has no
    // system-prompt CLI flag, so prepend the hint to the turn prompt when `vx` is available.
    if has_vx && kind == CliKind::Antigravity && !prompt.is_empty() {
        format!(
            "[Vireo environment]\n{}\n\n{}",
            crate::code_intel::AGENT_HINT,
            prompt
        )
    } else {
        prompt
    }
}

fn cli_args(kind: CliKind, ws: &Path, model: &str, mode: PermMode) -> Vec<String> {
    let model = model.trim();
    let want_model = !model.is_empty() && !model.eq_ignore_ascii_case("default");
    let mut args: Vec<String> = match kind {
        CliKind::Antigravity => {
            let mut a: Vec<String> = [
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ]
            .map(String::from)
            .to_vec();
            match mode {
                PermMode::Ask => a.extend(["--mode", "plan"].map(String::from)),
                // Headless `agy` auto-denies shell commands under `--mode accept-edits` because it
                // cannot prompt for approval, so AutoEdit and FullAccess both skip interactive prompts.
                PermMode::AutoEdit | PermMode::FullAccess => {
                    a.push("--dangerously-skip-permissions".to_string());
                }
            }
            a
        }
        // Claude talks to us over stream-json both ways: tool approvals arrive as
        // `control_request`s on stdout and are answered on stdin (`--permission-prompt-tool stdio`).
        CliKind::Claude => {
            let pm = match mode {
                PermMode::Ask => "default",
                PermMode::AutoEdit => "acceptEdits",
                PermMode::FullAccess => "bypassPermissions",
            };
            [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-prompt-tool",
                "stdio",
                "--permission-mode",
                pm,
                "--no-session-persistence",
            ]
            .map(String::from)
            .to_vec()
        }
        // `codex exec` cannot ask for approval, so the mode only picks the sandbox.
        CliKind::Codex => {
            let mut a: Vec<String> = ["exec", "--json", "--skip-git-repo-check"].map(String::from).to_vec();
            match mode {
                PermMode::Ask => a.extend(["--sandbox", "read-only"].map(String::from)),
                PermMode::AutoEdit => a.extend(["--sandbox", "workspace-write"].map(String::from)),
                PermMode::FullAccess => a.push("--dangerously-bypass-approvals-and-sandbox".to_string()),
            }
            a.push("-C".to_string());
            a.push(ws.to_string_lossy().to_string());
            a
        }
        CliKind::Gemini => {
            let am = match mode {
                PermMode::Ask => "plan",
                PermMode::AutoEdit => "auto_edit",
                PermMode::FullAccess => "yolo",
            };
            ["--output-format", "stream-json", "--approval-mode", am]
                .map(String::from)
                .to_vec()
        }
        CliKind::Qwen => {
            let am = match mode {
                PermMode::Ask => "plan",
                PermMode::AutoEdit => "auto-edit",
                PermMode::FullAccess => "yolo",
            };
            ["--output-format", "stream-json", "--approval-mode", am]
                .map(String::from)
                .to_vec()
        }
        CliKind::OpenCode => ["run", "--format", "json"].map(String::from).to_vec(),
        CliKind::Cursor => {
            let mut a: Vec<String> = ["-p", "--output-format", "stream-json"].map(String::from).to_vec();
            match mode {
                PermMode::Ask => a.extend(["--mode", "plan", "--trust"].map(String::from)),
                PermMode::AutoEdit | PermMode::FullAccess => {
                    a.extend(["--force", "--trust"].map(String::from))
                }
            }
            a
        }
        CliKind::Copilot => {
            let mut a: Vec<String> = ["--output-format", "json"].map(String::from).to_vec();
            if matches!(mode, PermMode::AutoEdit | PermMode::FullAccess) {
                a.push("--allow-all-tools".to_string());
            }
            a
        }
        CliKind::Amp => {
            let mut a: Vec<String> = ["-x", "--stream-json"].map(String::from).to_vec();
            if matches!(mode, PermMode::AutoEdit | PermMode::FullAccess) {
                a.push("--dangerously-allow-all".to_string());
            }
            a
        }
    };
    // Give the CLI the `vx` tools (found on PATH) without touching its own config: Claude and
    // Codex take the usage hint (and Claude's `vx` allow rule) via command-line flags;
    // Antigravity (`agy`) has no system-prompt flag and receives it via `with_vx_hint` in `run`.
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
            _ => {}
        }
    }
    if want_model {
        let flag = match kind {
            CliKind::Codex | CliKind::Gemini | CliKind::Qwen => "-m",
            CliKind::Amp => "--mode",
            _ => "--model",
        };
        args.push(flag.to_string());
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
    let cmds = kind.cmds();
    let Some(exe) = tokio::task::spawn_blocking(move || crate::pty::resolve_cli_tool_any(cmds))
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

    // The CLI edits files on its own: remember the pre-turn state silently, and only surface a
    // checkpoint at the end if the turn actually changed files.
    let mut turn = TurnTracker::new(auto_cp);
    turn.begin(state).await;

    let has_vx = crate::pty::vx_dir().is_some();
    let mut args = cli_args(kind, ws, &req.model, PermMode::parse(req.permission_mode.as_deref()));
    let prompt = if kind == CliKind::Copilot {
        let p = build_prompt_capped(&req.messages, MAX_ARG_PROMPT_CHARS);
        args.push("-p".to_string());
        args.push(p.clone());
        p
    } else {
        with_vx_hint(kind, build_prompt(&req.messages), has_vx)
    };

    let mut command = background_command(&exe.to_string_lossy());
    command
        .args(args)
        .current_dir(ws)
        .env("PATH", crate::pty::augmented_path_env())
        .env("VIREO_WORKSPACE", ws)
        .stdin(if kind == CliKind::Copilot { Stdio::null() } else { Stdio::piped() })
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
    // Claude stays interactive (stream-json): stdin is fed through a channel and only closes
    // once every sender is dropped, so permission answers can still be written mid-turn.
    let mut in_tx: Option<mpsc::Sender<String>> = None;
    if let Some(mut stdin) = child.stdin.take() {
        if kind == CliKind::Claude {
            let (itx, mut irx) = mpsc::channel::<String>(16);
            let _ = itx.send(user_message_line(&prompt)).await;
            tokio::spawn(async move {
                while let Some(mut line) = irx.recv().await {
                    line.push('\n');
                    if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                        break;
                    }
                }
                let _ = stdin.shutdown().await;
            });
            in_tx = Some(itx);
        } else if kind == CliKind::Antigravity {
            let line = agy_user_message_line(&prompt);
            tokio::spawn(async move {
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        } else {
            tokio::spawn(async move {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    }
    let turn_id = uuid::Uuid::new_v4().to_string();
    let _pending_guard = PendingGuard { state: state.clone(), turn_id: turn_id.clone() };
    let mut session_rules: Vec<String> = req.allowed_rules.clone().unwrap_or_default();
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
    let (remember_tx, mut remember_rx) = mpsc::unbounded_channel::<String>();

    loop {
        tokio::select! {
            _ = tx.closed() => {
                // Client pressed Stop / went away: end the CLI and anything it spawned.
                kill_tree(pid).await;
                let _ = child.start_kill();
                turn.finish(state, Some(tx)).await;
                return;
            }
            line = lines.next_line() => {
                let line = match line { Ok(Some(l)) => l, _ => break };
                let line = line.trim();
                if line.is_empty() { continue; }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                for ev in norm.feed(&v) {
                    match ev {
                        Out::PermissionRequest { request_id, tool, input } => {
                            let Some(itx) = in_tx.clone() else { continue };
                            if session_rules.iter().any(|r| rule_allows(r, &tool, &input)) {
                                let _ = itx.send(control_response(&request_id, true, &input)).await;
                                continue;
                            }
                            let rule = permission_rule(&tool, &input);
                            let (dtx, drx) = oneshot::channel::<PermissionDecision>();
                            if let Ok(mut m) = state.pending_permissions.lock() {
                                m.insert((turn_id.clone(), request_id.clone()), dtx);
                            }
                            send_sse(
                                tx,
                                "permission_request",
                                json!({
                                    "turn_id": turn_id,
                                    "request_id": request_id,
                                    "tool": tool,
                                    "input": input,
                                    "summary": permission_summary(&tool, &input),
                                    "rule": rule,
                                }),
                            )
                            .await;
                            let (rid, remember_tx) = (request_id, remember_tx.clone());
                            // Waits outside the read loop so Stop and other output keep flowing.
                            tokio::spawn(async move {
                                // A dropped sender (turn ended / request cancelled) means no answer.
                                let Ok(d) = drx.await else { return };
                                if d.allow && d.remember {
                                    let _ = remember_tx.send(rule);
                                }
                                let _ = itx.send(control_response(&rid, d.allow, &input)).await;
                            });
                        }
                        Out::PermissionCancel { request_id } => {
                            if let Ok(mut m) = state.pending_permissions.lock() {
                                m.remove(&(turn_id.clone(), request_id.clone()));
                            }
                            send_sse(tx, "permission_cancel", json!({ "request_id": request_id })).await;
                        }
                        Out::UnsupportedControl { request_id } => {
                            if let Some(itx) = &in_tx {
                                let _ = itx.send(control_error(&request_id)).await;
                            }
                        }
                        ev => {
                            if let Out::ToolCall { name, args, .. } = &ev {
                                turn.note_tool(name, args);
                            }
                            emit(state, tx, ev).await;
                        }
                    }
                }
                while let Ok(rule) = remember_rx.try_recv() {
                    if !session_rules.contains(&rule) {
                        session_rules.push(rule);
                    }
                }
                // Claude waits for another user message after it finishes: close stdin so it exits.
                if norm.finished && in_tx.is_some() {
                    in_tx = None;
                    if let Ok(mut m) = state.pending_permissions.lock() {
                        m.retain(|(t, _), _| t != &turn_id);
                    }
                }
            }
        }
    }

    for ev in norm.finish_eof() {
        emit(state, tx, ev).await;
    }

    let status = child.wait().await.ok();
    let stderr_text = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    let failed = status.map(|s| !s.success()).unwrap_or(true);
    let clean_eof = !failed && matches!(kind, CliKind::OpenCode | CliKind::Copilot) && norm.emitted_text;
    if !norm.errored && (failed || (!norm.finished && !clean_eof)) {
        let detail = stderr_text.trim();
        let code = status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        let msg = if detail.is_empty() {
            format!("{} exited unexpectedly (code {code}).", kind.label())
        } else {
            format!("{} exited with code {code}: {}", kind.label(), tail_chars(detail, 800))
        };
        send_sse(tx, "error", json!({ "message": msg })).await;
    }
    turn.finish(state, Some(tx)).await;
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
        // Handled by `run`, which owns the stdin / pending-permission state.
        Out::PermissionRequest { .. } | Out::PermissionCancel { .. } | Out::UnsupportedControl { .. } => {}
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
    fn claude_powershell_maps_to_run_command() {
        let out = feed_all(
            CliKind::Claude,
            r"C:\proj",
            &[
                r#"{"type":"assistant","message":{"model":"claude-x","content":[{"type":"tool_use","id":"tu2","name":"PowerShell","input":{"command":"Get-ChildItem"}}]}}"#,
            ],
        );
        assert_eq!(
            out[0],
            Out::ToolCall { id: "tu2".into(), name: "run_command".into(), args: json!({"command": "Get-ChildItem"}) }
        );
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
        let m = PermMode::AutoEdit;
        assert!(!cli_args(CliKind::Codex, ws, "default", m).contains(&"-m".to_string()));
        assert!(cli_args(CliKind::Codex, ws, "gpt-5.6-luna", m).contains(&"-m".to_string()));
        assert!(cli_args(CliKind::Claude, ws, "sonnet", m).contains(&"--model".to_string()));
    }

    #[test]
    fn permission_modes_map_to_cli_flags() {
        let ws = Path::new("C:/proj");
        let has = |k, m, f: &str| cli_args(k, ws, "default", m).iter().any(|a| a == f);
        assert!(has(CliKind::Claude, PermMode::Ask, "default"));
        assert!(has(CliKind::Claude, PermMode::AutoEdit, "acceptEdits"));
        assert!(has(CliKind::Claude, PermMode::FullAccess, "bypassPermissions"));
        assert!(has(CliKind::Claude, PermMode::Ask, "stdio"));
        assert!(has(CliKind::Codex, PermMode::Ask, "read-only"));
        assert!(has(CliKind::Codex, PermMode::AutoEdit, "workspace-write"));
        assert!(has(CliKind::Codex, PermMode::FullAccess, "--dangerously-bypass-approvals-and-sandbox"));
        assert_eq!(PermMode::parse(None), PermMode::AutoEdit);
        assert_eq!(PermMode::parse(Some("ask")), PermMode::Ask);
    }

    #[test]
    fn claude_control_requests_are_normalized() {
        let out = feed_all(
            CliKind::Claude,
            "C:/w",
            &[
                r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"C:\\Users\\u\\.claude\\CLAUDE.md"}}}"#,
                r#"{"type":"control_request","request_id":"r2","request":{"subtype":"hook_callback"}}"#,
                r#"{"type":"control_cancel_request","request_id":"r1"}"#,
            ],
        );
        assert!(matches!(&out[0], Out::PermissionRequest { request_id, tool, .. } if request_id == "r1" && tool == "Edit"));
        assert_eq!(out[1], Out::UnsupportedControl { request_id: "r2".into() });
        assert_eq!(out[2], Out::PermissionCancel { request_id: "r1".into() });
    }

    #[test]
    fn control_response_shapes() {
        let input = json!({ "command": "ls" });
        let allow: Value = serde_json::from_str(&control_response("r1", true, &input)).unwrap();
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(allow["response"]["response"]["updatedInput"], input);
        let deny: Value = serde_json::from_str(&control_response("r1", false, &input)).unwrap();
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert!(deny["response"]["response"]["message"].is_string());
    }

    #[test]
    fn remembered_rules_are_narrow() {
        let edit = json!({ "file_path": "C:\\Users\\U\\.claude\\CLAUDE.md" });
        let rule = permission_rule("Edit", &edit);
        assert_eq!(rule, "Edit:dir:c:/users/u/.claude");
        assert!(rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude/settings.json" })));
        assert!(!rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude-other/x" })));
        assert!(!rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude/../.ssh/id" })));
        assert!(!rule_allows(&rule, "Write", &edit));

        let sh = permission_rule("Bash", &json!({ "command": "cargo build" }));
        assert!(rule_allows(&sh, "Bash", &json!({ "command": "cargo test -p x" })));
        assert!(!rule_allows(&sh, "Bash", &json!({ "command": "cargo build && rm -rf x" })));
        assert!(!rule_allows(&sh, "Bash", &json!({ "command": "rm -rf x" })));
        assert!(rule_allows("WebFetch", "WebFetch", &json!({ "url": "https://x" })));
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

    #[test]
    fn antigravity_real_stream() {
        // Captured from agy.exe `--input-format stream-json --output-format stream-json`.
        let ws = r"C:\Users\user\Documents\Developer\vireo";
        let mut n = Normalizer::new(CliKind::Antigravity, Path::new(ws));
        let lines = [
            r#"{"event":"init","conversation_id":"c1","init":{"cwd":"C:\\Users\\user\\Documents\\Developer\\vireo","tools":["read_file","write_to_file"],"permission_mode":"request-review"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":0,"state":"DONE","step_type":"user_input"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"EndLine":3,"Path":"C:\\Users\\user\\Documents\\Developer\\vireo\\backend\\Cargo.toml","StartLine":1}}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":2,"state":"DONE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"EndLine":3,"Path":"C:\\Users\\user\\Documents\\Developer\\vireo\\backend\\Cargo.toml","StartLine":1},"output":"50 lines, 1165 bytes"}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":3,"state":"ACTIVE","step_type":"agent_response","text_delta":"vi"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":3,"state":"ACTIVE","step_type":"agent_response","text_delta":"reo"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":3,"state":"DONE","step_type":"agent_response","text_delta":"\n"}}"#,
            r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"vireo\n","num_turns":1}}"#,
        ];
        let mut out: Vec<Out> = lines
            .iter()
            .flat_map(|l| n.feed(&serde_json::from_str::<Value>(l).unwrap()))
            .collect();
        out.extend(n.finish_eof());
        assert!(n.finished && !n.errored);
        assert_eq!(
            out,
            vec![
                Out::ToolCall {
                    id: "step_2".into(),
                    name: "read_file".into(),
                    args: json!({"path": "backend/Cargo.toml", "start_line": 1, "end_line": 3}),
                },
                Out::ToolResult {
                    id: "step_2".into(),
                    name: "read_file".into(),
                    result: "50 lines, 1165 bytes".into(),
                },
                Out::Text("vireo".into()),
            ]
        );
    }

    #[test]
    fn antigravity_error_result() {
        let mut n = Normalizer::new(CliKind::Antigravity, Path::new("C:/proj"));
        let out = n.feed(&json!({
            "event": "result",
            "result": { "status": "ERROR", "response": "", "error": "not authenticated" }
        }));
        assert_eq!(out, vec![Out::Error("not authenticated".into())]);
        assert!(n.finished && n.errored);
    }

    #[test]
    fn antigravity_tool_error_object_and_denied_actions() {
        // Captured from agy.exe when a command is denied in headless mode.
        let out = feed_all(
            CliKind::Antigravity,
            r"C:\proj",
            &[
                r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"}}}}"#,
                r#"{"event":"step_update","step_update":{"step_index":2,"state":"ERROR","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"},"error":{"type":"TOOL_ERROR","message":"permission check failed"}}}}"#,
                r#"{"event":"result","result":{"status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"Bash"}]}}"#,
            ],
        );
        assert_eq!(
            out[0],
            Out::ToolCall { id: "step_2".into(), name: "run_command".into(), args: json!({"command": "echo hello"}) }
        );
        assert_eq!(
            out[1],
            Out::ToolResult { id: "step_2".into(), name: "run_command".into(), result: "Error: permission check failed".into() }
        );
        assert!(matches!(&out[2], Out::Error(msg) if msg.contains("Bash")));
    }

    #[test]
    fn gemini_and_qwen_streams_normalize() {
        let out = feed_all(
            CliKind::Gemini,
            r"C:\proj",
            &[
                r#"{"type":"init","session_id":"s1","model":"gemini-2.5-pro"}"#,
                r#"{"type":"message","role":"assistant","content":"Checking ","delta":true}"#,
                r#"{"type":"message","role":"assistant","content":"repo.","delta":true}"#,
                r#"{"type":"tool_use","tool_id":"t1","tool_name":"run_shell_command","parameters":{"command":"git status"}}"#,
                r#"{"type":"tool_result","tool_id":"t1","output":"clean","status":"success"}"#,
                r#"{"type":"message","role":"assistant","content":"All clean."}"#,
                r#"{"type":"result","status":"success"}"#,
            ],
        );
        assert_eq!(out[0], Out::Text("Checking repo.".into()));
        assert_eq!(
            out[1],
            Out::ToolCall { id: "t1".into(), name: "run_command".into(), args: json!({"command": "git status"}) }
        );
        assert_eq!(
            out[2],
            Out::ToolResult { id: "t1".into(), name: "run_command".into(), result: "clean".into() }
        );
        assert_eq!(out[3], Out::Text("All clean.".into()));
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn opencode_stream_normalizes() {
        let out = feed_all(
            CliKind::OpenCode,
            r"C:\proj",
            &[
                r#"{"type":"step_start","part":{"id":"p1"}}"#,
                r#"{"type":"tool_use","part":{"callID":"c1","tool":"bash","state":{"status":"completed","input":{"command":"cargo test"},"output":"ok","metadata":{"exitCode":0}}}}"#,
                r#"{"type":"text","part":{"text":"Tests pass."}}"#,
                r#"{"type":"step_finish","part":{"reason":"stop"}}"#,
            ],
        );
        assert_eq!(
            out,
            vec![
                Out::ToolCall { id: "c1".into(), name: "run_command".into(), args: json!({"command": "cargo test"}) },
                Out::ToolResult { id: "c1".into(), name: "run_command".into(), result: "exit=0\nok".into() },
                Out::Text("Tests pass.".into()),
            ]
        );
    }

    #[test]
    fn cursor_stream_normalizes() {
        let out = feed_all(
            CliKind::Cursor,
            r"C:\proj",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading file."}]}}"#,
                r#"{"type":"tool_call","subtype":"started","call_id":"k1","tool_call":{"readToolCall":{"args":{"path":"C:\\proj\\src\\a.ts","offset":1,"limit":10}}}}"#,
                r#"{"type":"tool_call","subtype":"completed","call_id":"k1","tool_call":{"readToolCall":{"result":{"success":{"content":"export const x = 1;"}}}}}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"result":"Done."}"#,
            ],
        );
        assert_eq!(out[0], Out::Text("Reading file.".into()));
        assert_eq!(
            out[1],
            Out::ToolCall { id: "k1".into(), name: "read_file".into(), args: json!({"path": "src/a.ts", "start_line": 1, "end_line": 10}) }
        );
        assert_eq!(
            out[2],
            Out::ToolResult { id: "k1".into(), name: "read_file".into(), result: "export const x = 1;".into() }
        );
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn copilot_stream_normalizes() {
        let out = feed_all(
            CliKind::Copilot,
            r"C:\proj",
            &[
                r#"{"type":"tool.execution_start","data":{"toolCallId":"tc1","toolName":"edit","arguments":{"path":"C:\\proj\\a.rs"}}}"#,
                r#"{"type":"tool.execution_complete","data":{"toolCallId":"tc1","success":true,"result":{"content":"updated"}}}"#,
                r#"{"type":"assistant.message","data":{"content":"Updated a.rs."}}"#,
                r#"{"type":"session.task_complete","data":{"summary":"Updated a.rs."}}"#,
            ],
        );
        assert_eq!(
            out,
            vec![
                Out::ToolCall { id: "tc1".into(), name: "edit_file".into(), args: json!({"path": "a.rs"}) },
                Out::ToolResult { id: "tc1".into(), name: "edit_file".into(), result: "updated".into() },
                Out::Text("Updated a.rs.".into()),
            ]
        );
    }

    #[test]
    fn new_cli_args_and_modes() {
        let ws = Path::new("C:/proj");
        let has = |k, model, m, f: &str| cli_args(k, ws, model, m).iter().any(|a| a == f);
        assert!(has(CliKind::Antigravity, "default", PermMode::Ask, "plan"));
        assert!(has(CliKind::Antigravity, "default", PermMode::AutoEdit, "--dangerously-skip-permissions"));
        assert!(has(CliKind::Antigravity, "default", PermMode::FullAccess, "--dangerously-skip-permissions"));
        assert!(has(CliKind::Antigravity, "gemini-4-neon-high", PermMode::AutoEdit, "--model"));
        assert!(has(CliKind::Gemini, "gemini-2.5-pro", PermMode::AutoEdit, "auto_edit"));
        assert!(has(CliKind::Gemini, "gemini-2.5-pro", PermMode::FullAccess, "yolo"));
        assert!(has(CliKind::Qwen, "qwen3-coder", PermMode::AutoEdit, "auto-edit"));
        assert!(has(CliKind::OpenCode, "default", PermMode::AutoEdit, "run"));
        assert!(has(CliKind::Cursor, "default", PermMode::AutoEdit, "--force"));
        assert!(has(CliKind::Copilot, "default", PermMode::AutoEdit, "--allow-all-tools"));
        assert!(has(CliKind::Amp, "rush", PermMode::AutoEdit, "--mode"));
    }

    #[test]
    fn antigravity_prompt_includes_vx_hint() {
        let p = with_vx_hint(CliKind::Antigravity, "fix the bug".into(), true);
        assert!(p.contains(crate::code_intel::AGENT_HINT));
        assert!(p.ends_with("fix the bug"));

        // No hint when vx is absent, or for CLIs that receive it via flags.
        assert_eq!(with_vx_hint(CliKind::Antigravity, "fix the bug".into(), false), "fix the bug");
        assert_eq!(with_vx_hint(CliKind::Claude, "fix the bug".into(), true), "fix the bug");
        assert_eq!(with_vx_hint(CliKind::Codex, "fix the bug".into(), true), "fix the bug");
        assert_eq!(with_vx_hint(CliKind::Antigravity, String::new(), true), "");
    }
}
