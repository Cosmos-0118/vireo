//! HTTP endpoints for session lifecycle: warm a thread's session and read its slash commands,
//! or end it.

use crate::cli_agent::{CliKind, PermMode};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

type ApiErr = (StatusCode, String);

#[derive(Deserialize)]
pub struct PrepareReq {
    pub thread_id: String,
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub resume_id: Option<String>,
    #[serde(default)]
    pub allowed_rules: Vec<String>,
}

/// `POST /api/agent/session/prepare`: starts (or resumes) the thread's session ahead of the first
/// message and returns the agent's slash commands for the composer's `/` palette.
pub async fn prepare_session(State(state): State<AppState>, Json(req): Json<PrepareReq>) -> Result<Json<Value>, ApiErr> {
    let kind = CliKind::from_provider(&req.provider.to_lowercase())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Not an agent CLI provider.".to_string()))?;
    let ws = state.workspace_path().await;
    let mode = PermMode::parse(req.permission_mode.as_deref());
    let acquired = state
        .sessions
        .acquire(&req.thread_id, kind, &ws, &req.model, mode, req.resume_id.as_deref(), &req.allowed_rules)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({
        "resume_id": acquired.session.resume_id(),
        "resumed": acquired.resumed,
        "commands": acquired.session.commands(),
    })))
}

#[derive(Deserialize)]
pub struct CloseReq {
    pub thread_id: String,
}

/// `POST /api/agent/session/close`: ends the thread's agent process (the thread was deleted).
pub async fn close_session(State(state): State<AppState>, Json(req): Json<CloseReq>) -> Json<Value> {
    state.sessions.close(&req.thread_id).await;
    Json(json!({ "ok": true }))
}
