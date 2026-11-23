//! GitHub Copilot over the Agent Client Protocol (ACP): one long-lived process per chat thread,
//! JSON-RPC over stdio.
//!
//! Vireo advertises no client capabilities, so the agent keeps using its own file and terminal
//! tools; Vireo only renders the stream and answers `session/request_permission` from its cards.

use super::acp_events::{auto_allows, permission_outcome, AcpEvents};
use super::broker::{self, PermissionPrompt, SharedRules};
use super::jsonrpc::{Incoming, RpcClient};
use super::process::{self, failure_text};
use super::types::{wants_model, SlashCommand, SseTx, TurnEnd, TurnInput};
use crate::agent::send_sse;
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{emit, CliKind, Out, PendingGuard, PermMode};
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const CANCEL_GRACE: Duration = Duration::from_secs(8);

/// Flags that put a CLI into ACP mode, for the CLIs known to support it.
pub fn launch_args(kind: CliKind) -> Option<&'static [&'static str]> {
    match kind {
        CliKind::Copilot => Some(&["--acp"]),
        CliKind::Claude | CliKind::Codex => None,
    }
}

pub struct AcpSession {
    kind: CliKind,
    rpc: RpcClient,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<Incoming>>,
    session_id: String,
    events: Mutex<AcpEvents>,
    /// The Vireo environment hint is sent once, with the first prompt.
    hint_pending: AtomicBool,
    alive: Arc<AtomicBool>,
    stderr: Arc<Mutex<String>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    busy: tokio::sync::Mutex<()>,
    rules: SharedRules,
    /// Model last applied through `session/set_model`.
    model: Mutex<String>,
}

fn vx_mcp_servers(ws: &Path) -> Value {
    let Some(dir) = crate::pty::vx_dir() else { return json!([]) };
    let exe = dir.join(if cfg!(windows) { "vx.exe" } else { "vx" }).to_string_lossy().replace('\\', "/");
    let ws = ws.to_string_lossy().replace('\\', "/");
    json!([{ "name": "vx", "command": exe, "args": ["mcp", "--workspace", ws], "env": [] }])
}

async fn call(rpc: &RpcClient, method: &str, params: Value) -> Result<Value, String> {
    match tokio::time::timeout(RPC_TIMEOUT, rpc.request(method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("The agent did not answer `{method}` in time.")),
    }
}

