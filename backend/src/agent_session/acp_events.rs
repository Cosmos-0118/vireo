//! Translates Agent Client Protocol `session/update` notifications and permission requests into
//! the chat timeline's normalized events. Pure and synchronous so it can be tested on captures.

use super::commands;
use super::types::SlashCommand;
use crate::cli_agent::{map_vx, rel_path, PermMode, Out};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct AcpEvents {
    ws: PathBuf,
    /// Tool-call id -> normalized tool name.
    names: HashMap<String, String>,
    done: HashSet<String>,
    /// Latest `available_commands_update`, if the agent sent one.
    pub commands: Option<Vec<SlashCommand>>,
}

impl AcpEvents {
    pub fn new(ws: &Path) -> Self {
        Self { ws: ws.to_path_buf(), names: HashMap::new(), done: HashSet::new(), commands: None }
    }

    /// The `update` object of a `session/update` notification.
    pub fn feed(&mut self, update: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => {
                if let Some(t) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                    out.push(Out::TextDelta(t.to_string()));
                }
            }
            Some("tool_call") => {
                let id = update["toolCallId"].as_str().unwrap_or("tool").to_string();
                if !self.names.contains_key(&id) {
                    let (name, args) = self.map_tool(update);
                    self.names.insert(id.clone(), name.clone());
                    out.push(Out::ToolCall { id: id.clone(), name, args });
                }
                self.maybe_result(&id, update, &mut out);
            }
            Some("tool_call_update") => {
                let id = update["toolCallId"].as_str().unwrap_or("tool").to_string();
                self.maybe_result(&id, update, &mut out);
            }
            Some("available_commands_update") => {
                self.commands = Some(commands::from_acp(&update["availableCommands"]));
            }
            _ => {}
        }
        out
    }

    fn maybe_result(&mut self, id: &str, update: &Value, out: &mut Vec<Out>) {
        let status = update["status"].as_str().unwrap_or("");
        if !matches!(status, "completed" | "failed") || self.done.contains(id) {
            return;
        }
        self.done.insert(id.to_string());
        let name = self.names.get(id).cloned().unwrap_or_else(|| "tool".to_string());
        let text = result_text(update);
        let result = if status == "failed" && !text.starts_with("Error") { format!("Error: {text}") } else { text };
        out.push(Out::ToolResult { id: id.to_string(), name, result });
    }

    fn path_of(&self, v: &Value) -> String {
        let raw = v["locations"][0]["path"]
            .as_str()
            .or_else(|| ["path", "file_path", "filePath", "filepath"].iter().find_map(|k| v["rawInput"][*k].as_str()))
            .or_else(|| v["content"].as_array().into_iter().flatten().find_map(|c| c["path"].as_str()));
        raw.map(|p| rel_path(&self.ws, p)).unwrap_or_default()
    }

    fn map_tool(&self, v: &Value) -> (String, Value) {
        let title = v["title"].as_str().unwrap_or("tool");
        match v["kind"].as_str() {
            Some("read") => ("read_file".into(), json!({ "path": self.path_of(v) })),
            Some("edit") | Some("delete") | Some("move") => ("edit_file".into(), json!({ "path": self.path_of(v) })),
            Some("search") => ("search_code".into(), json!({ "query": title })),
            Some("execute") => {
                let command = v["rawInput"]["command"].as_str().unwrap_or(title).to_string();
                map_vx(&command, &self.ws).unwrap_or_else(|| {
                    let mut args = json!({ "command": command });
                    // ACP titles are often the agent's own description of the command.
                    if let Some(d) = intent_of(v, &command) {
                        args["description"] = json!(d);
                    }
                    ("run_command".into(), args)
                })
            }
            _ => (title.to_string(), if v["rawInput"].is_object() { v["rawInput"].clone() } else { json!({}) }),
        }
    }

    /// `(tool, input)` for a `session/request_permission` card.
    pub fn permission_card(&self, params: &Value) -> (String, Value) {
        let tc = &params["toolCall"];
        let title = tc["title"].as_str().unwrap_or("tool");
        match tc["kind"].as_str() {
            Some("execute") => {
                let command = tc["rawInput"]["command"].as_str().unwrap_or(title);
                let mut input = json!({ "command": command });
                if let Some(d) = intent_of(tc, command) {
                    input["description"] = json!(d);
                }
                (shell_label().to_string(), input)
            }
            Some("edit") | Some("delete") | Some("move") => {
                ("Edit".to_string(), json!({ "file_path": self.path_of(tc), "title": title }))
            }
            _ => (title.to_string(), if tc["rawInput"].is_object() { tc["rawInput"].clone() } else { json!({}) }),
        }
    }
}

