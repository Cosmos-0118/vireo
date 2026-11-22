use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    FsChanged { paths: Vec<String> },
    WorkspaceSwitched { path: String },
    CheckpointCreated { checkpoint: Checkpoint },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointFileDelta {
    pub path: String,
    /// `None` if the file did not exist before the turn.
    pub before_blob: Option<String>,
    /// `None` if the turn deleted the file.
    pub after_blob: Option<String>,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub label: String,
    /// Pre-turn tree for turn checkpoints; the saved snapshot for manual ones.
    pub tree_sha: String,
    pub created_at: String,
    /// "turn" (scoped to `files`) or "manual" (whole-workspace snapshot).
    #[serde(default)]
    pub kind: String,
    /// Only the files this turn changed.
    #[serde(default)]
    pub files: Vec<CheckpointFileDelta>,
    /// Set once (part of) the turn has been undone: what was replaced, for 1-click redo.
    #[serde(default)]
    pub redo_files: Option<Vec<CheckpointFileDelta>>,
}

#[derive(Clone)]
pub struct AppState {
    pub workspace: Arc<RwLock<PathBuf>>,
    pub events_tx: broadcast::Sender<WorkspaceEvent>,
    pub checkpoints: Arc<RwLock<Vec<Checkpoint>>>,
    /// Serializes undo/redo so a double click cannot apply a rollback twice.
    pub checkpoint_lock: Arc<tokio::sync::Mutex<()>>,
    pub watcher_retarget_tx: mpsc::Sender<PathBuf>,
    pub http_client: reqwest::Client,
    pub main_window: Arc<OnceLock<tauri::WebviewWindow>>,
    /// Tool-permission prompts from a running agent CLI awaiting the user's answer,
    /// keyed by `(turn_id, request_id)`.
    pub pending_permissions: Arc<std::sync::Mutex<HashMap<(String, String), oneshot::Sender<PermissionDecision>>>>,
}

/// The user's answer to an in-chat permission card.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PermissionDecision {
    pub allow: bool,
    /// Also allow matching requests for the rest of the thread.
    #[serde(default)]
    pub remember: bool,
}

impl AppState {
    pub fn new(initial_workspace: PathBuf, watcher_retarget_tx: mpsc::Sender<PathBuf>) -> Self {
        let (events_tx, _) = broadcast::channel(128);
        Self {
            workspace: Arc::new(RwLock::new(initial_workspace)),
            events_tx,
            checkpoints: Arc::new(RwLock::new(Vec::new())),
            checkpoint_lock: Arc::new(tokio::sync::Mutex::new(())),
            watcher_retarget_tx,
            http_client: reqwest::Client::new(),
            main_window: Arc::new(OnceLock::new()),
            pending_permissions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub async fn workspace_path(&self) -> PathBuf {
        self.workspace.read().await.clone()
    }

    pub fn notify_fs_changed(&self, paths: Vec<String>) {
        let _ = self.events_tx.send(WorkspaceEvent::FsChanged { paths });
    }
}

pub fn resolve_safe_path(workspace: &Path, rel: &str) -> Result<PathBuf, String> {
    let cleaned = rel.trim().replace('\\', "/");
    let rel_path = Path::new(&cleaned);
    if rel_path.is_absolute() {
        return Err("Absolute paths are not allowed".to_string());
    }
    let mut out = workspace.to_path_buf();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Path traversal is not allowed".to_string());
            }
        }
    }
    Ok(out)
}

pub fn is_ignored_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".nuxt"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".turbo"
            | ".cache"
            | "coverage"
            | ".pnpm-store"
            | ".yarn"
            | ".svelte-kit"
            | ".parcel-cache"
            | ".gradle"
    )
}

/// Spawns a background CLI command without flashing a console window on Windows GUI builds.
pub fn background_command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
