//! Prompt assembly for an agent session that has no memory of the thread yet.

use crate::agent::ChatMessage;

/// Largest slice of earlier conversation replayed into a brand-new session.
const MAX_HISTORY_CHARS: usize = 40_000;

/// The new request, preceded by earlier turns as plain text when there are any.
pub(crate) fn build_prompt(messages: &[ChatMessage]) -> String {
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
        if used > MAX_HISTORY_CHARS && !history.is_empty() {
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
