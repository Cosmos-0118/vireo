//! Translates Codex `app-server` (v2 protocol) notifications and approval requests into the chat
//! timeline's normalized events. Pure and synchronous so it can be tested against captured streams.

use super::broker::{Question, QuestionOption};
use crate::cli_agent::{map_vx, simplify_command, Out};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What an approval request from Codex needs from the user.
#[derive(Debug, PartialEq)]
pub enum Approval {
    /// A shell command (shown as a `Bash` / `PowerShell` card).
    Command { command: String, reason: Option<String> },
    /// File edits (shown as an `Edit` card).
    FileChange { path: String, reason: Option<String> },
    /// Extra permissions such as network access.
    Permissions { reason: Option<String> },
}

pub struct CodexEvents {
    ws: PathBuf,
    /// Item id -> normalized tool name, so results can be labelled.
    names: HashMap<String, String>,
    /// `agentMessage` items whose text already arrived as deltas.
    streamed: HashSet<String>,
    /// Item id -> workspace-relative paths of a `fileChange` item.
    file_paths: HashMap<String, Vec<String>>,
    turn_text: bool,
    pending_break: bool,
    /// The turn reported completion.
    pub finished: bool,
}

impl CodexEvents {
    pub fn new(ws: &Path) -> Self {
        Self {
            ws: ws.to_path_buf(),
            names: HashMap::new(),
            streamed: HashSet::new(),
            file_paths: HashMap::new(),
            turn_text: false,
            pending_break: false,
            finished: false,
        }
    }

    pub fn begin_turn(&mut self) {
        self.finished = false;
        self.turn_text = false;
        self.pending_break = false;
        self.streamed.clear();
    }

    /// One server notification (`method` + `params`) to zero or more timeline events.
    pub fn feed(&mut self, method: &str, p: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match method {
            "item/agentMessage/delta" => {
                let Some(delta) = p["delta"].as_str().filter(|d| !d.is_empty()) else { return out };
                if let Some(id) = p["itemId"].as_str() {
                    self.streamed.insert(id.to_string());
                }
                let delta = if std::mem::take(&mut self.pending_break) { format!("\n\n{delta}") } else { delta.to_string() };
                self.turn_text = true;
                out.push(Out::TextDelta(delta));
            }
            "item/started" => self.item_started(&p["item"], &mut out),
            "item/completed" => self.item_completed(&p["item"], &mut out),
            "turn/completed" => {
                self.finished = true;
                let turn = &p["turn"];
                if turn["status"].as_str() == Some("failed") {
                    let msg = turn["error"]["message"].as_str().unwrap_or("Codex turn failed.");
                    out.push(Out::Error(msg.to_string()));
                }
            }
            _ => {}
        }
        out
    }

    fn rel(&self, path: &str) -> String {
        crate::cli_agent::rel_path(&self.ws, path)
    }

