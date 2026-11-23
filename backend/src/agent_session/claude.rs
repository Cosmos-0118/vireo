//! Claude Code kept alive across turns over its bidirectional stream-json protocol.
//!
//! One `claude` process serves a whole chat thread, so history, tool state and the prompt cache
//! survive between messages. Tool approvals and `AskUserQuestion` arrive as `control_request`s and
//! are answered on stdin; `initialize` returns the slash commands, models and agents.

use super::broker::{self, PermissionPrompt, SharedRules};
use super::commands;
use super::process::{self, failure_text};
use super::types::{wants_model, SlashCommand, SseTx, TurnEnd, TurnInput};
use crate::agent::send_sse;
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{
    claude_session_args, control_error, control_request, control_response, emit, user_message_line, Normalizer, Out,
    PendingGuard, PermMode,
};
use crate::state::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const INIT_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a stopped turn gets to wind down before the process is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(8);

type ControlWaiters = Mutex<HashMap<String, oneshot::Sender<Value>>>;

enum Msg {
    Out(Out),
    /// Claude's `result` event: the turn is over.
    Finished,
    Closed(Option<i32>),
}

/// State shared between the reader task and the turn loop.
struct Shared {
    norm: Mutex<Normalizer>,
    waiters: ControlWaiters,
    /// A turn is consuming events; otherwise stray requests are refused by the reader.
    active: AtomicBool,
    alive: AtomicBool,
    settings: Mutex<Settings>,
    commands: Mutex<Vec<SlashCommand>>,
    stderr: Arc<Mutex<String>>,
}

#[derive(Clone)]
struct Settings {
    mode: PermMode,
    /// Empty for the CLI's default model.
    model: String,
}

pub struct ClaudeSession {
    session_id: String,
    stdin: mpsc::UnboundedSender<String>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<Msg>>,
    shared: Arc<Shared>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    busy: tokio::sync::Mutex<()>,
    rules: SharedRules,
    next_control: std::sync::atomic::AtomicU64,
}

