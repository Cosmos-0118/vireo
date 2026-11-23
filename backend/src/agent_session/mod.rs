//! Persistent agent sessions for the chat panel.
//!
//! Instead of starting a CLI per message and replaying the conversation as plain text, one
//! long-lived process serves each chat thread, so history, tool state and prompt caches survive
//! between turns. Claude Code is driven through its stream-json control protocol, Codex through
//! `codex app-server`, and GitHub Copilot through the Agent Client Protocol (ACP).

mod acp;
mod acp_events;
mod broker;
mod claude;
mod codex;
mod codex_events;
mod commands;
mod jsonrpc;
mod manager;
mod process;
mod routes;
pub(crate) mod steer;
mod types;

pub use broker::answer_question;
pub use manager::SessionManager;
pub use routes::{close_session, prepare_session};
pub use steer::{cancel_steer, steer_turn, ActiveTurnRegistry};

use crate::agent::{send_sse, AgentChatReq};
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{build_prompt, CliKind, PermMode};
use crate::state::AppState;
use serde_json::json;
use std::path::Path;
use types::{SseTx, TurnEnd, TurnInput};

/// Runs one chat turn for a CLI provider through the thread's persistent session, starting or
/// resuming the agent process when needed.
pub async fn run_turn(state: &AppState, ws: &Path, req: &AgentChatReq, kind: CliKind, auto_cp: bool, tx: &SseTx) {
    // Clients always send a thread id; a request without one gets a throwaway session.
    let thread_id = match req.thread_id.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => format!("oneoff-{}", uuid::Uuid::new_v4()),
    };
    let oneoff = req.thread_id.as_deref().is_none_or(str::is_empty);
    let mode = PermMode::parse(req.permission_mode.as_deref());
    let rules = req.allowed_rules.clone().unwrap_or_default();
    let acquired = state
        .sessions
        .acquire(&thread_id, kind, ws, &req.model, mode, req.resume_id.as_deref(), &rules)
        .await;
    let acquired = match acquired {
        Ok(a) => a,
        Err(message) => return send_sse(tx, "error", json!({ "message": message })).await,
    };

    let session = acquired.session;
    send_sse(tx, "session", json!({ "resume_id": session.resume_id(), "resumed": acquired.resumed })).await;
    let commands = session.commands();
    if !commands.is_empty() {
        send_sse(tx, "commands", json!({ "commands": commands })).await;
    }

    let prompt = if acquired.needs_history {
        build_prompt(&req.messages)
    } else {
        req.messages.last().map(|m| m.content.clone()).unwrap_or_default()
    };
    let input = TurnInput {
        thread_id: thread_id.clone(),
        prompt,
        model: req.model.clone(),
        mode,
        auto_cp,
    };
    let mut turn = TurnTracker::new(auto_cp);
    let end = session.run_turn(state, tx, &mut turn, &input).await;
    match end {
        _ if oneoff => state.sessions.close(&thread_id).await,
        TurnEnd::Broken => state.sessions.close(&thread_id).await,
        TurnEnd::Done | TurnEnd::Stopped => state.sessions.touch(&thread_id).await,
    }
}
