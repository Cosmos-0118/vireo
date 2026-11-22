//! Turn-scoped, reversible checkpoints.
//!
//! A checkpoint records only the files one agent turn actually changed, as `(path, before_blob,
//! after_blob)` triples whose blobs live in the git object store. Undo/redo never resets the
//! whole working tree: every file goes through [`apply_change`], which writes the target blob only
//! when the file still matches what the turn left behind, 3-way merges when someone edited it
//! afterwards, and otherwise reports a conflict without touching the file. Every undo records the
//! content it replaced, so it can always be redone.

use crate::agent::send_sse;
use crate::state::{
    background_command, resolve_safe_path, AppState, Checkpoint, CheckpointFileDelta, WorkspaceEvent,
};
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use axum::response::sse::Event;

const MAX_CHECKPOINTS: usize = 50;

// ---------------------------------------------------------------------------------------------
// git plumbing
// ---------------------------------------------------------------------------------------------

struct GitOut {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

async fn git(ws: &Path, args: &[&str], index: Option<&Path>) -> Result<GitOut, String> {
    let mut cmd = background_command("git");
    cmd.args(args).current_dir(ws).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(idx) = index {
        cmd.env("GIT_INDEX_FILE", idx);
    }
    let out = cmd.output().await.map_err(|e| format!("Failed to run git: {e}"))?;
    Ok(GitOut {
        ok: out.status.success(),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

fn text(out: &GitOut) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Repo-root-relative prefix of the workspace ("" when the workspace is the repo root).
async fn workspace_prefix(ws: &Path) -> Result<String, String> {
    let out = git(ws, &["rev-parse", "--show-prefix"], None).await?;
    if !out.ok {
        return Err("Not a git repository".to_string());
    }
    Ok(text(&out).replace('\\', "/"))
}

struct Cx {
    ws: PathBuf,
    /// Repo-root-relative prefix of `ws` (ends with '/' unless empty).
    prefix: String,
}

impl Cx {
    async fn new(ws: &Path) -> Result<Self, String> {
        Ok(Self { ws: ws.to_path_buf(), prefix: workspace_prefix(ws).await? })
    }
    fn root_rel(&self, rel: &str) -> String {
        format!("{}{}", self.prefix, rel)
    }
}

/// Snapshots the working tree (tracked + untracked, honouring .gitignore) into a tree object
/// using a throwaway index, so the user's real index and working tree are never touched.
pub async fn snapshot_tree(ws: &Path) -> Result<String, String> {
    let (is_repo, real_index) = {
        let inside = git(ws, &["rev-parse", "--is-inside-work-tree"], None).await?;
        let idx = git(ws, &["rev-parse", "--git-path", "index"], None).await?;
        let p = PathBuf::from(text(&idx));
        (inside.ok, if p.is_absolute() { p } else { ws.join(p) })
    };
    if !is_repo {
        return Err("Not a git repository".to_string());
    }

    let tmp = std::env::temp_dir().join(format!("vireo-idx-{}", uuid::Uuid::new_v4()));
    // Seeding from the real index lets git reuse its stat cache instead of re-hashing every file.
    let seeded = std::fs::copy(&real_index, &tmp).is_ok();
    let mut add = git(ws, &["add", "-A"], Some(&tmp)).await;
    if seeded && !matches!(&add, Ok(o) if o.ok) {
        let _ = std::fs::remove_file(&tmp);
        add = git(ws, &["add", "-A"], Some(&tmp)).await;
    }
    let result = match add {
        Ok(o) if o.ok => match git(ws, &["write-tree"], Some(&tmp)).await {
            Ok(t) if t.ok => Ok(text(&t)),
            Ok(t) => Err(t.stderr),
            Err(e) => Err(e),
        },
        Ok(o) => Err(o.stderr),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

fn is_zero_sha(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c == '0')
}

/// Per-file changes between two trees (paths are repo-root-relative). Submodules and symlinks
/// are skipped: writing blob text over them would corrupt them.
async fn diff_trees(ws: &Path, from: &str, to: &str) -> Result<Vec<CheckpointFileDelta>, String> {
    let raw = git(ws, &["diff-tree", "-r", "-z", "--raw", "--no-renames", "--no-abbrev", from, to], None).await?;
    if !raw.ok {
        return Err(raw.stderr);
    }
    let num = git(ws, &["diff-tree", "-r", "-z", "--numstat", "--no-renames", from, to], None).await?;
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    if num.ok {
        for rec in String::from_utf8_lossy(&num.stdout).split('\0') {
            let mut it = rec.splitn(3, '\t');
            if let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) {
                counts.insert(p.to_string(), (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
            }
        }
    }

    let mut out = Vec::new();
    let body = String::from_utf8_lossy(&raw.stdout).to_string();
    let mut toks = body.split('\0');
    while let Some(meta) = toks.next() {
        let Some(meta) = meta.strip_prefix(':') else { continue };
        let Some(path) = toks.next() else { break };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let (old_mode, new_mode, old_sha, new_sha) = (f[0], f[1], f[2], f[3]);
        if [old_mode, new_mode].iter().any(|m| *m == "160000" || *m == "120000") {
            continue;
        }
        let (additions, deletions) = counts.get(path).copied().unwrap_or((0, 0));
        out.push(CheckpointFileDelta {
            path: path.to_string(),
            before_blob: (!is_zero_sha(old_sha)).then(|| old_sha.to_string()),
            after_blob: (!is_zero_sha(new_sha)).then(|| new_sha.to_string()),
            additions,
            deletions,
        });
    }
    Ok(out)
}

/// Keeps only files inside the workspace and makes their paths workspace-relative.
fn to_workspace_relative(cx: &Cx, deltas: Vec<CheckpointFileDelta>) -> Vec<CheckpointFileDelta> {
    deltas
        .into_iter()
        .filter_map(|mut d| {
            let rel = d.path.strip_prefix(&cx.prefix)?.to_string();
            d.path = rel;
            Some(d)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The one file-level primitive
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Applied {
    /// File was already at the target content.
    NoOp,
    /// File was rewritten; `before`/`after` are the on-disk blobs around the write.
    Done { before: Option<String>, after: Option<String> },
    /// File changed since `expected` and could not be merged; left untouched.
    Conflict,
}

/// Hashes (and stores) the file's current content exactly as `git add` would. `None` if absent.
async fn hash_disk(cx: &Cx, rel: &str) -> Result<Option<String>, String> {
    let abs = resolve_safe_path(&cx.ws, rel)?;
    match tokio::fs::symlink_metadata(&abs).await {
        Err(_) => return Ok(None),
        Ok(m) if m.is_dir() => return Err(format!("{rel} is a directory")),
        Ok(_) => {}
    }
    let out = git(&cx.ws, &["hash-object", "-w", "--", rel], None).await?;
    if !out.ok {
        return Err(format!("hash-object {rel}: {}", out.stderr));
    }
    Ok(Some(text(&out)))
}

/// Blob content in working-tree form (smudge filters / CRLF conversion applied).
async fn blob_bytes(cx: &Cx, rel: &str, sha: &str) -> Result<Vec<u8>, String> {
    let path_arg = format!("--path={}", cx.root_rel(rel));
    let out = git(&cx.ws, &["cat-file", "--filters", &path_arg, sha], None).await?;
    if out.ok {
        return Ok(out.stdout);
    }
    let raw = git(&cx.ws, &["cat-file", "blob", sha], None).await?;
    if raw.ok {
        Ok(raw.stdout)
    } else {
        Err(format!("cat-file {sha}: {}", raw.stderr))
    }
}

/// 3-way merge of `other - base` into the current file. `None` on any conflict or merge error.
async fn merge3(cx: &Cx, abs: &Path, rel: &str, base: &str, other: &str) -> Result<Option<Vec<u8>>, String> {
    let dir = std::env::temp_dir().join(format!("vireo-merge-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let result = async {
        let (b, o) = (dir.join("base"), dir.join("other"));
        tokio::fs::write(&b, blob_bytes(cx, rel, base).await?).await.map_err(|e| e.to_string())?;
        tokio::fs::write(&o, blob_bytes(cx, rel, other).await?).await.map_err(|e| e.to_string())?;
        let (abs_s, b_s, o_s) = (abs.to_string_lossy(), b.to_string_lossy(), o.to_string_lossy());
        let out = git(&cx.ws, &["merge-file", "-p", &abs_s, &b_s, &o_s], None).await?;
        Ok::<_, String>(out.ok.then_some(out.stdout))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    result
}

/// Moves one file from `expected` to `target` without clobbering anyone else's edits.
async fn apply_change(
    cx: &Cx,
    rel: &str,
    expected: Option<&str>,
    target: Option<&str>,
    force: bool,
) -> Result<Applied, String> {
    let abs = resolve_safe_path(&cx.ws, rel)?;
    let cur = hash_disk(cx, rel).await?;
    if cur.as_deref() == target {
        return Ok(Applied::NoOp);
    }

    let content: Option<Vec<u8>> = if force || cur.as_deref() == expected {
        match target {
            Some(sha) => Some(blob_bytes(cx, rel, sha).await?),
            None => None,
        }
    } else {
        match (cur.as_deref(), expected, target) {
            (Some(_), Some(base), Some(other)) => match merge3(cx, &abs, rel, base, other).await? {
                Some(merged) => Some(merged),
                None => return Ok(Applied::Conflict),
            },
            _ => return Ok(Applied::Conflict),
        }
    };

    match content {
        Some(bytes) => {
            if let Some(parent) = abs.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
            }
            tokio::fs::write(&abs, bytes).await.map_err(|e| format!("write {rel}: {e}"))?;
        }
        None => match tokio::fs::remove_file(&abs).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("delete {rel}: {e}")),
        },
    }
    let after = hash_disk(cx, rel).await?;
    Ok(Applied::Done { before: cur, after })
}

// ---------------------------------------------------------------------------------------------
// Checkpoint store
// ---------------------------------------------------------------------------------------------

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

async fn store(state: &AppState, cp: &Checkpoint) {
    {
        let mut cps = state.checkpoints.write().await;
        cps.insert(0, cp.clone());
        cps.truncate(MAX_CHECKPOINTS);
    }
    let _ = state.events_tx.send(WorkspaceEvent::CheckpointCreated { checkpoint: cp.clone() });
}

async fn update_stored(state: &AppState, cp: &Checkpoint) {
    let mut cps = state.checkpoints.write().await;
    if let Some(slot) = cps.iter_mut().find(|c| c.id == cp.id) {
        *slot = cp.clone();
    }
}

/// Whole-workspace manual snapshot (the "Save a checkpoint" button).
pub async fn create_checkpoint_internal(state: &AppState, label: String) -> Result<Checkpoint, String> {
    let ws = state.workspace_path().await;
    let tree_sha = snapshot_tree(&ws).await?;
    let cp = Checkpoint {
        id: new_id(),
        label,
        tree_sha,
        created_at: Utc::now().format("%H:%M:%S").to_string(),
        kind: "manual".to_string(),
        files: vec![],
        redo_files: None,
    };
    store(state, &cp).await;
    Ok(cp)
}

#[derive(Deserialize)]
pub struct CheckpointReq {
    pub label: Option<String>,
}

pub async fn create_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<CheckpointReq>,
) -> Result<Json<Checkpoint>, (StatusCode, String)> {
    let label = req
        .label
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Manual checkpoint".to_string());
    create_checkpoint_internal(&state, label)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

// ---------------------------------------------------------------------------------------------
// Turn tracking
// ---------------------------------------------------------------------------------------------

/// Watches one agent turn. Nothing is shown to the user unless the turn really changed files.
pub struct TurnTracker {
    enabled: bool,
    pre_tree: Option<String>,
    touched: HashSet<String>,
    /// A shell command (or other opaque mutation) ran: any changed file may be ours.
    opaque: bool,
}

fn norm_key(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_lowercase()
}

impl TurnTracker {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, pre_tree: None, touched: HashSet::new(), opaque: false }
    }

    /// Silently records the pre-turn state. Emits nothing.
    pub async fn begin(&mut self, state: &AppState) {
        if !self.enabled || self.pre_tree.is_some() {
            return;
        }
        let ws = state.workspace_path().await;
        self.pre_tree = snapshot_tree(&ws).await.ok();
    }

    pub fn note_tool(&mut self, name: &str, args: &Value) {
        match name {
            "write_file" | "edit_file" | "symbol_edit" => match args.get("path").and_then(|v| v.as_str()) {
                Some(p) if !p.is_empty() => {
                    self.touched.insert(norm_key(p));
                }
                _ => self.opaque = true,
            },
            "run_command" => self.opaque = true,
            "ast" if args.get("rewrite").and_then(|v| v.as_str()).map_or(false, |r| !r.is_empty()) => {
                self.opaque = true
            }
            _ => {}
        }
    }

    /// Computes what the turn changed. Returns (and stores) a checkpoint only if files changed.
    pub async fn finish(
        &mut self,
        state: &AppState,
        tx: Option<&mpsc::Sender<Result<Event, Infallible>>>,
    ) -> Option<Checkpoint> {
        let pre = self.pre_tree.take()?;
        let ws = state.workspace_path().await;
        let post = snapshot_tree(&ws).await.ok()?;
        if post == pre {
            return None;
        }
        let cx = Cx::new(&ws).await.ok()?;
        let mut files = to_workspace_relative(&cx, diff_trees(&ws, &pre, &post).await.ok()?);
        if !self.opaque {
            files.retain(|f| self.touched.contains(&norm_key(&f.path)));
        }
        if files.is_empty() {
            return None;
        }
        let cp = Checkpoint {
            id: new_id(),
            label: "Agent turn".to_string(),
            tree_sha: pre,
            created_at: Utc::now().format("%H:%M:%S").to_string(),
            kind: "turn".to_string(),
            files,
            redo_files: None,
        };
        store(state, &cp).await;
        if let Some(tx) = tx {
            send_sse(tx, "checkpoint", serde_json::to_value(&cp).unwrap_or(Value::Null)).await;
        }
        Some(cp)
    }
}

// ---------------------------------------------------------------------------------------------
// Undo / redo
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RollbackReq {
    pub id: String,
    /// Overwrite files that changed after the turn (their current content stays redoable).
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, Default)]
pub struct RollbackRes {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub restored: usize,
    pub conflicts: Vec<String>,
    pub checkpoint: Option<Checkpoint>,
}

fn err500(e: String) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e)
}

async fn find_checkpoint(state: &AppState, id: &str) -> Result<Checkpoint, (StatusCode, String)> {
    state
        .checkpoints
        .read()
        .await
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Checkpoint not found".to_string()))
}

pub async fn rollback_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<RollbackReq>,
) -> Result<Json<RollbackRes>, (StatusCode, String)> {
    let _guard = state.checkpoint_lock.lock().await;
    let mut cp = find_checkpoint(&state, &req.id).await?;
    let ws = state.workspace_path().await;
    let cx = Cx::new(&ws).await.map_err(err500)?;

    if cp.kind == "manual" {
        return restore_manual(&state, &cx, &cp).await.map(Json).map_err(err500);
    }

    let mut redo: Vec<CheckpointFileDelta> = cp.redo_files.clone().unwrap_or_default();
    let done: HashSet<String> = redo.iter().map(|r| r.path.clone()).collect();
    let (mut restored, mut conflicts, mut changed) = (0usize, Vec::new(), Vec::new());

    for f in cp.files.iter().filter(|f| !done.contains(&f.path)) {
        match apply_change(&cx, &f.path, f.after_blob.as_deref(), f.before_blob.as_deref(), req.force)
            .await
            .map_err(err500)?
        {
            Applied::Done { before, after } => {
                restored += 1;
                changed.push(f.path.clone());
                redo.push(CheckpointFileDelta {
                    path: f.path.clone(),
                    before_blob: before,
                    after_blob: after,
                    additions: f.deletions,
                    deletions: f.additions,
                });
            }
            Applied::NoOp => redo.push(CheckpointFileDelta {
                path: f.path.clone(),
                before_blob: f.before_blob.clone(),
                after_blob: f.before_blob.clone(),
                additions: 0,
                deletions: 0,
            }),
            Applied::Conflict => conflicts.push(f.path.clone()),
        }
    }

    cp.redo_files = (!redo.is_empty()).then_some(redo);
    update_stored(&state, &cp).await;
    if !changed.is_empty() {
        state.notify_fs_changed(changed);
    }
    let ok = conflicts.is_empty();
    Ok(Json(RollbackRes {
        ok,
        stdout: format!("Reverted {restored} file(s)"),
        stderr: if ok {
            String::new()
        } else {
            format!("{} file(s) changed since this turn and were left alone: {}", conflicts.len(), conflicts.join(", "))
        },
        restored,
        conflicts,
        checkpoint: Some(cp),
    }))
}

pub async fn redo_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<RollbackReq>,
) -> Result<Json<RollbackRes>, (StatusCode, String)> {
    let _guard = state.checkpoint_lock.lock().await;
    let mut cp = find_checkpoint(&state, &req.id).await?;
    let Some(records) = cp.redo_files.clone() else {
        return Err((StatusCode::BAD_REQUEST, "Nothing to redo".to_string()));
    };
    let ws = state.workspace_path().await;
    let cx = Cx::new(&ws).await.map_err(err500)?;

    let (mut restored, mut conflicts, mut changed, mut left) = (0usize, Vec::new(), Vec::new(), Vec::new());
    for r in records {
        match apply_change(&cx, &r.path, r.after_blob.as_deref(), r.before_blob.as_deref(), req.force)
            .await
            .map_err(err500)?
        {
            Applied::Done { .. } => {
                restored += 1;
                changed.push(r.path.clone());
            }
            Applied::NoOp => {}
            Applied::Conflict => {
                conflicts.push(r.path.clone());
                left.push(r);
            }
        }
    }
    cp.redo_files = (!left.is_empty()).then_some(left);
    update_stored(&state, &cp).await;
    if !changed.is_empty() {
        state.notify_fs_changed(changed);
    }
    let ok = conflicts.is_empty();
    Ok(Json(RollbackRes {
        ok,
        stdout: format!("Re-applied {restored} file(s)"),
        stderr: if ok {
            String::new()
        } else {
            format!("{} file(s) changed since the undo and were left alone: {}", conflicts.len(), conflicts.join(", "))
        },
        restored,
        conflicts,
        checkpoint: Some(cp),
    }))
}

/// Restores a manual whole-workspace snapshot. The current state is saved as a new manual
/// checkpoint first, so the restore itself can be reverted from the same list.
async fn restore_manual(state: &AppState, cx: &Cx, cp: &Checkpoint) -> Result<RollbackRes, String> {
    let current = snapshot_tree(&cx.ws).await?;
    if current == cp.tree_sha {
        return Ok(RollbackRes { ok: true, stdout: "Already at this checkpoint".into(), ..Default::default() });
    }
    let backup = Checkpoint {
        id: new_id(),
        label: "Before restore".to_string(),
        tree_sha: current.clone(),
        created_at: Utc::now().format("%H:%M:%S").to_string(),
        kind: "manual".to_string(),
        files: vec![],
        redo_files: None,
    };
    store(state, &backup).await;

    let deltas = to_workspace_relative(cx, diff_trees(&cx.ws, &current, &cp.tree_sha).await?);
    let (mut restored, mut changed) = (0usize, Vec::new());
    for d in deltas {
        // `before_blob` is the file as snapshotted a moment ago; the backup above preserves it.
        if let Applied::Done { .. } = apply_change(cx, &d.path, d.before_blob.as_deref(), d.after_blob.as_deref(), true).await? {
            restored += 1;
            changed.push(d.path);
        }
    }
    if !changed.is_empty() {
        state.notify_fs_changed(changed);
    }
    Ok(RollbackRes {
        ok: true,
        stdout: format!("Restored {restored} file(s); previous state saved as a checkpoint"),
        restored,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests;