impl ClaudeSession {
    /// Starts `claude` and waits for its `initialize` handshake. With `resume`, continues that
    /// conversation; the caller falls back to a fresh start if that fails.
    pub async fn start(
        exe: &Path,
        ws: &Path,
        model: &str,
        mode: PermMode,
        resume: Option<&str>,
        rules: Vec<String>,
    ) -> Result<Self, String> {
        let session_id = resume.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let args = claude_session_args(ws, model, mode, &session_id, resume.is_some());
        // Allow launching from inside another Claude Code session.
        let spawned = process::spawn(exe, &args, ws, &["CLAUDECODE"])
            .map_err(|e| format!("Failed to start Claude Code ({}): {e}", exe.display()))?;
        let process::Spawned { mut child, mut stdin, stdout, pid, stderr } = spawned;

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(mut line) = stdin_rx.recv().await {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let shared = Arc::new(Shared {
            norm: Mutex::new(Normalizer::new(ws)),
            waiters: Mutex::new(HashMap::new()),
            active: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            settings: Mutex::new(Settings { mode, model: model.trim().to_string() }),
            commands: Mutex::new(Vec::new()),
            stderr: stderr.clone(),
        });
        let (evt_tx, evt_rx) = mpsc::unbounded_channel::<Msg>();
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();

        let reader_shared = shared.clone();
        let reader_stdin = stdin_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                tokio::select! {
                    // Fires on an explicit shutdown and when the session is dropped.
                    _ = &mut kill_rx => {
                        process::kill_tree(pid).await;
                        let _ = child.start_kill();
                        break;
                    }
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
                        route_line(&reader_shared, &reader_stdin, &evt_tx, &v);
                    }
                }
            }
            reader_shared.alive.store(false, Ordering::SeqCst);
            let code = child.wait().await.ok().and_then(|s| s.code());
            let _ = evt_tx.send(Msg::Closed(code));
        });

        let session = Self {
            session_id,
            stdin: stdin_tx,
            events: tokio::sync::Mutex::new(evt_rx),
            shared,
            kill: Mutex::new(Some(kill_tx)),
            busy: tokio::sync::Mutex::new(()),
            rules: Arc::new(Mutex::new(rules)),
            next_control: std::sync::atomic::AtomicU64::new(1),
        };
        match session.control("initialize", json!({}), INIT_TIMEOUT).await {
            Ok(init) => {
                if let Ok(mut c) = session.shared.commands.lock() {
                    *c = commands::from_claude_init(&init);
                }
                Ok(session)
            }
            Err(e) => {
                let detail = stderr.lock().map(|s| s.trim().to_string()).unwrap_or_default();
                session.shutdown();
                Err(if detail.is_empty() { e } else { format!("{e} {}", crate::cli_agent::tail_chars(&detail, 600)) })
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    pub fn resume_id(&self) -> String {
        self.session_id.clone()
    }

    pub fn commands(&self) -> Vec<SlashCommand> {
        self.shared.commands.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// The thread's remembered "always allow" rules, merged with those the client re-sent.
    pub fn merge_rules(&self, client_rules: &[String]) {
        if let Ok(mut r) = self.rules.lock() {
            for c in client_rules {
                if !r.contains(c) {
                    r.push(c.clone());
                }
            }
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut k) = self.kill.lock() {
            if let Some(tx) = k.take() {
                let _ = tx.send(());
            }
        }
    }

    /// Sends a control request and waits for its response body.
    async fn control(&self, subtype: &str, extra: Value, timeout: Duration) -> Result<Value, String> {
        let id = format!("vireo-{}", self.next_control.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        if let Ok(mut w) = self.shared.waiters.lock() {
            w.insert(id.clone(), tx);
        }
        if self.stdin.send(control_request(&id, subtype, extra)).is_err() {
            return Err("Claude Code is not running.".to_string());
        }
        let outcome = tokio::time::timeout(timeout, rx).await;
        if let Ok(mut w) = self.shared.waiters.lock() {
            w.remove(&id);
        }
        match outcome {
            Ok(Ok(resp)) if resp["subtype"].as_str() == Some("error") => {
                Err(resp["error"].as_str().unwrap_or("Claude Code rejected the request.").to_string())
            }
            Ok(Ok(resp)) => Ok(resp["response"].clone()),
            Ok(Err(_)) => Err("Claude Code exited before answering.".to_string()),
            Err(_) => Err(format!("Claude Code did not answer `{subtype}` in time.")),
        }
    }

    /// Applies the composer's permission mode / model to the live session.
    async fn sync_settings(&self, input: &TurnInput) {
        let want_model = wants_model(&input.model).unwrap_or("default").to_string();
        let current = self.shared.settings.lock().map(|s| s.clone()).ok();
        let Some(current) = current else { return };
        if current.mode != input.mode {
            let mode = match input.mode {
                PermMode::Ask => "default",
                PermMode::AutoEdit => "acceptEdits",
                PermMode::FullAccess => "bypassPermissions",
            };
            if self.control("set_permission_mode", json!({ "mode": mode }), CONTROL_TIMEOUT).await.is_ok() {
                if let Ok(mut s) = self.shared.settings.lock() {
                    s.mode = input.mode;
                }
            }
        }
        let have = if current.model.is_empty() { "default".to_string() } else { current.model.clone() };
        if have != want_model && self.control("set_model", json!({ "model": want_model }), CONTROL_TIMEOUT).await.is_ok() {
            if let Ok(mut s) = self.shared.settings.lock() {
                s.model = if want_model == "default" { String::new() } else { want_model };
            }
        }
    }

    pub async fn run_turn(&self, state: &AppState, tx: &SseTx, turn: &mut TurnTracker, input: &TurnInput) -> TurnEnd {
        let Ok(_busy) = self.busy.try_lock() else {
            send_sse(tx, "error", json!({ "message": "This thread already has a turn running." })).await;
            return TurnEnd::Done;
        };
        let mut events = self.events.lock().await;
        while events.try_recv().is_ok() {}
        if !self.is_alive() {
            return TurnEnd::Broken;
        }

        self.sync_settings(input).await;
        let turn_id = uuid::Uuid::new_v4().to_string();
        let _pending = PendingGuard { state: state.clone(), turn_id: turn_id.clone() };
        if let Ok(mut n) = self.shared.norm.lock() {
            n.begin_turn();
        }
        self.shared.active.store(true, Ordering::SeqCst);
        turn.begin(state).await;
        let _ = self.stdin.send(user_message_line(&input.prompt));

        let end = self.pump(state, tx, turn, &mut events, &turn_id, input.mode).await;
        self.shared.active.store(false, Ordering::SeqCst);
        turn.finish(state, Some(tx)).await;
        if end == TurnEnd::Broken {
            self.shutdown();
        }
        end
    }

    fn finished(&self) -> bool {
        self.shared.norm.lock().map(|n| n.finished).unwrap_or(true)
    }

    async fn pump(
        &self,
        state: &AppState,
        tx: &SseTx,
        turn: &mut TurnTracker,
        events: &mut mpsc::UnboundedReceiver<Msg>,
        turn_id: &str,
        mode: PermMode,
    ) -> TurnEnd {
        loop {
            tokio::select! {
                _ = tx.closed() => return self.interrupt(turn, events).await,
                msg = events.recv() => match msg {
                    Some(Msg::Out(ev)) => self.handle(state, tx, turn, turn_id, mode, ev).await,
                    Some(Msg::Finished) => return TurnEnd::Done,
                    Some(Msg::Closed(code)) => {
                        if !self.finished() {
                            let text = failure_text("Claude Code", code, &self.shared.stderr);
                            send_sse(tx, "error", json!({ "message": text })).await;
                        }
                        return TurnEnd::Broken;
                    }
                    None => return TurnEnd::Broken,
                },
            }
        }
    }

    /// Stop was pressed: ask Claude to stop and let it finish the turn so the session stays usable.
    async fn interrupt(&self, turn: &mut TurnTracker, events: &mut mpsc::UnboundedReceiver<Msg>) -> TurnEnd {
        let _ = self.stdin.send(control_request("vireo-interrupt", "interrupt", json!({})));
        let deadline = tokio::time::sleep(INTERRUPT_GRACE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return TurnEnd::Broken,
                msg = events.recv() => match msg {
                    Some(Msg::Out(Out::ToolCall { name, args, .. })) => turn.note_tool(&name, &args),
                    Some(Msg::Out(_)) => {}
                    Some(Msg::Finished) => return TurnEnd::Stopped,
                    _ => return TurnEnd::Broken,
                },
            }
        }
    }

    async fn handle(&self, state: &AppState, tx: &SseTx, turn: &mut TurnTracker, turn_id: &str, mode: PermMode, ev: Out) {
        match ev {
            Out::PermissionRequest { request_id, tool, input } => {
                // `vx` edits are pre-approved outside Ask mode; the launch-time allow list only
                // holds its read-only tools because the mode can change mid-session.
                if mode != PermMode::Ask && tool.starts_with("mcp__vx__") {
                    let _ = self.stdin.send(control_response(&request_id, true, &input));
                    return;
                }
                let stdin = self.stdin.clone();
                let (rid, original) = (request_id.clone(), input.clone());
                let prompt = PermissionPrompt { request_id, tool, input };
                broker::ask_permission(state, tx, turn_id, &self.rules, prompt, move |d| {
                    let _ = stdin.send(control_response(&rid, d.allow, &original));
                })
                .await;
            }
            Out::PermissionCancel { request_id } => broker::cancel_permission(state, tx, turn_id, &request_id).await,
            Out::Question { request_id, input } => {
                let questions = broker::parse_claude_questions(&input);
                let stdin = self.stdin.clone();
                let rid = request_id.clone();
                broker::ask_questions(state, tx, turn_id, &request_id, &questions, move |answers| {
                    let line = match answers {
                        Some(a) => question_answer_line(&rid, &input, &a),
                        None => dismissed_question_line(&rid),
                    };
                    let _ = stdin.send(line);
                })
                .await;
            }
            Out::UnsupportedControl { request_id } => {
                let _ = self.stdin.send(control_error(&request_id));
            }
            ev => {
                if let Out::ToolCall { name, args, .. } = &ev {
                    turn.note_tool(name, args);
                }
                emit(state, tx, ev).await;
            }
        }
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Reader-side routing of one stdout line.
fn route_line(shared: &Shared, stdin: &mpsc::UnboundedSender<String>, evt_tx: &mpsc::UnboundedSender<Msg>, v: &Value) {
    if v["type"].as_str() == Some("control_response") {
        let id = v["response"]["request_id"].as_str().unwrap_or("");
        let waiter = shared.waiters.lock().ok().and_then(|mut w| w.remove(id));
        if let Some(w) = waiter {
            let _ = w.send(v["response"].clone());
        }
        return;
    }
    let outs = shared.norm.lock().map(|mut n| n.feed(v)).unwrap_or_default();
    let active = shared.active.load(Ordering::SeqCst);
    let finished = v["type"].as_str() == Some("result");
    for out in outs {
        if active {
            // The session id is already known (Vireo chose it) and is reported per turn.
            if !matches!(out, Out::Session(_)) {
                let _ = evt_tx.send(Msg::Out(out));
            }
            continue;
        }
        // No turn is listening: refuse requests so Claude never waits on a card nobody can see.
        match out {
            Out::PermissionRequest { request_id, input, .. } => {
                let _ = stdin.send(control_response(&request_id, false, &input));
            }
            Out::Question { request_id, .. } => {
                let _ = stdin.send(dismissed_question_line(&request_id));
            }
            Out::UnsupportedControl { request_id } => {
                let _ = stdin.send(control_error(&request_id));
            }
            _ => {}
        }
    }
    if finished && active {
        let _ = evt_tx.send(Msg::Finished);
    }
}

/// `allow` response carrying the user's answers, keyed by question text (multi-select answers are
/// joined with ", ").
fn question_answer_line(request_id: &str, input: &Value, answers: &HashMap<String, Vec<String>>) -> String {
    let joined: serde_json::Map<String, Value> =
        answers.iter().map(|(q, a)| (q.clone(), Value::String(a.join(", ")))).collect();
    let mut updated = input.clone();
    updated["answers"] = Value::Object(joined);
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id,
            "response": { "behavior": "allow", "updatedInput": updated } }
    })
    .to_string()
}

fn dismissed_question_line(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id,
            "response": { "behavior": "deny", "message": "The user dismissed the question without answering." } }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_keyed_by_question_text_and_joined() {
        let input = json!({ "questions": [{ "question": "Pick?", "options": [] }] });
        let mut answers = HashMap::new();
        answers.insert("Pick?".to_string(), vec!["Red".to_string(), "Blue".to_string()]);
        let line: Value = serde_json::from_str(&question_answer_line("r1", &input, &answers)).unwrap();
        assert_eq!(line["response"]["request_id"], "r1");
        assert_eq!(line["response"]["response"]["behavior"], "allow");
        assert_eq!(line["response"]["response"]["updatedInput"]["answers"]["Pick?"], "Red, Blue");
        assert_eq!(line["response"]["response"]["updatedInput"]["questions"][0]["question"], "Pick?");
    }

    #[test]
    fn dismissing_denies_the_tool_call() {
        let line: Value = serde_json::from_str(&dismissed_question_line("r2")).unwrap();
        assert_eq!(line["response"]["response"]["behavior"], "deny");
    }
}
