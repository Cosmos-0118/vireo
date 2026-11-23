//! Writes normalized [`Out`] events to the chat's SSE stream.

use super::normalize::Out;
use crate::agent::send_sse;
use crate::state::AppState;
use axum::response::sse::Event;
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::mpsc;

/// Longest tool result forwarded to the UI.
const MAX_RESULT_CHARS: usize = 1500;

pub(crate) async fn emit(state: &AppState, tx: &mpsc::Sender<Result<Event, Infallible>>, ev: Out) {
    match ev {
        Out::Text(text) => send_sse(tx, "text", json!({ "text": text })).await,
        Out::TextDelta(text) => send_sse(tx, "text_delta", json!({ "text": text })).await,
        Out::Session(id) => send_sse(tx, "session", json!({ "resume_id": id })).await,
        Out::ToolCall { id, name, args } => {
            send_sse(tx, "tool_call", json!({ "id": id, "name": name, "args": args })).await
        }
        Out::ToolResult { id, name, result } => {
            if matches!(name.as_str(), "write_file" | "edit_file" | "run_command") {
                state.notify_fs_changed(vec!["cli".to_string()]);
            }
            let result: String = result.chars().take(MAX_RESULT_CHARS).collect();
            send_sse(tx, "tool_result", json!({ "id": id, "name": name, "result": result })).await
        }
        Out::Error(message) => send_sse(tx, "error", json!({ "message": message })).await,
        // Handled by the runner, which owns the stdin / pending-request state.
        Out::PermissionRequest { .. }
        | Out::PermissionCancel { .. }
        | Out::UnsupportedControl { .. }
        | Out::Question { .. } => {}
    }
}