    fn item_started(&mut self, item: &Value, out: &mut Vec<Out>) {
        let id = item["id"].as_str().unwrap_or("item").to_string();
        match item["type"].as_str() {
            Some("agentMessage") => self.pending_break = self.turn_text,
            Some("commandExecution") => {
                let command = display_command(item);
                let (name, args) = map_vx(&command, &self.ws)
                    .unwrap_or_else(|| ("run_command".to_string(), json!({ "command": command })));
                self.names.insert(id.clone(), name.clone());
                out.push(Out::ToolCall { id, name, args });
            }
            Some("fileChange") => {
                let paths = item["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["path"].as_str())
                    .map(|p| self.rel(p))
                    .collect();
                self.file_paths.insert(id, paths);
            }
            Some("mcpToolCall") => {
                let (server, tool) = (item["server"].as_str().unwrap_or(""), item["tool"].as_str().unwrap_or(""));
                let args_val = item.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let (name, args) = if server == "vx" {
                    crate::cli_agent::map_vx_mcp_tool(tool, &args_val, &self.ws)
                } else {
                    (format!("mcp__{server}__{tool}"), args_val)
                };
                self.names.insert(id.clone(), name.clone());
                out.push(Out::ToolCall { id, name, args });
            }
            _ => {}
        }
    }

    fn item_completed(&mut self, item: &Value, out: &mut Vec<Out>) {
        let id = item["id"].as_str().unwrap_or("item").to_string();
        match item["type"].as_str() {
            Some("agentMessage") => {
                if !self.streamed.contains(&id) {
                    if let Some(t) = item["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        let t = if std::mem::take(&mut self.pending_break) { format!("\n\n{t}") } else { t.to_string() };
                        self.turn_text = true;
                        out.push(Out::TextDelta(t));
                    }
                }
            }
            Some("commandExecution") => {
                let name = self.names.get(&id).cloned().unwrap_or_else(|| "run_command".to_string());
                let output = item["aggregatedOutput"].as_str().unwrap_or("");
                let declined = item["status"].as_str() == Some("declined");
                let code = item["exitCode"].as_i64().unwrap_or(if declined { -1 } else { 0 });
                let result = if declined {
                    "Error: the command was not approved.".to_string()
                } else if name == "run_command" {
                    format!("exit={code}\n{output}")
                } else if code != 0 && !output.starts_with("Error") {
                    format!("Error: {output}")
                } else {
                    output.to_string()
                };
                out.push(Out::ToolResult { id, name, result });
            }
            Some("fileChange") => {
                let ok = item["status"].as_str() == Some("completed");
                for (i, ch) in item["changes"].as_array().into_iter().flatten().enumerate() {
                    let kind = ch["kind"]["type"].as_str().or_else(|| ch["kind"].as_str()).unwrap_or("update");
                    let name = if kind == "add" { "write_file" } else { "edit_file" };
                    let path = self.rel(ch["path"].as_str().unwrap_or(""));
                    let call_id = format!("{id}:{i}");
                    out.push(Out::ToolCall {
                        id: call_id.clone(),
                        name: name.to_string(),
                        args: json!({ "path": path, "kind": kind }),
                    });
                    let verb = match kind {
                        "add" => "Created",
                        "delete" => "Deleted",
                        _ => "Updated",
                    };
                    let result = if ok { format!("{verb} {path}") } else { format!("Error: change to {path} was not applied") };
                    out.push(Out::ToolResult { id: call_id, name: name.to_string(), result });
                }
            }
            Some("mcpToolCall") => {
                let (server, tool) = (item["server"].as_str().unwrap_or(""), item["tool"].as_str().unwrap_or(""));
                let name = self.names.get(&id).cloned().unwrap_or_else(|| format!("mcp__{server}__{tool}"));
                let result = if let Some(err) = item["error"]["message"].as_str() {
                    format!("Error: {err}")
                } else if let Some(content) = item["result"]["content"].as_array() {
                    content.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
                } else {
                    String::new()
                };
                out.push(Out::ToolResult { id, name, result });
            }
            Some("contextCompaction") => out.push(Out::Text("Conversation compacted.".to_string())),
            _ => {}
        }
    }

    /// Maps a server-initiated approval request to what the user is asked.
    pub fn approval(&self, method: &str, p: &Value) -> Option<Approval> {
        let reason = p["reason"].as_str().filter(|r| !r.is_empty()).map(str::to_string);
        match method {
            "item/commandExecution/requestApproval" => {
                Some(Approval::Command { command: display_command(p), reason })
            }
            "item/fileChange/requestApproval" => {
                let item = p["itemId"].as_str().unwrap_or("");
                let path = self
                    .file_paths
                    .get(item)
                    .and_then(|v| v.first().cloned())
                    .or_else(|| p["grantRoot"].as_str().map(|r| self.rel(r)))
                    .unwrap_or_default();
                Some(Approval::FileChange { path, reason })
            }
            "item/permissions/requestApproval" => Some(Approval::Permissions { reason }),
            _ => None,
        }
    }
}

/// The command as the user would type it: Codex wraps it in `pwsh -Command '…'` / `bash -lc '…'`.
fn display_command(v: &Value) -> String {
    if let [only] = v["commandActions"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if let Some(c) = only["command"].as_str().filter(|c| !c.trim().is_empty()) {
            return c.trim().to_string();
        }
    }
    let simple = simplify_command(v["command"].as_str().unwrap_or(""));
    let t = simple.trim();
    for q in ['\'', '"'] {
        if let Some(inner) = t.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner.to_string();
        }
    }
    t.to_string()
}

