//! Codex kept alive across turns through `codex app-server` (JSON-RPC 2.0 over stdio).
//!
//! One server process hosts a thread per chat thread: history and tool state persist, replies
//! stream token by token, and command / file-change approvals arrive as server requests that are
//! answered from the chat's permission cards.

use super::broker::{self, PermissionPrompt, SharedRules};
use super::codex_events::{self, Approval, CodexEvents};
use super::commands;
use super::jsonrpc::{Incoming, RpcClient, RpcError};
use super::process::{self, failure_text};
use super::types::{wants_model, SlashCommand, SseTx, TurnEnd, TurnInput};
use crate::agent::send_sse;
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{codex_app_server_args, emit, Out, PendingGuard, PermMode};
use crate::state::{AppState, PermissionDecision};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const INTERRUPT_GRACE: Duration = Duration::from_secs(8);

#[derive(Clone)]
struct Settings {
    mode: PermMode,
    model: String,
}

pub struct CodexSession {
    rpc: RpcClient,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<Incoming>>,
    thread_id: String,
    ws: PathBuf,
    events: Mutex<CodexEvents>,
    commands: Mutex<Vec<SlashCommand>>,
    skill_paths: Mutex<HashMap<String, String>>,
    settings: Mutex<Settings>,
    /// The vx MCP server is read-only in Ask mode, which is fixed at launch.
    launched_read_only: bool,
    alive: Arc<AtomicBool>,
    stderr: Arc<Mutex<String>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    busy: tokio::sync::Mutex<()>,
    rules: SharedRules,
}

/// `(approvalPolicy, sandbox mode, turn sandboxPolicy)` for a Vireo permission mode.
fn policy(mode: PermMode) -> (&'static str, &'static str, Value) {
    match mode {
        PermMode::Ask => ("untrusted", "workspace-write", json!({ "type": "workspaceWrite" })),
        PermMode::AutoEdit => ("on-request", "workspace-write", json!({ "type": "workspaceWrite" })),
        PermMode::FullAccess => ("never", "danger-full-access", json!({ "type": "dangerFullAccess" })),
    }
}

fn shell_label() -> &'static str {
    if cfg!(windows) {
        "PowerShell"
    } else {
        "Bash"
    }
}

async fn call(rpc: &RpcClient, method: &str, params: Value) -> Result<Value, String> {
    match tokio::time::timeout(RPC_TIMEOUT, rpc.request(method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("Codex did not answer `{method}` in time.")),
    }
}

