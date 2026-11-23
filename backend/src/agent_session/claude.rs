//! Claude Code kept alive across turns over its bidirectional stream-json protocol.
//!
//! One `claude` process serves a whole chat thread, so history, tool state and the prompt cache
//! survive between messages. Tool approvals and `AskUserQuestion` arrive as `control_request`s and
//! are answered on stdin; `initialize` returns the slash commands, models and agents.

use super::broker::{self, PermissionPrompt, SharedRules};
use super::commands;
use super::process::{self, failure_text};
use super::steer::{emit_steer_applied, emit_steer_cancelled, ActiveTurnEntry, SteerDelivery, SteerItem, SteerMode};
use super::types::{wants_model, SlashCommand, SseTx, TurnEnd, TurnInput};
use crate::agent::send_sse;
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{
    claude_session_args, control_error, control_request, control_response, emit, steer_user_message_line,
    user_message_line, Normalizer, Out, PendingGuard, PermMode,
};
use crate::state::AppState;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const INIT_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a stopped turn gets to wind down before the process is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(8);

type ControlWaiters = Mutex<HashMap<String, oneshot::Sender<Value>>>;

#[derive(Debug, Clone)]
struct TurnResultMeta {
    queued_turn_count: usize,
    user_message_uuids: Vec<String>,
    has_user_message_uuids: bool,
    is_error: bool,
}

#[derive(Debug, Clone)]
struct InFlightSteer {
    item: SteerItem,
    applied: bool,
    sent_at_epoch: u64,
    saw_tool_result: bool,
}

enum Msg {
    Out(Out),
    /// A top-level assistant message started streaming (`stream_event` -> `message_start`).
    MessageStart,
    /// Claude's `result` event: the turn (or sub-turn) is over.
    Finished(TurnResultMeta),
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
    next_control: Arc<AtomicU64>,
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
            next_control: Arc::new(AtomicU64::new(1)),
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
        let active_turn = state.active_turns.register(&input.thread_id, &turn_id);
        send_sse(tx, "turn", json!({ "turn_id": turn_id })).await;

        let in_flight: Arc<Mutex<VecDeque<InFlightSteer>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel::<(String, oneshot::Sender<bool>)>();
        active_turn.entry().set_cancel_hook(cancel_tx);

        // Background task handling `cancel_async_message` for already-dispatched mid-turn steers.
        let cancel_stdin = self.stdin.clone();
        let cancel_shared = self.shared.clone();
        let cancel_in_flight = in_flight.clone();
        let cancel_seq = self.next_control.clone();
        let cancel_task = tokio::spawn(async move {
            while let Some((steer_id, reply)) = cancel_rx.recv().await {
                let cancellable = cancel_in_flight
                    .lock()
                    .ok()
                    .is_some_and(|q| q.iter().any(|s| s.item.steer_id == steer_id && !s.applied));
                if !cancellable {
                    let _ = reply.send(false);
                    continue;
                }
                let id = format!("vireo-{}", cancel_seq.fetch_add(1, Ordering::Relaxed));
                let (wtx, wrx) = oneshot::channel();
                if let Ok(mut w) = cancel_shared.waiters.lock() {
                    w.insert(id.clone(), wtx);
                }
                if cancel_stdin
                    .send(control_request(&id, "cancel_async_message", json!({ "message_uuid": steer_id })))
                    .is_err()
                {
                    if let Ok(mut w) = cancel_shared.waiters.lock() {
                        w.remove(&id);
                    }
                    let _ = reply.send(false);
                    continue;
                }
                let outcome = tokio::time::timeout(Duration::from_secs(5), wrx).await;
                if let Ok(mut w) = cancel_shared.waiters.lock() {
                    w.remove(&id);
                }
                let cancelled = matches!(
                    outcome,
                    Ok(Ok(resp)) if resp["subtype"].as_str() != Some("error")
                        && resp["response"]["cancelled"].as_bool() == Some(true)
                );
                if cancelled {
                    if let Ok(mut q) = cancel_in_flight.lock() {
                        q.retain(|s| s.item.steer_id != steer_id);
                    }
                }
                let _ = reply.send(cancelled);
            }
        });

        if let Ok(mut n) = self.shared.norm.lock() {
            n.begin_turn();
        }
        self.shared.active.store(true, Ordering::SeqCst);
        turn.begin(state).await;
        let _ = self.stdin.send(user_message_line(&input.prompt));

