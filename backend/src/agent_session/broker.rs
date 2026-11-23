//! Routes the agent's questions to the user: tool-approval cards and multiple-choice questions.
//!
//! A driver hands over the request plus a `reply` callback. The broker auto-answers from the
//! thread's remembered rules where it can, and otherwise shows a card in the chat, waits outside
//! the driver's read loop (so Stop and other output keep flowing) and calls `reply` with the answer.

use super::types::SseTx;
use crate::agent::send_sse;
use crate::cli_agent::{permission_rule, permission_summary, rule_allows};
use crate::state::{AppState, PermissionDecision, QuestionAnswer};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// Rules the user chose to "always allow" in this thread. Lives as long as the session.
pub type SharedRules = Arc<Mutex<Vec<String>>>;

pub struct PermissionPrompt {
    pub request_id: String,
    pub tool: String,
    pub input: Value,
}

/// Shows an approval card unless a remembered rule already covers the request. `reply` runs once,
/// with the user's decision — or never, if the turn ends before they answer.
pub async fn ask_permission(
    state: &AppState,
    tx: &SseTx,
    turn_id: &str,
    rules: &SharedRules,
    prompt: PermissionPrompt,
    reply: impl FnOnce(PermissionDecision) + Send + 'static,
) {
    let PermissionPrompt { request_id, tool, input } = prompt;
    let covered = rules.lock().map(|r| r.iter().any(|r| rule_allows(r, &tool, &input))).unwrap_or(false);
    if covered {
        reply(PermissionDecision { allow: true, remember: false });
        return;
    }
    let rule = permission_rule(&tool, &input);
    let (dtx, drx) = oneshot::channel::<PermissionDecision>();
    if let Ok(mut m) = state.pending_permissions.lock() {
        m.insert((turn_id.to_string(), request_id.clone()), dtx);
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
    let rules = rules.clone();
    tokio::spawn(async move {
        // A dropped sender (turn ended / request withdrawn) means nobody answered.
        let Ok(decision) = drx.await else { return };
        if decision.allow && decision.remember {
            if let Ok(mut r) = rules.lock() {
                if !r.contains(&rule) {
                    r.push(rule);
                }
            }
        }
        reply(decision);
    });
}

/// Withdraws an unanswered card (the agent cancelled its request).
pub async fn cancel_permission(state: &AppState, tx: &SseTx, turn_id: &str, request_id: &str) {
    if let Ok(mut m) = state.pending_permissions.lock() {
        m.remove(&(turn_id.to_string(), request_id.to_string()));
    }
    send_sse(tx, "permission_cancel", json!({ "request_id": request_id })).await;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Question {
    /// Key of the answer. Claude keys answers by the question text itself.
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    /// The user may type a free-form answer instead of picking an option.
    pub allow_other: bool,
    pub secret: bool,
}

/// Questions from Claude's `AskUserQuestion` tool input.
pub fn parse_claude_questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let question = q["question"].as_str()?.to_string();
            Some(Question {
                id: question.clone(),
                header: q["header"].as_str().unwrap_or("").to_string(),
                question,
                options: parse_options(&q["options"]),
                multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                allow_other: true,
                secret: false,
            })
        })
        .collect()
}

pub fn parse_options(v: &Value) -> Vec<QuestionOption> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(QuestionOption {
                label: o["label"].as_str()?.to_string(),
                description: o["description"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Shows a question card; `reply` gets the answers keyed by question id, or `None` if dismissed.
pub async fn ask_questions(
    state: &AppState,
    tx: &SseTx,
    turn_id: &str,
    request_id: &str,
    questions: &[Question],
    reply: impl FnOnce(Option<HashMap<String, Vec<String>>>) + Send + 'static,
) {
    let (dtx, drx) = oneshot::channel::<QuestionAnswer>();
    if let Ok(mut m) = state.pending_questions.lock() {
        m.insert((turn_id.to_string(), request_id.to_string()), dtx);
    }
    send_sse(
        tx,
        "user_question",
        json!({ "turn_id": turn_id, "request_id": request_id, "questions": questions }),
    )
    .await;
    tokio::spawn(async move {
        let Ok(answer) = drx.await else { return };
        reply(if answer.cancelled { None } else { Some(answer.answers) });
    });
}

/// Body of `POST /api/agent/question`.
#[derive(Deserialize)]
pub struct QuestionReq {
    pub turn_id: String,
    pub request_id: String,
    #[serde(default)]
    pub answers: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub cancelled: bool,
}

pub async fn answer_question(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(req): axum::Json<QuestionReq>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, String)> {
    let tx = state
        .pending_questions
        .lock()
        .ok()
        .and_then(|mut m| m.remove(&(req.turn_id, req.request_id)));
    match tx {
        Some(tx) => {
            let _ = tx.send(QuestionAnswer { answers: req.answers, cancelled: req.cancelled });
            Ok(axum::Json(json!({ "ok": true })))
        }
        None => Err((axum::http::StatusCode::NOT_FOUND, "No such pending question".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_questions_are_parsed_from_the_tool_input() {
        // Captured from Claude Code's `AskUserQuestion` permission request.
        let input = json!({ "questions": [{
            "question": "Which colour do you prefer?", "header": "Colour", "multiSelect": false,
            "options": [{ "label": "Red", "description": "Choose red" }, { "label": "Blue", "description": "Choose blue" }]
        }]});
        let qs = parse_claude_questions(&input);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].id, "Which colour do you prefer?");
        assert_eq!(qs[0].header, "Colour");
        assert_eq!(qs[0].options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Red", "Blue"]);
        assert!(!qs[0].multi_select);
    }
}