/// Human-readable intent of a tool call: `rawInput.description`, else the title when it isn't just the command.
fn intent_of(v: &Value, command: &str) -> Option<String> {
    let clean = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s != command.trim()).then(|| s.to_string())
    };
    v["rawInput"]["description"]
        .as_str()
        .and_then(clean)
        .or_else(|| v["title"].as_str().and_then(clean))
}

fn shell_label() -> &'static str {
    if cfg!(windows) {
        "PowerShell"
    } else {
        "Bash"
    }
}

/// Plain text of a tool result: text blocks, diff summaries, else the raw output.
fn result_text(update: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in update["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("content") => {
                if let Some(t) = c["content"]["text"].as_str() {
                    parts.push(t.to_string());
                }
            }
            Some("diff") => parts.push(format!("Updated {}", c["path"].as_str().unwrap_or("file"))),
            _ => {}
        }
    }
    if parts.is_empty() {
        if let Some(s) = update["rawOutput"]["content"].as_str().or_else(|| update["rawOutput"].as_str()) {
            parts.push(s.to_string());
        }
    }
    parts.join("\n").trim_end().to_string()
}

/// Does Vireo's permission mode approve a tool of this ACP `kind` without asking?
pub fn auto_allows(mode: PermMode, kind: Option<&str>) -> bool {
    let read_only = matches!(kind, Some("read" | "search" | "think"));
    match mode {
        PermMode::FullAccess => true,
        PermMode::AutoEdit => read_only || matches!(kind, Some("edit")),
        PermMode::Ask => read_only,
    }
}

