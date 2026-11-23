//! Tool-permission prompts: the HTTP answer endpoint, pending-request cleanup, and the narrow
//! "always allow in this thread" rules enforced by Vireo itself.

use super::tools::tail_chars;
use crate::state::{AppState, PermissionDecision};
use serde_json::{json, Value};

/// Drops this run's unanswered permission prompts however `run` exits.
pub(crate) struct PendingGuard {
    pub(crate) state: AppState,
    pub(crate) turn_id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.state.pending_permissions.lock() {
            m.retain(|(t, _), _| t != &self.turn_id);
        }
        if let Ok(mut m) = self.state.pending_questions.lock() {
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

/// Path-like argument of a tool request, if any.
pub(crate) fn input_path(input: &Value) -> Option<&str> {
    ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .filter(|s| !s.is_empty())
}

pub(crate) fn norm_path(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

pub(crate) fn is_shell_tool(tool: &str) -> bool {
    matches!(tool, "Bash" | "PowerShell")
}

pub(crate) fn has_shell_meta(cmd: &str) -> bool {
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

pub(crate) fn permission_summary(tool: &str, input: &Value) -> String {
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
