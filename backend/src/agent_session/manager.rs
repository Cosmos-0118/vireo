//! Registry of live agent sessions, one per chat thread.

use super::acp::AcpSession;
use super::claude::ClaudeSession;
use super::codex::CodexSession;
use super::process::resolve_exe;
use super::types::{SlashCommand, SseTx, TurnEnd, TurnInput};
use crate::checkpoint::TurnTracker;
use crate::cli_agent::{CliKind, PermMode};
use crate::state::AppState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Sessions untouched this long are shut down; the thread resumes on its next message.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
const SWEEP_EVERY: Duration = Duration::from_secs(60);

pub enum Session {
    Claude(ClaudeSession),
    Codex(CodexSession),
    Acp(AcpSession),
}

impl Session {
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Claude(s) => s.is_alive(),
            Self::Codex(s) => s.is_alive(),
            Self::Acp(s) => s.is_alive(),
        }
    }

    /// The agent's own conversation id, saved with the thread so it can be resumed later.
    pub fn resume_id(&self) -> String {
        match self {
            Self::Claude(s) => s.resume_id(),
            Self::Codex(s) => s.resume_id(),
            Self::Acp(s) => s.resume_id(),
        }
    }

    pub fn commands(&self) -> Vec<SlashCommand> {
        match self {
            Self::Claude(s) => s.commands(),
            Self::Codex(s) => s.commands(),
            Self::Acp(s) => s.commands(),
        }
    }

    pub fn merge_rules(&self, rules: &[String]) {
        match self {
            Self::Claude(s) => s.merge_rules(rules),
            Self::Codex(s) => s.merge_rules(rules),
            Self::Acp(s) => s.merge_rules(rules),
        }
    }

    pub fn shutdown(&self) {
        match self {
            Self::Claude(s) => s.shutdown(),
            Self::Codex(s) => s.shutdown(),
            Self::Acp(s) => s.shutdown(),
        }
    }

    pub async fn run_turn(&self, state: &AppState, tx: &SseTx, turn: &mut TurnTracker, input: &TurnInput) -> TurnEnd {
        match self {
            Self::Claude(s) => s.run_turn(state, tx, turn, input).await,
            Self::Codex(s) => s.run_turn(state, tx, turn, input).await,
            Self::Acp(s) => s.run_turn(state, tx, turn, input).await,
        }
    }

    fn needs_relaunch(&self, mode: PermMode) -> bool {
        matches!(self, Self::Codex(s) if s.needs_relaunch(mode))
    }
}

struct Entry {
    session: Arc<Session>,
    kind: CliKind,
    ws: PathBuf,
    last_used: Instant,
    /// The agent already holds this thread's earlier turns (it ran one, or was resumed).
    has_history: bool,
}

/// A session ready for a turn, and how it came to be.
pub struct Acquired {
    pub session: Arc<Session>,
    /// The agent has no memory of earlier turns: send them along (flattened) with the prompt.
    pub needs_history: bool,
    /// An existing conversation was resumed in a new process.
    pub resumed: bool,
}

type Sessions = Arc<Mutex<HashMap<String, Entry>>>;

#[derive(Default)]
pub struct SessionManager {
    map: Sessions,
    reaper: std::sync::Once,
}

impl SessionManager {
    /// Returns the thread's live session, starting (or resuming) one when needed.
    #[allow(clippy::too_many_arguments)]
    pub async fn acquire(
        &self,
        thread_id: &str,
        kind: CliKind,
        ws: &Path,
        model: &str,
        mode: PermMode,
        resume_id: Option<&str>,
        rules: &[String],
    ) -> Result<Acquired, String> {
        self.ensure_reaper();
        let mut resume = resume_id.map(str::to_string);
        {
            let mut map = self.map.lock().await;
            if let Some(entry) = map.get_mut(thread_id) {
                let reusable = entry.kind == kind
                    && entry.ws == ws
                    && entry.session.is_alive()
                    && !entry.session.needs_relaunch(mode);
                if reusable {
                    entry.last_used = Instant::now();
                    entry.session.merge_rules(rules);
                    return Ok(Acquired {
                        session: entry.session.clone(),
                        needs_history: !entry.has_history,
                        resumed: false,
                    });
                }
                // Same conversation, different process: resume where the old one left off.
                if entry.kind == kind && entry.ws == ws {
                    resume = Some(entry.session.resume_id());
                }
                entry.session.shutdown();
                map.remove(thread_id);
            }
        }

        let exe = resolve_exe(kind)
            .await
            .ok_or_else(|| format!("{} was not found. Install it with: {}", kind.label(), kind.install_hint()))?;
        let rules = rules.to_vec();

        let mut resumed = false;
        let mut started = None;
        if let Some(id) = resume.as_deref() {
            if let Ok(s) = start(kind, &exe, ws, model, mode, Some(id), rules.clone()).await {
                resumed = true;
                started = Some(s);
            }
        }
        let session = match started {
            Some(s) => s,
            None => start(kind, &exe, ws, model, mode, None, rules).await?,
        };

        let session = Arc::new(session);
        let mut map = self.map.lock().await;
        // A concurrent request (the `/` palette warms the session while the first message is
        // sent) may have started one too: keep the one already in use.
        if let Some(existing) = map.get_mut(thread_id) {
            if existing.kind == kind && existing.ws == ws && existing.session.is_alive() {
                session.shutdown();
                existing.last_used = Instant::now();
                return Ok(Acquired {
                    session: existing.session.clone(),
                    needs_history: !existing.has_history,
                    resumed: false,
                });
            }
        }
        if let Some(old) = map.insert(
            thread_id.to_string(),
            Entry { session: session.clone(), kind, ws: ws.to_path_buf(), last_used: Instant::now(), has_history: resumed },
        ) {
            old.session.shutdown();
        }
        Ok(Acquired { session, needs_history: !resumed, resumed })
    }

    /// Ends a thread's session (thread deleted, or the session broke).
    pub async fn close(&self, thread_id: &str) {
        if let Some(entry) = self.map.lock().await.remove(thread_id) {
            entry.session.shutdown();
        }
    }

    /// Marks a turn as run: the agent now remembers this thread, so later turns send only the new message.
    pub async fn touch(&self, thread_id: &str) {
        if let Some(e) = self.map.lock().await.get_mut(thread_id) {
            e.last_used = Instant::now();
            e.has_history = true;
        }
    }

    /// Ends every session (the workspace changed).
    pub async fn close_all(&self) {
        for (_, entry) in self.map.lock().await.drain() {
            entry.session.shutdown();
        }
    }

    /// Starts the background sweep (once), which drops dead and idle sessions.
    fn ensure_reaper(&self) {
        self.reaper.call_once(|| {
            let map = self.map.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(SWEEP_EVERY);
                loop {
                    tick.tick().await;
                    map.lock().await.retain(|_, e| {
                        let keep = e.session.is_alive() && e.last_used.elapsed() < IDLE_LIMIT;
                        if !keep {
                            e.session.shutdown();
                        }
                        keep
                    });
                }
            });
        });
    }
}

async fn start(
    kind: CliKind,
    exe: &Path,
    ws: &Path,
    model: &str,
    mode: PermMode,
    resume: Option<&str>,
    rules: Vec<String>,
) -> Result<Session, String> {
    match kind {
        CliKind::Claude => ClaudeSession::start(exe, ws, model, mode, resume, rules).await.map(Session::Claude),
        CliKind::Codex => CodexSession::start(exe, ws, model, mode, resume, rules).await.map(Session::Codex),
        CliKind::Copilot => AcpSession::start(kind, exe, ws, model, resume, rules).await.map(Session::Acp),
    }
}