        let end = self
            .pump(state, tx, turn, &mut events, &turn_id, input, active_turn.entry(), &in_flight)
            .await;
        active_turn.entry().clear_cancel_hook();
        cancel_task.abort();
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

    #[allow(clippy::too_many_arguments)]
    async fn pump(
        &self,
        state: &AppState,
        tx: &SseTx,
        turn: &mut TurnTracker,
        events: &mut mpsc::UnboundedReceiver<Msg>,
        turn_id: &str,
        input: &TurnInput,
        active_turn: &Arc<ActiveTurnEntry>,
        in_flight: &Arc<Mutex<VecDeque<InFlightSteer>>>,
    ) -> TurnEnd {
        let mut msg_epoch: u64 = 0;

        loop {
            // Drain any pending steers into Claude's stdin immediately.
            for item in active_turn.try_pop_all() {
                let priority = match item.mode {
                    SteerMode::Steer => "next",
                    SteerMode::Now => "now",
                };
                let _ = self.stdin.send(steer_user_message_line(&item.text, &item.steer_id, priority));
                let applied = item.mode == SteerMode::Now;
                if let Ok(mut q) = in_flight.lock() {
                    q.push_back(InFlightSteer {
                        item: item.clone(),
                        applied,
                        sent_at_epoch: msg_epoch,
                        saw_tool_result: false,
                    });
                }
                if applied {
                    emit_steer_applied(tx, &item, SteerDelivery::Preempted).await;
                }
            }

            tokio::select! {
                _ = tx.closed() => {
                    let _ = active_turn.force_close_and_drain();
                    return self.interrupt(turn, events).await;
                }
                _ = active_turn.notified() => {
                    // Loop iteration will drain and forward queued steers.
                }
                msg = events.recv() => match msg {
                    Some(Msg::MessageStart) => {
                        msg_epoch += 1;
                        let to_apply: Vec<SteerItem> = in_flight
                            .lock()
                            .map(|mut q| {
                                q.iter_mut()
                                    .filter(|s| !s.applied && ((s.sent_at_epoch >= 1 && msg_epoch > s.sent_at_epoch) || s.saw_tool_result))
                                    .map(|s| {
                                        s.applied = true;
                                        s.item.clone()
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        for item in to_apply {
                            emit_steer_applied(tx, &item, SteerDelivery::MidTurn).await;
                        }
                    }
                    Some(Msg::Out(ev)) => {
                        if matches!(&ev, Out::ToolResult { .. }) {
                            if let Ok(mut q) = in_flight.lock() {
                                for s in q.iter_mut() {
                                    if !s.applied {
                                        s.saw_tool_result = true;
                                    }
                                }
                            }
                        } else if matches!(&ev, Out::Text(_) | Out::ToolCall { .. }) {
                            // Fallback for non-streaming turns without `stream_event` `message_start`.
                            let to_apply: Vec<SteerItem> = in_flight
                                .lock()
                                .map(|mut q| {
                                    q.iter_mut()
                                        .filter(|s| !s.applied && s.saw_tool_result)
                                        .map(|s| {
                                            s.applied = true;
                                            s.item.clone()
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            for item in to_apply {
                                emit_steer_applied(tx, &item, SteerDelivery::MidTurn).await;
                            }
                        }
                        self.handle(state, tx, turn, turn_id, input.mode, ev).await;
                    }
                    Some(Msg::Finished(meta)) => {
                        if meta.is_error {
                            self.cancel_remaining_steers(tx, active_turn, in_flight, "turn_failed").await;
                            return TurnEnd::Done;
                        }

                        // Reconcile steers consumed during the finished sub-turn.
                        let consumed_unapplied: Vec<SteerItem> = in_flight
                            .lock()
                            .map(|mut q| {
                                let mut out = Vec::new();
                                if meta.has_user_message_uuids {
                                    q.retain(|s| {
                                        if meta.user_message_uuids.iter().any(|u| u == &s.item.steer_id) {
                                            if !s.applied {
                                                out.push(s.item.clone());
                                            }
                                            false
                                        } else {
                                            true
                                        }
                                    });
                                } else if meta.queued_turn_count == 0 {
                                    for s in q.drain(..) {
                                        if !s.applied {
                                            out.push(s.item);
                                        }
                                    }
                                }
                                out
                            })
                            .unwrap_or_default();
                        for item in consumed_unapplied {
                            emit_steer_applied(tx, &item, SteerDelivery::MidTurn).await;
                        }

                        let has_in_flight = in_flight.lock().map(|q| !q.is_empty()).unwrap_or(false);
                        if meta.queued_turn_count > 0 || has_in_flight {
                            // Claude is automatically starting a chained turn from its internal queue.
                            turn.finish(state, Some(tx)).await;
                            *turn = TurnTracker::new(input.auto_cp);
                            turn.begin(state).await;
                            let chained: Vec<SteerItem> = in_flight
                                .lock()
                                .map(|mut q| {
                                    q.iter_mut()
                                        .filter(|s| !s.applied)
                                        .map(|s| {
                                            s.applied = true;
                                            s.item.clone()
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            for item in chained {
                                emit_steer_applied(tx, &item, SteerDelivery::Continuation).await;
                            }
                            msg_epoch = 0;
                            continue;
                        }

                        // Atomic turn-end handshake: check if a steer arrived right before closing.
                        if let Some(item) = active_turn.try_close_if_empty() {
                            turn.finish(state, Some(tx)).await;
                            *turn = TurnTracker::new(input.auto_cp);
                            turn.begin(state).await;
                            let priority = match item.mode {
                                SteerMode::Steer => "next",
                                SteerMode::Now => "now",
                            };
                            let _ = self.stdin.send(steer_user_message_line(&item.text, &item.steer_id, priority));
                            if let Ok(mut q) = in_flight.lock() {
                                q.push_back(InFlightSteer {
                                    item: item.clone(),
                                    applied: true,
                                    sent_at_epoch: 0,
                                    saw_tool_result: false,
                                });
                            }
                            let delivery = if item.mode == SteerMode::Now {
                                SteerDelivery::Preempted
                            } else {
                                SteerDelivery::Continuation
                            };
                            emit_steer_applied(tx, &item, delivery).await;
                            msg_epoch = 0;
                            continue;
                        }
                        return TurnEnd::Done;
                    }
                    Some(Msg::Closed(code)) => {
                        self.cancel_remaining_steers(tx, active_turn, in_flight, "turn_failed").await;
                        if !self.finished() {
                            let text = failure_text("Claude Code", code, &self.shared.stderr);
                            send_sse(tx, "error", json!({ "message": text })).await;
                        }
                        return TurnEnd::Broken;
                    }
                    None => {
                        self.cancel_remaining_steers(tx, active_turn, in_flight, "turn_failed").await;
                        return TurnEnd::Broken;
                    }
                },
            }
        }
    }

    async fn cancel_remaining_steers(
        &self,
        tx: &SseTx,
        active_turn: &Arc<ActiveTurnEntry>,
        in_flight: &Arc<Mutex<VecDeque<InFlightSteer>>>,
        reason: &str,
    ) {
        let mut ids: Vec<String> = in_flight
            .lock()
            .map(|mut q| q.drain(..).filter(|s| !s.applied).map(|s| s.item.steer_id).collect())
            .unwrap_or_default();
        for item in active_turn.force_close_and_drain() {
            ids.push(item.steer_id);
        }
        for id in ids {
            emit_steer_cancelled(tx, &id, reason).await;
        }
    }

    /// Stop was pressed: ask Claude to stop (and discard queued steers) so the session stays usable.
    async fn interrupt(&self, turn: &mut TurnTracker, events: &mut mpsc::UnboundedReceiver<Msg>) -> TurnEnd {
        let _ = self.stdin.send(control_request("vireo-interrupt", "interrupt", json!({ "cancel_queued": true })));
        let deadline = tokio::time::sleep(INTERRUPT_GRACE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return TurnEnd::Broken,
                msg = events.recv() => match msg {
                    Some(Msg::Out(Out::ToolCall { name, args, .. })) => turn.note_tool(&name, &args),
                    Some(Msg::Out(_)) | Some(Msg::MessageStart) => {}
                    Some(Msg::Finished(_)) => return TurnEnd::Stopped,
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
    let is_message_start = v["type"].as_str() == Some("stream_event")
        && v["parent_tool_use_id"].is_null()
        && v["event"]["type"].as_str() == Some("message_start");
    if is_message_start && active {
        let _ = evt_tx.send(Msg::MessageStart);
    }
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
        let meta = TurnResultMeta {
            queued_turn_count: v["queued_turn_count"].as_u64().unwrap_or(0) as usize,
            has_user_message_uuids: v.get("user_message_uuids").and_then(Value::as_array).is_some(),
            user_message_uuids: v["user_message_uuids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            is_error: v["is_error"].as_bool() == Some(true),
        };
        let _ = evt_tx.send(Msg::Finished(meta));
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