impl CodexSession {
    /// Starts `codex app-server` and a thread (or resumes `resume`, falling back to the caller on error).
    pub async fn start(
        exe: &Path,
        ws: &Path,
        model: &str,
        mode: PermMode,
        resume: Option<&str>,
        rules: Vec<String>,
    ) -> Result<Self, String> {
        let args = codex_app_server_args(ws, mode);
        let spawned = process::spawn(exe, &args, ws, &[])
            .map_err(|e| format!("Failed to start Codex ({}): {e}", exe.display()))?;
        let process::Spawned { mut child, stdin, stdout, pid, stderr } = spawned;
        let (rpc, incoming) = RpcClient::spawn(stdout, stdin, false);

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

        let session_stderr = stderr.clone();
        let fail = |what: String| -> String {
            let detail = session_stderr.lock().map(|s| s.trim().to_string()).unwrap_or_default();
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
                "clientInfo": { "name": "vireo", "title": "Vireo", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true },
            }),
        )
        .await;
        if let Err(e) = init {
            let _ = kill_tx.send(());
            return Err(fail(format!("Codex failed to initialize: {e}")));
        }
        rpc.notify("initialized", Value::Null);

        let (approval, sandbox, _) = policy(mode);
        let mut params = json!({
            "cwd": ws.to_string_lossy(),
            "approvalPolicy": approval,
            "sandbox": sandbox,
        });
        if let Some(m) = wants_model(model) {
            params["model"] = json!(m);
        }
        let thread = match resume {
            Some(id) => {
                params["threadId"] = json!(id);
                params["excludeTurns"] = json!(true);
                call(&rpc, "thread/resume", params).await
            }
            None => call(&rpc, "thread/start", params).await,
        };
        let thread_id = match thread.as_ref().ok().and_then(|t| t["thread"]["id"].as_str()) {
            Some(id) => id.to_string(),
            None => {
                let _ = kill_tx.send(());
                let why = thread.err().unwrap_or_else(|| "no thread id returned".to_string());
                return Err(fail(format!("Codex could not open a thread: {why}")));
            }
        };

        let session = Self {
            rpc,
            incoming: tokio::sync::Mutex::new(incoming),
            thread_id,
            ws: ws.to_path_buf(),
            events: Mutex::new(CodexEvents::new(ws)),
            commands: Mutex::new(commands::codex_builtin()),
            skill_paths: Mutex::new(HashMap::new()),
            settings: Mutex::new(Settings { mode, model: model.trim().to_string() }),
            launched_read_only: mode == PermMode::Ask,
            alive,
            stderr,
            kill: Mutex::new(Some(kill_tx)),
            busy: tokio::sync::Mutex::new(()),
            rules: Arc::new(Mutex::new(rules)),
        };
        session.load_skills().await;
        Ok(session)
    }

    async fn load_skills(&self) {
        let res = call(&self.rpc, "skills/list", json!({ "cwds": [self.ws.to_string_lossy()] })).await;
        if let Ok(res) = res {
            if let Ok(mut c) = self.commands.lock() {
                *c = commands::from_codex_skills(&res);
            }
            if let Ok(mut p) = self.skill_paths.lock() {
                *p = commands::codex_skill_paths(&res);
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn resume_id(&self) -> String {
        self.thread_id.clone()
    }

    pub fn commands(&self) -> Vec<SlashCommand> {
        self.commands.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// True when `mode` needs a different launch config (the vx MCP server's read-only flag).
    pub fn needs_relaunch(&self, mode: PermMode) -> bool {
        self.launched_read_only != (mode == PermMode::Ask)
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

    /// Turn parameters shared by `turn/start`; mode / model overrides only when they changed.
    fn turn_overrides(&self, input: &TurnInput) -> Value {
        let mut p = json!({});
        let Ok(mut s) = self.settings.lock() else { return p };
        if s.mode != input.mode {
            let (approval, _, sandbox) = policy(input.mode);
            p["approvalPolicy"] = json!(approval);
            p["sandboxPolicy"] = sandbox;
            s.mode = input.mode;
        }
        let want = wants_model(&input.model).unwrap_or("");
        if s.model != want && !want.is_empty() {
            p["model"] = json!(want);
            s.model = want.to_string();
        }
        p
    }

    /// The JSON-RPC request that starts this turn: a plain message, a skill, `/compact` or `/review`.
    fn start_request(&self, input: &TurnInput) -> (&'static str, Value) {
        let known = self.commands();
        let mut params = json!({ "threadId": self.thread_id });
        if let Some((name, args)) = commands::parse_invocation(&input.prompt, &known) {
            match name {
                "compact" => return ("thread/compact/start", params),
                "review" => {
                    params["target"] = if args.is_empty() {
                        json!({ "type": "uncommittedChanges" })
                    } else {
                        json!({ "type": "custom", "instructions": args })
                    };
                    params["delivery"] = json!("inline");
                    return ("review/start", params);
                }
                _ => {
                    let path = self.skill_paths.lock().ok().and_then(|p| p.get(name).cloned());
                    if let Some(path) = path {
                        let text = if args.is_empty() { format!("${name}") } else { format!("${name} {args}") };
                        params["input"] = json!([
                            { "type": "skill", "name": name, "path": path },
                            { "type": "text", "text": text },
                        ]);
                        params.as_object_mut().map(|o| o.extend(self.turn_overrides(input).as_object().cloned().unwrap_or_default()));
                        return ("turn/start", params);
                    }
                }
            }
        }
        params["input"] = json!([{ "type": "text", "text": input.prompt }]);
        if let (Some(o), Some(extra)) = (params.as_object_mut(), self.turn_overrides(input).as_object().cloned()) {
            o.extend(extra);
        }
        ("turn/start", params)
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

        let turn_id = uuid::Uuid::new_v4().to_string();
        let _pending = PendingGuard { state: state.clone(), turn_id: turn_id.clone() };
        if let Ok(mut e) = self.events.lock() {
            e.begin_turn();
        }
        turn.begin(state).await;

        let (method, params) = self.start_request(input);
        let rpc = self.rpc.clone();
        let mut start = tokio::spawn(async move { rpc.request(method, params).await });
        let end = self.pump(state, tx, turn, &mut incoming, &turn_id, &mut start).await;
        turn.finish(state, Some(tx)).await;
        if end == TurnEnd::Broken {
            self.shutdown();
        }
        end
    }

    fn finished(&self) -> bool {
        self.events.lock().map(|e| e.finished).unwrap_or(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn pump(
        &self,
        state: &AppState,
        tx: &SseTx,
        turn: &mut TurnTracker,
        incoming: &mut mpsc::UnboundedReceiver<Incoming>,
        turn_id: &str,
        start: &mut tokio::task::JoinHandle<Result<Value, RpcError>>,
    ) -> TurnEnd {
        let mut codex_turn: Option<String> = None;
        let mut start_done = false;
        loop {
            tokio::select! {
                _ = tx.closed() => {
                    if codex_turn.is_none() && !start_done {
                        if let Ok(Ok(Ok(r))) = tokio::time::timeout(Duration::from_secs(5), &mut *start).await {
                            codex_turn = r["turn"]["id"].as_str().map(str::to_string);
                        }
                    }
                    return self.interrupt(codex_turn, turn, incoming).await;
                }
                res = &mut *start, if !start_done => {
                    start_done = true;
                    match res {
                        Ok(Ok(r)) => codex_turn = r["turn"]["id"].as_str().map(str::to_string),
                        Ok(Err(e)) => {
                            send_sse(tx, "error", json!({ "message": format!("Codex rejected the request: {e}") })).await;
                            return TurnEnd::Done;
                        }
                        Err(_) => return TurnEnd::Broken,
                    }
                }
                msg = incoming.recv() => match msg {
                    Some(Incoming::Notification { method, params }) => {
                        if method == "turn/started" && codex_turn.is_none() {
                            codex_turn = params["turn"]["id"].as_str().map(str::to_string);
                        }
                        let outs = self.events.lock().map(|mut e| e.feed(&method, &params)).unwrap_or_default();
                        for ev in outs {
                            if let Out::ToolCall { name, args, .. } = &ev {
                                turn.note_tool(name, args);
                            }
                            emit(state, tx, ev).await;
                        }
                        if self.finished() {
                            return TurnEnd::Done;
                        }
                    }
                    Some(Incoming::Request { id, method, params }) => {
                        self.on_request(state, tx, turn_id, id, &method, &params).await;
                    }
                    Some(Incoming::Closed) | None => {
                        if !self.finished() {
                            let text = failure_text("Codex", None, &self.stderr);
                            send_sse(tx, "error", json!({ "message": text })).await;
                        }
                        return TurnEnd::Broken;
                    }
                },
            }
        }
    }

    /// Stop was pressed: interrupt the Codex turn and wait for it to wind down.
    async fn interrupt(
        &self,
        codex_turn: Option<String>,
        turn: &mut TurnTracker,
        incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    ) -> TurnEnd {
        let Some(codex_turn) = codex_turn else { return TurnEnd::Broken };
        let rpc = self.rpc.clone();
        let thread_id = self.thread_id.clone();
        tokio::spawn(async move {
            let _ = rpc.request("turn/interrupt", json!({ "threadId": thread_id, "turnId": codex_turn })).await;
        });
        let deadline = tokio::time::sleep(INTERRUPT_GRACE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return TurnEnd::Broken,
                msg = incoming.recv() => match msg {
                    Some(Incoming::Notification { method, params }) => {
                        let outs = self.events.lock().map(|mut e| e.feed(&method, &params)).unwrap_or_default();
                        for ev in outs {
                            if let Out::ToolCall { name, args, .. } = &ev {
                                turn.note_tool(name, args);
                            }
                        }
                        if self.finished() {
                            return TurnEnd::Stopped;
                        }
                    }
                    // Nobody is left to ask: refuse so Codex can stop.
                    Some(Incoming::Request { id, .. }) => self.rpc.respond_error(&id, -32000, "The turn was stopped."),
                    _ => return TurnEnd::Broken,
                },
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn on_request(
        &self,
        state: &AppState,
        tx: &SseTx,
        turn_id: &str,
        id: Value,
        method: &str,
        params: &Value,
    ) {
        let rpc = self.rpc.clone();
        let request_id = id.to_string();
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" | "item/permissions/requestApproval" => {
                let approval = self.events.lock().ok().and_then(|e| e.approval(method, params));
                let Some(approval) = approval else {
                    rpc.respond_error(&id, -32601, "Unsupported approval request.");
                    return;
                };
                let (tool, input) = match approval {
                    Approval::Command { command, reason } => {
                        (shell_label().to_string(), json!({ "command": command, "reason": reason }))
                    }
                    Approval::FileChange { path, reason } => {
                        ("Edit".to_string(), json!({ "file_path": path, "reason": reason }))
                    }
                    Approval::Permissions { reason } => ("Permissions".to_string(), json!({ "reason": reason })),
                };
                let available = params["availableDecisions"].clone();
                let granted = params["permissions"].clone();
                let kind = method.to_string();
                let prompt = PermissionPrompt { request_id, tool, input };
                broker::ask_permission(state, tx, turn_id, &self.rules, prompt, move |d| {
                    let body = if kind == "item/permissions/requestApproval" {
                        permissions_response(d, &granted)
                    } else {
                        json!({ "decision": pick_decision(d, &available) })
                    };
                    rpc.respond(&id, body);
                })
                .await;
            }
            "item/tool/requestUserInput" => {
                let questions = codex_events::parse_questions(params);
                broker::ask_questions(state, tx, turn_id, &request_id, &questions, move |answers| match answers {
                    Some(a) => rpc.respond(&id, codex_events::question_response(&a)),
                    None => rpc.respond(&id, json!({ "answers": {} })),
                })
                .await;
            }
            "mcpServer/elicitation/request" => {
                let server = params["serverName"].as_str().unwrap_or("mcp").to_string();
                let message = params["message"].as_str().unwrap_or("").to_string();
                let simple = params["requestedSchema"]["properties"].as_object().map_or(true, |p| p.is_empty());
                if !simple {
                    rpc.respond(&id, json!({ "action": "decline" }));
                    return;
                }
                let prompt = PermissionPrompt {
                    request_id,
                    tool: format!("mcp:{server}"),
                    input: json!({ "message": message }),
                };
                broker::ask_permission(state, tx, turn_id, &self.rules, prompt, move |d| {
                    let body = if d.allow { json!({ "action": "accept", "content": {} }) } else { json!({ "action": "decline" }) };
                    rpc.respond(&id, body);
                })
                .await;
            }
            _ => rpc.respond_error(&id, -32601, "Vireo does not support this request."),
        }
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The wire decision for a command / file-change approval, limited to what Codex offered.
fn pick_decision(d: PermissionDecision, available: &Value) -> Value {
    let offered = |name: &str| {
        available.as_array().is_none_or(|a| a.iter().any(|x| x.as_str() == Some(name)))
    };
    if !d.allow {
        return json!(if offered("decline") { "decline" } else { "cancel" });
    }
    if d.remember {
        if offered("acceptForSession") && available.is_array() {
            return json!("acceptForSession");
        }
        let amendment = available.as_array().and_then(|a| a.iter().find(|x| x.get("acceptWithExecpolicyAmendment").is_some()));
        if let Some(a) = amendment {
            return a.clone();
        }
    }
    json!("accept")
}

fn permissions_response(d: PermissionDecision, requested: &Value) -> Value {
    if d.allow {
        json!({ "permissions": requested, "scope": if d.remember { "session" } else { "turn" } })
    } else {
        json!({ "permissions": {}, "scope": "turn" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(allow: bool, remember: bool) -> PermissionDecision {
        PermissionDecision { allow, remember }
    }

    #[test]
    fn modes_map_to_policies() {
        assert_eq!(policy(PermMode::Ask).0, "untrusted");
        assert_eq!(policy(PermMode::AutoEdit).0, "on-request");
        assert_eq!(policy(PermMode::FullAccess).1, "danger-full-access");
    }

    #[test]
    fn decisions_respect_what_codex_offered() {
        // Captured `availableDecisions` of a command approval: no `decline`, no session scope.
        let offered = json!(["accept", { "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["echo", "hi"] } }, "cancel"]);
        assert_eq!(pick_decision(d(true, false), &offered), json!("accept"));
        assert_eq!(
            pick_decision(d(true, true), &offered),
            json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["echo", "hi"] } })
        );
        assert_eq!(pick_decision(d(false, false), &offered), json!("cancel"));
        let full = json!(["accept", "acceptForSession", "decline", "cancel"]);
        assert_eq!(pick_decision(d(true, true), &full), json!("acceptForSession"));
        assert_eq!(pick_decision(d(false, false), &full), json!("decline"));
        // File-change approvals list no `availableDecisions` at all.
        assert_eq!(pick_decision(d(false, false), &Value::Null), json!("decline"));
        assert_eq!(pick_decision(d(true, true), &Value::Null), json!("accept"));
    }

    #[test]
    fn denying_extra_permissions_grants_nothing() {
        let req = json!({ "network": { "enabled": true } });
        assert_eq!(permissions_response(d(true, false), &req), json!({ "permissions": req, "scope": "turn" }));
        assert_eq!(permissions_response(d(false, false), &req)["permissions"], json!({}));
    }
}