impl AcpSession {
    /// Starts the agent and opens (or, if it supports it, reloads) a session.
    pub async fn start(
        kind: CliKind,
        exe: &Path,
        ws: &Path,
        model: &str,
        resume: Option<&str>,
        rules: Vec<String>,
    ) -> Result<Self, String> {
        let flags = launch_args(kind).ok_or_else(|| format!("{} has no ACP mode.", kind.label()))?;
        let args: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
        let spawned = process::spawn(exe, &args, ws, &[])
            .map_err(|e| format!("Failed to start {} ({}): {e}", kind.label(), exe.display()))?;
        let process::Spawned { mut child, stdin, stdout, pid, stderr } = spawned;
        let (rpc, mut incoming) = RpcClient::spawn(stdout, stdin, true);

        let alive = Arc::new(AtomicBool::new(true));
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let watch_alive = alive.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = kill_rx => {
                    process::kill_tree(pid).await;
                    let _ = child.start_kill();
                }
                _ = child.wait() => {}
            }
            watch_alive.store(false, Ordering::SeqCst);
        });

        let events = Mutex::new(AcpEvents::new(ws));
        let label = kind.label();
        let fail = |what: String| -> String {
            let detail = stderr.lock().map(|s| s.trim().to_string()).unwrap_or_default();
            if detail.is_empty() {
                what
            } else {
                format!("{what} {}", crate::cli_agent::tail_chars(&detail, 600))
            }
        };

        let init = call(
            &rpc,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                "clientInfo": { "name": "vireo", "title": "Vireo", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await;
        let init = match init {
            Ok(i) => i,
            Err(e) => {
                let _ = kill_tx.send(());
                return Err(fail(format!("{label} did not start in ACP mode: {e}")));
            }
        };

        let cwd = ws.to_string_lossy().to_string();
        let can_load = init["agentCapabilities"]["loadSession"].as_bool().unwrap_or(false);
        let opened = match resume.filter(|_| can_load) {
            Some(id) => {
                let r = call(&rpc, "session/load", json!({ "sessionId": id, "cwd": cwd, "mcpServers": vx_mcp_servers(ws) })).await;
                r.map(|_| id.to_string())
            }
            None => call(&rpc, "session/new", json!({ "cwd": cwd, "mcpServers": vx_mcp_servers(ws) }))
                .await
                .and_then(|r| r["sessionId"].as_str().map(str::to_string).ok_or_else(|| "no session id returned".to_string())),
        };
        let session_id = match opened {
            Ok(id) => id,
            Err(e) => {
                let _ = kill_tx.send(());
                return Err(fail(format!("{label} could not open a session: {e}")));
            }
        };
        // Replayed history (`session/load`) and the first command listing arrive as notifications.
        while let Ok(msg) = incoming.try_recv() {
            if let Incoming::Notification { method, params } = msg {
                if method == "session/update" {
                    if let Ok(mut e) = events.lock() {
                        e.feed(&params["update"]);
                    }
                }
            }
        }

        let session = Self {
            kind,
            rpc,
            incoming: tokio::sync::Mutex::new(incoming),
            session_id,
            events,
            hint_pending: AtomicBool::new(resume.is_none() && crate::pty::vx_dir().is_some()),
            alive,
            stderr: stderr.clone(),
            kill: Mutex::new(Some(kill_tx)),
            busy: tokio::sync::Mutex::new(()),
            rules: Arc::new(Mutex::new(rules)),
            model: Mutex::new(String::new()),
        };
        session.apply_model(model).await;
        Ok(session)
    }

    /// Best effort: `session/set_model` is not part of every agent's ACP surface.
    async fn apply_model(&self, model: &str) {
        let Some(m) = wants_model(model) else { return };
        let same = self.model.lock().map(|c| c.as_str() == m).unwrap_or(false);
        if same {
            return;
        }
        let r = call(&self.rpc, "session/set_model", json!({ "sessionId": self.session_id, "modelId": m })).await;
        if r.is_ok() {
            if let Ok(mut c) = self.model.lock() {
                *c = m.to_string();
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn resume_id(&self) -> String {
        self.session_id.clone()
    }

    pub fn commands(&self) -> Vec<SlashCommand> {
        self.events.lock().ok().and_then(|e| e.commands.clone()).unwrap_or_default()
    }

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

    pub async fn run_turn(&self, state: &AppState, tx: &SseTx, turn: &mut TurnTracker, input: &TurnInput) -> TurnEnd {
        let Ok(_busy) = self.busy.try_lock() else {
            send_sse(tx, "error", json!({ "message": "This thread already has a turn running." })).await;
            return TurnEnd::Done;
        };
        let mut incoming = self.incoming.lock().await;
        while let Ok(stale) = incoming.try_recv() {
            if stale == Incoming::Closed {
                return TurnEnd::Broken;
            }
        }
        if !self.is_alive() {
            return TurnEnd::Broken;
        }
        self.apply_model(&input.model).await;

        let turn_id = uuid::Uuid::new_v4().to_string();
        let _pending = PendingGuard { state: state.clone(), turn_id: turn_id.clone() };
        turn.begin(state).await;

        let mut text = input.prompt.clone();
        if self.hint_pending.swap(false, Ordering::SeqCst) {
            text = format!("[Vireo environment]\n{}\n\n{text}", crate::code_intel::AGENT_HINT);
        }
        let rpc = self.rpc.clone();
        let params = json!({ "sessionId": self.session_id, "prompt": [{ "type": "text", "text": text }] });
        let mut prompt = tokio::spawn(async move { rpc.request("session/prompt", params).await });

        let end = self.pump(state, tx, turn, &mut incoming, &turn_id, input.mode, &mut prompt).await;
        turn.finish(state, Some(tx)).await;
        if end == TurnEnd::Broken {
            self.shutdown();
        }
        end
    }

    #[allow(clippy::too_many_arguments)]
    async fn pump(
        &self,
        state: &AppState,
        tx: &SseTx,
        turn: &mut TurnTracker,
        incoming: &mut mpsc::UnboundedReceiver<Incoming>,
        turn_id: &str,
        mode: PermMode,
        prompt: &mut tokio::task::JoinHandle<Result<Value, super::jsonrpc::RpcError>>,
    ) -> TurnEnd {
        loop {
            tokio::select! {
                _ = tx.closed() => return self.cancel(prompt, turn, incoming).await,
                res = &mut *prompt => {
                    // Everything the agent sent before its answer is already queued.
                    while let Ok(msg) = incoming.try_recv() {
                        self.handle(state, tx, turn, turn_id, mode, msg).await;
                    }
                    return match res {
                        Ok(Ok(r)) => self.finish_reason(tx, r["stopReason"].as_str().unwrap_or("end_turn")).await,
                        Ok(Err(e)) => {
                            send_sse(tx, "error", json!({ "message": format!("{} failed: {e}", self.kind.label()) })).await;
                            TurnEnd::Done
                        }
                        Err(_) => TurnEnd::Broken,
                    };
                }
                msg = incoming.recv() => match msg {
                    Some(Incoming::Closed) | None => {
                        let text = failure_text(self.kind.label(), None, &self.stderr);
                        send_sse(tx, "error", json!({ "message": text })).await;
                        return TurnEnd::Broken;
                    }
                    Some(msg) => self.handle(state, tx, turn, turn_id, mode, msg).await,
                },
            }
        }
    }

    async fn finish_reason(&self, tx: &SseTx, reason: &str) -> TurnEnd {
        let note = match reason {
            "max_tokens" => Some("The agent stopped because it reached its output limit."),
            "max_turn_requests" => Some("The agent stopped because it reached its request limit."),
            "refusal" => Some("The agent declined to continue."),
            _ => None,
        };
        if let Some(note) = note {
            send_sse(tx, "error", json!({ "message": note })).await;
        }
        if reason == "cancelled" {
            TurnEnd::Stopped
        } else {
            TurnEnd::Done
        }
    }

    /// Stop was pressed: send `session/cancel` and let the prompt call return.
    async fn cancel(
        &self,
        prompt: &mut tokio::task::JoinHandle<Result<Value, super::jsonrpc::RpcError>>,
        turn: &mut TurnTracker,
        incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    ) -> TurnEnd {
        self.rpc.notify("session/cancel", json!({ "sessionId": self.session_id }));
        let deadline = tokio::time::sleep(CANCEL_GRACE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return TurnEnd::Broken,
                _ = &mut *prompt => return TurnEnd::Stopped,
                msg = incoming.recv() => match msg {
                    Some(Incoming::Notification { method, params }) if method == "session/update" => {
                        let outs = self.events.lock().map(|mut e| e.feed(&params["update"])).unwrap_or_default();
                        for ev in outs {
                            if let Out::ToolCall { name, args, .. } = &ev {
                                turn.note_tool(name, args);
                            }
                        }
                    }
                    // Nobody can answer a card now: report the turn as cancelled.
                    Some(Incoming::Request { id, method, .. }) if method == "session/request_permission" => {
                        self.rpc.respond(&id, json!({ "outcome": { "outcome": "cancelled" } }));
                    }
                    Some(Incoming::Request { id, .. }) => self.rpc.respond_error(&id, -32601, "The turn was stopped."),
                    Some(Incoming::Notification { .. }) => {}
                    _ => return TurnEnd::Broken,
                },
            }
        }
    }

    async fn handle(&self, state: &AppState, tx: &SseTx, turn: &mut TurnTracker, turn_id: &str, mode: PermMode, msg: Incoming) {
        match msg {
            Incoming::Notification { method, params } if method == "session/update" => {
                let outs = self.events.lock().map(|mut e| e.feed(&params["update"])).unwrap_or_default();
                for ev in outs {
                    if let Out::ToolCall { name, args, .. } = &ev {
                        turn.note_tool(name, args);
                    }
                    emit(state, tx, ev).await;
                }
            }
            Incoming::Request { id, method, params } if method == "session/request_permission" => {
                let rpc = self.rpc.clone();
                let options = params["options"].clone();
                if auto_allows(mode, params["toolCall"]["kind"].as_str()) {
                    rpc.respond(&id, json!({ "outcome": permission_outcome(&options, true, false) }));
                    return;
                }
                let (tool, input) = self.events.lock().map(|e| e.permission_card(&params)).unwrap_or_default();
                let request_id = params["toolCall"]["toolCallId"].as_str().map(str::to_string).unwrap_or_else(|| id.to_string());
                let prompt = PermissionPrompt { request_id, tool, input };
                broker::ask_permission(state, tx, turn_id, &self.rules, prompt, move |d| {
                    rpc.respond(&id, json!({ "outcome": permission_outcome(&options, d.allow, d.remember) }));
                })
                .await;
            }
            Incoming::Request { id, .. } => self.rpc.respond_error(&id, -32601, "Vireo does not support this request."),
            _ => {}
        }
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
