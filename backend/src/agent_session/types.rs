//! Types shared by the session drivers, the manager and the HTTP routes.

use crate::cli_agent::PermMode;
use axum::response::sse::Event;
use serde::Serialize;
use std::convert::Infallible;
use tokio::sync::mpsc;

/// Sender half of one chat turn's SSE stream.
pub type SseTx = mpsc::Sender<Result<Event, Infallible>>;

/// An entry for the composer's `/` palette.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlashCommand {
    /// Without the leading slash.
    pub name: String,
    pub description: String,
    /// Argument hint such as `<file>` or `[audit | document]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// `builtin`, `skill`, `project` or `user`; only used for grouping and icons.
    pub source: String,
}

/// What a driver needs to run one turn.
#[derive(Debug, Clone)]
pub struct TurnInput {
    pub prompt: String,
    pub model: String,
    pub mode: PermMode,
}

/// How a turn ended, as far as the session's health is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// The turn finished (successfully or with a reported error); the session is reusable.
    Done,
    /// The user stopped the turn; the session was interrupted and is reusable.
    Stopped,
    /// The process died or got stuck; the session must be dropped.
    Broken,
}

/// The agent's `default` model name as sent by the composer.
pub fn wants_model(model: &str) -> Option<&str> {
    let m = model.trim();
    (!m.is_empty() && !m.eq_ignore_ascii_case("default")).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_means_no_override() {
        assert_eq!(wants_model(" Default "), None);
        assert_eq!(wants_model(""), None);
        assert_eq!(wants_model("opus"), Some("opus"));
    }
}