/// Questions from `item/tool/requestUserInput`.
pub fn parse_questions(p: &Value) -> Vec<Question> {
    p["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            Some(Question {
                id: q["id"].as_str()?.to_string(),
                header: q["header"].as_str().unwrap_or("").to_string(),
                question: q["question"].as_str().unwrap_or("").to_string(),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| {
                        Some(QuestionOption {
                            label: o["label"].as_str()?.to_string(),
                            description: o["description"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect(),
                multi_select: false,
                allow_other: q["isOther"].as_bool().unwrap_or(false),
                secret: q["isSecret"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Response body for `item/tool/requestUserInput`.
pub fn question_response(answers: &HashMap<String, Vec<String>>) -> Value {
    let map: serde_json::Map<String, Value> =
        answers.iter().map(|(id, a)| (id.clone(), json!({ "answers": a }))).collect();
    json!({ "answers": map })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Value> {
        include_str!("fixtures/codex_command_turn.ndjson")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// Replays the captured stream (a command turn run under the `untrusted` approval policy).
    fn replay() -> (Vec<Out>, Vec<Approval>) {
        let mut ev = CodexEvents::new(Path::new(r"C:\proj"));
        let (mut outs, mut approvals) = (Vec::new(), Vec::new());
        for m in fixture() {
            let method = m["method"].as_str().unwrap();
            if m.get("id").is_some() {
                approvals.extend(ev.approval(method, &m["params"]));
            } else {
                outs.extend(ev.feed(method, &m["params"]));
            }
        }
        (outs, approvals)
    }

    #[test]
    fn real_command_turn_becomes_text_tool_and_result() {
        let (outs, _) = replay();
        let text: String = outs
            .iter()
            .filter_map(|o| if let Out::TextDelta(t) = o { Some(t.as_str()) } else { None })
            .collect();
        // Two agent messages in one turn are separated by a paragraph break, with no repeats.
        assert_eq!(text, "Running the requested command.\n\nThe output is `hello-vireo`.");
        assert!(outs.iter().all(|o| !matches!(o, Out::Text(_))), "streamed text must not repeat");
        let call = outs.iter().find_map(|o| if let Out::ToolCall { name, args, .. } = o { Some((name, args)) } else { None });
        assert_eq!(call, Some((&"run_command".to_string(), &json!({ "command": "echo hello-vireo" }))));
        let result = outs.iter().find_map(|o| if let Out::ToolResult { result, .. } = o { Some(result.clone()) } else { None });
        assert_eq!(result.as_deref(), Some("exit=0\nhello-vireo\r\n"));
    }

    #[test]
    fn real_approval_request_shows_the_plain_command() {
        let (_, approvals) = replay();
        assert_eq!(approvals, vec![Approval::Command { command: "echo hello-vireo".into(), reason: None }]);
    }

    #[test]
    fn completed_turn_sets_finished_and_failures_become_errors() {
        let mut ev = CodexEvents::new(Path::new("/w"));
        assert!(ev.feed("turn/completed", &json!({ "turn": { "status": "completed" } })).is_empty());
        assert!(ev.finished);
        ev.begin_turn();
        assert!(!ev.finished);
        let out = ev.feed(
            "turn/completed",
            &json!({ "turn": { "status": "failed", "error": { "message": "model not supported" } } }),
        );
        assert_eq!(out, vec![Out::Error("model not supported".into())]);
    }

    #[test]
    fn file_changes_report_each_path_and_back_approvals() {
        let mut ev = CodexEvents::new(Path::new("/w"));
        let item = json!({ "type": "fileChange", "id": "fc1", "status": "inProgress",
            "changes": [{ "path": "/w/src/a.rs", "kind": { "type": "update", "move_path": null }, "diff": "" }] });
        assert!(ev.feed("item/started", &json!({ "item": item })).is_empty());
        let ask = ev.approval("item/fileChange/requestApproval", &json!({ "itemId": "fc1", "reason": "needs write" }));
        assert_eq!(ask, Some(Approval::FileChange { path: "src/a.rs".into(), reason: Some("needs write".into()) }));
        let mut done = item.clone();
        done["status"] = json!("completed");
        let out = ev.feed("item/completed", &json!({ "item": done }));
        assert_eq!(
            out,
            vec![
                Out::ToolCall { id: "fc1:0".into(), name: "edit_file".into(), args: json!({ "path": "src/a.rs", "kind": "update" }) },
                Out::ToolResult { id: "fc1:0".into(), name: "edit_file".into(), result: "Updated src/a.rs".into() },
            ]
        );
    }

    #[test]
    fn user_input_questions_round_trip() {
        let p = json!({ "questions": [{ "id": "q1", "header": "Env", "question": "Which env?", "isOther": true,
            "options": [{ "label": "prod", "description": "Production" }] }] });
        let qs = parse_questions(&p);
        assert_eq!((qs[0].id.as_str(), qs[0].allow_other, qs[0].options.len()), ("q1", true, 1));
        let mut a = HashMap::new();
        a.insert("q1".to_string(), vec!["prod".to_string()]);
        assert_eq!(question_response(&a), json!({ "answers": { "q1": { "answers": ["prod"] } } }));
    }
}