/// The `outcome` for a permission request: the matching option, or `cancelled` if none fits.
pub fn permission_outcome(options: &Value, allow: bool, remember: bool) -> Value {
    let wanted: &[&str] = match (allow, remember) {
        (true, true) => &["allow_always", "allow_once"],
        (true, false) => &["allow_once", "allow_always"],
        (false, true) => &["reject_always", "reject_once"],
        (false, false) => &["reject_once", "reject_always"],
    };
    for kind in wanted {
        let hit = options.as_array().into_iter().flatten().find(|o| o["kind"].as_str() == Some(kind));
        if let Some(id) = hit.and_then(|o| o["optionId"].as_str()) {
            return json!({ "outcome": "selected", "optionId": id });
        }
    }
    json!({ "outcome": "cancelled" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(lines: &[&str]) -> (AcpEvents, Vec<Out>) {
        let mut ev = AcpEvents::new(Path::new("C:/proj"));
        let outs = lines.iter().flat_map(|l| ev.feed(&serde_json::from_str::<Value>(l).unwrap())).collect();
        (ev, outs)
    }

    #[test]
    fn copilot_command_turn_normalizes() {
        // Captured from `copilot --acp` 1.0.114.
        let (_, outs) = feed(&[
            r#"{"sessionUpdate":"tool_call","toolCallId":"c1","title":"Run the requested echo command","kind":"execute","status":"pending","rawInput":{"command":"echo hello-acp","description":"x"}}"#,
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"c1","content":[{"type":"content","content":{"type":"text","text":"hello-acp\n"}}]}"#,
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"c1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"hello-acp\n<shellId: 0 completed with exit code 0>"}}]}"#,
            r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"},"messageId":"m"}"#,
            r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"-acp"},"messageId":"m"}"#,
        ]);
        assert_eq!(
            outs,
            vec![
                Out::ToolCall { id: "c1".into(), name: "run_command".into(), args: json!({ "command": "echo hello-acp", "description": "x" }) },
                Out::ToolResult {
                    id: "c1".into(),
                    name: "run_command".into(),
                    result: "hello-acp\n<shellId: 0 completed with exit code 0>".into()
                },
                Out::TextDelta("hello".into()),
                Out::TextDelta("-acp".into()),
            ]
        );
    }

    #[test]
    fn edits_use_locations_and_results_report_once() {
        let (_, outs) = feed(&[
            r#"{"sessionUpdate":"tool_call","toolCallId":"e1","title":"Edit a.rs","kind":"edit","status":"pending","locations":[{"path":"C:/proj/src/a.rs"}]}"#,
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed","content":[{"type":"diff","path":"C:/proj/src/a.rs","oldText":"a","newText":"b"}]}"#,
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed"}"#,
        ]);
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0], Out::ToolCall { id: "e1".into(), name: "edit_file".into(), args: json!({ "path": "src/a.rs" }) });
        assert_eq!(
            outs[1],
            Out::ToolResult { id: "e1".into(), name: "edit_file".into(), result: "Updated C:/proj/src/a.rs".into() }
        );
    }

    #[test]
    fn available_commands_are_kept() {
        let (ev, outs) = feed(&[
            r#"{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"add-dir","description":"Allow a dir","input":{"hint":"directory"}}]}"#,
        ]);
        assert!(outs.is_empty());
        assert_eq!(ev.commands.unwrap()[0].hint.as_deref(), Some("directory"));
    }

    #[test]
    fn permission_cards_and_outcomes() {
        let ev = AcpEvents::new(Path::new("C:/proj"));
        let params: Value = serde_json::from_str(
            r#"{"toolCall":{"toolCallId":"c1","title":"echo hello-acp","kind":"execute","rawInput":{"command":"echo hello-acp"}},
                "options":[{"optionId":"allow_once","kind":"allow_once","name":"Allow once"},
                           {"optionId":"allow_always","kind":"allow_always","name":"Always allow"},
                           {"optionId":"reject_once","kind":"reject_once","name":"Deny"}]}"#,
        )
        .unwrap();
        let (tool, input) = ev.permission_card(&params);
        assert!(tool == "PowerShell" || tool == "Bash");
        assert_eq!(input, json!({ "command": "echo hello-acp" }));
        let opts = &params["options"];
        assert_eq!(permission_outcome(opts, true, false), json!({ "outcome": "selected", "optionId": "allow_once" }));
        assert_eq!(permission_outcome(opts, true, true), json!({ "outcome": "selected", "optionId": "allow_always" }));
        assert_eq!(permission_outcome(opts, false, false), json!({ "outcome": "selected", "optionId": "reject_once" }));
        // No reject_always was offered, so "deny and remember" falls back to a plain reject.
        assert_eq!(permission_outcome(opts, false, true), json!({ "outcome": "selected", "optionId": "reject_once" }));
        assert_eq!(permission_outcome(&json!([]), true, false), json!({ "outcome": "cancelled" }));
    }

    #[test]
    fn modes_decide_what_needs_asking() {
        assert!(auto_allows(PermMode::Ask, Some("read")));
        assert!(!auto_allows(PermMode::Ask, Some("edit")));
        assert!(auto_allows(PermMode::AutoEdit, Some("edit")));
        assert!(!auto_allows(PermMode::AutoEdit, Some("execute")));
        assert!(auto_allows(PermMode::FullAccess, Some("execute")));
        assert!(!auto_allows(PermMode::Ask, None));
    }
}
