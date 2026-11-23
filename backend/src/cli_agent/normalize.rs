//! Translates Claude Code's stream-json events into normalized [`Out`] events for the chat timeline.

use super::tools::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One normalized event destined for the chat timeline.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Claude wants to use a tool that needs the user's approval.
    PermissionRequest { request_id: String, tool: String, input: Value },
    /// Claude withdrew a pending permission request.
    PermissionCancel { request_id: String },
    /// A control request we do not implement; must be answered with an error or the CLI waits.
    UnsupportedControl { request_id: String },
    /// The agent asks the user multiple-choice questions (Claude's `AskUserQuestion`).
    Question { request_id: String, input: Value },
    /// The agent's own id for this conversation, usable to resume it after a restart.
    Session(String),
    Text(String),
    /// A streamed fragment of the assistant's reply (token-level streaming).
    TextDelta(String),
    ToolCall { id: String, name: String, args: Value },
    ToolResult { id: String, name: String, result: String },
    Error(String),
}

/// Stateful translator from Claude Code's JSON lines to [`Out`] events.
pub struct Normalizer {
    ws: PathBuf,
    /// tool-call id -> normalized tool name (so results can be labelled).
    names: HashMap<String, String>,
    /// The CLI reported a final result for the current turn.
    pub finished: bool,
    /// `--include-partial-messages`: ids of assistant messages whose text already arrived as
    /// deltas, so the final `assistant` event must not repeat it.
    streamed_msgs: HashSet<String>,
    /// Id of the assistant message currently streaming (top level only).
    cur_msg: Option<String>,
    /// A new assistant message started after text was already shown this turn, so its first
    /// delta needs a paragraph break.
    pending_break: bool,
    /// Whether any text (streamed or whole) was shown during the current turn.
    turn_text: bool,
    /// The session id was already reported.
    session_sent: bool,
}

impl Normalizer {
    pub fn new(ws: &Path) -> Self {
        Self {
            ws: ws.to_path_buf(),
            names: HashMap::new(),
            finished: false,
            streamed_msgs: HashSet::new(),
            cur_msg: None,
            pending_break: false,
            turn_text: false,
            session_sent: false,
        }
    }

    /// Resets per-turn flags when the long-lived session starts another turn.
    pub fn begin_turn(&mut self) {
        self.finished = false;
        self.turn_text = false;
        self.pending_break = false;
        self.cur_msg = None;
        self.streamed_msgs.clear();
    }

    pub fn feed(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("control_request") => {
                let request_id = v["request_id"].as_str().unwrap_or("").to_string();
                let req = &v["request"];
                if req["subtype"].as_str() == Some("can_use_tool") && !request_id.is_empty() {
                    if req["tool_name"].as_str() == Some("AskUserQuestion") {
                        out.push(Out::Question { request_id, input: req["input"].clone() });
                    } else {
                        out.push(Out::PermissionRequest {
                            request_id,
                            tool: req["tool_name"].as_str().unwrap_or("tool").to_string(),
                            input: req["input"].clone(),
                        });
                    }
                } else if !request_id.is_empty() {
                    out.push(Out::UnsupportedControl { request_id });
                }
            }
            Some("control_cancel_request") => {
                if let Some(id) = v["request_id"].as_str() {
                    out.push(Out::PermissionCancel { request_id: id.to_string() });
                }
            }
            Some("system") => {
                if v["subtype"].as_str() == Some("status") && v["compact_result"].as_str() == Some("failed") {
                    let why = v["compact_error"].as_str().unwrap_or("unknown reason");
                    self.turn_text = true;
                    out.push(Out::Text(format!("Could not compact the conversation: {why}")));
                }
                if v["subtype"].as_str() == Some("init") && !self.session_sent {
                    if let Some(id) = v["session_id"].as_str() {
                        self.session_sent = true;
                        out.push(Out::Session(id.to_string()));
                    }
                }
            }
            // Token-level streaming (`--include-partial-messages`). Sub-agent output is skipped
            // here; it still arrives whole through the `assistant` event.
            Some("stream_event") => {
                if !v["parent_tool_use_id"].is_null() {
                    return out;
                }
                let ev = &v["event"];
                match ev["type"].as_str() {
                    Some("message_start") => {
                        self.cur_msg = ev["message"]["id"].as_str().map(str::to_string);
                        self.pending_break = self.turn_text;
                    }
                    Some("content_block_delta") if ev["delta"]["type"].as_str() == Some("text_delta") => {
                        let Some(text) = ev["delta"]["text"].as_str().filter(|t| !t.is_empty()) else {
                            return out;
                        };
                        if let Some(id) = &self.cur_msg {
                            self.streamed_msgs.insert(id.clone());
                        }
                        let text = if std::mem::take(&mut self.pending_break) {
                            format!("\n\n{text}")
                        } else {
                            text.to_string()
                        };
                        self.turn_text = true;
                        out.push(Out::TextDelta(text));
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                let msg = &v["message"];
                // Claude Code fabricates "synthetic" assistant messages for errors such as
                // "Not logged in"; the matching `result` event reports them, so skip the echo.
                if msg["model"].as_str() == Some("<synthetic>") {
                    return out;
                }
                let streamed = msg["id"].as_str().is_some_and(|id| self.streamed_msgs.contains(id));
                for block in msg["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if streamed => {}
                        Some("text") => {
                            if let Some(t) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                self.turn_text = true;
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
                // Output of a local slash command (`/context`, `/cost`, …) is echoed as a user
                // message wrapped in `<local-command-stdout>`.
                if let Some(text) = local_command_output(&v["message"]["content"]) {
                    out.push(Out::Text(text));
                    return out;
                }
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
                // A slash command's own output (`/context`, `/cost`) only appears here: its
                // assistant echo is a "<synthetic>" message that is skipped above.
                if v["is_error"].as_bool() != Some(true) && !self.turn_text {
                    if let Some(t) = v["result"].as_str().filter(|t| !t.trim().is_empty()) {
                        self.turn_text = true;
                        out.push(Out::Text(t.to_string()));
                    }
                }
                if v["is_error"].as_bool() == Some(true) {
                    let label = "Claude Code";
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
}
