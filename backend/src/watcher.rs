use crate::git::{resolve_git_dirs, GitDirs};
use crate::state::{is_ignored_dir_name, WorkspaceEvent};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

/// Quiet period after the last event before a batch is flushed.
const DEBOUNCE: Duration = Duration::from_millis(80);
/// A batch is flushed at the latest this long after its first event, so a continuous stream of
/// writes (build output, `npm install`) cannot starve the UI of updates.
const MAX_BATCH_AGE: Duration = Duration::from_millis(280);
/// Paths reported per event. Past this the batch is reported as "something changed" (empty list),
/// which clients treat as a full refresh.
const MAX_REPORTED_PATHS: usize = 200;
/// Hard cap on the pending set so a huge burst cannot grow memory without bound.
const MAX_PENDING: usize = 2000;

/// Normalize a path to forward slashes without Windows `\\?\` UNC prefix.
fn normalize_path_str(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string()
}

/// Strip `base` from `path` in a platform-appropriate way (case-insensitive on Windows,
/// ignoring `\\?\` UNC prefixes), returning a forward-slash relative string.
fn strip_prefix_norm(base: &Path, path: &Path) -> Option<String> {
    let b = normalize_path_str(base);
    let p = normalize_path_str(path);
    if b.is_empty() {
        return None;
    }
    let matches_prefix = |a: &str, pref: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(pref)
        } else {
            a == pref
        }
    };
    if matches_prefix(&p, &b) {
        return Some(String::new());
    }
    if p.len() > b.len() && p.as_bytes()[b.len()] == b'/' {
        if let Some(head) = p.get(..b.len()) {
            if matches_prefix(head, &b) {
                return Some(p[b.len() + 1..].to_string());
            }
        }
    }
    None
}

/// `.git` entries (relative to the git dir, without a leading `.git/`) whose change alters what
/// source control shows: branch, staging, upstream/remotes, tags, fetch state, stashes, or
/// in-progress merge/rebase. Object storage, reflogs (except stash), and `*.lock` files are noise.
fn git_rel_relevant(rel_in_git: &str) -> bool {
    if rel_in_git.is_empty() || rel_in_git.ends_with(".lock") {
        return false;
    }
    matches!(
        rel_in_git,
        "HEAD"
            | "index"
            | "packed-refs"
            | "config"
            | "FETCH_HEAD"
            | "ORIG_HEAD"
            | "MERGE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "rebase-merge"
            | "rebase-apply"
            | "logs/refs/stash"
            | "reftable/tables.list"
    ) || rel_in_git.starts_with("refs/")
        || rel_in_git.starts_with("rebase-merge/")
        || rel_in_git.starts_with("rebase-apply/")
}

/// Classify a filesystem event path against the workspace and resolved git dirs.
/// Returns `Some(normalized_rel)` to report (with git metadata normalized to `.git/...`),
/// or `None` if the event should be ignored.
fn classify_event_path(
    workspace: &Path,
    git_dirs: Option<&GitDirs>,
    path: &Path,
) -> Option<String> {
    if let Some(gd) = git_dirs {
        if let Some(rel) = strip_prefix_norm(&gd.git_dir, path) {
            return git_rel_relevant(&rel).then(|| format!(".git/{rel}"));
        }
        if gd.common_dir != gd.git_dir {
            if let Some(rel) = strip_prefix_norm(&gd.common_dir, path) {
                return git_rel_relevant(&rel).then(|| format!(".git/{rel}"));
            }
        }
    }

    if let Some(rel) = strip_prefix_norm(workspace, path) {
        if rel.is_empty() {
            return None;
        }
        for comp in rel.split('/') {
            if comp == ".git" {
                let in_git = rel.strip_prefix(".git/").unwrap_or("");
                return git_rel_relevant(in_git).then(|| rel);
            }
            if is_ignored_dir_name(comp) {
                return None;
            }
        }
        return Some(rel);
    }

    // Outside workspace and outside known git dirs: ignore.
    None
}

#[cfg(test)]
fn should_ignore_path(workspace: &Path, path: &Path) -> bool {
    classify_event_path(workspace, None, path).is_none()
}

enum Msg {
    Fs(notify::Result<Event>),
    Retarget(PathBuf),
}

#[derive(Default)]
struct GitWatchState {
    dirs: Option<GitDirs>,
    watched: Vec<PathBuf>,
}

impl GitWatchState {
    fn watch_path(&mut self, w: &mut RecommendedWatcher, p: PathBuf, mode: RecursiveMode) {
        if self.watched.iter().any(|x| x == &p) {
            return;
        }
        if p.exists() && w.watch(&p, mode).is_ok() {
            self.watched.push(p);
        }
    }

    fn attach(&mut self, w: &mut RecommendedWatcher, ws: &Path) {
        let Some(dirs) = resolve_git_dirs(ws) else {
            return;
        };
        let git_dir = dirs.git_dir.clone();
        let common_dir = dirs.common_dir.clone();
        self.dirs = Some(dirs);
        self.watch_path(w, git_dir.clone(), RecursiveMode::NonRecursive);
        if common_dir != git_dir {
            self.watch_path(w, common_dir.clone(), RecursiveMode::NonRecursive);
        }
        self.watch_path(w, common_dir.join("refs"), RecursiveMode::Recursive);
        self.watch_path(w, common_dir.join("logs").join("refs"), RecursiveMode::NonRecursive);
        self.watch_path(w, common_dir.join("reftable"), RecursiveMode::NonRecursive);
    }

    fn unwatch(&mut self, w: &mut RecommendedWatcher) {
        for p in self.watched.drain(..) {
            let _ = w.unwatch(&p);
        }
        self.dirs = None;
    }
}

/// Watch the workspace without descending into dependency / build / VCS-object directories:
/// the root non-recursively, every non-ignored top-level directory recursively, and only the
/// `.git` files/directories that alter source control state.
fn watch_workspace(w: &mut RecommendedWatcher, ws: &Path, git_watch: &mut GitWatchState) {
    if ws.as_os_str().is_empty() {
        return;
    }
    let _ = w.watch(ws, RecursiveMode::NonRecursive);
    if let Ok(rd) = std::fs::read_dir(ws) {
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || is_ignored_dir_name(&name) {
                continue;
            }
            let _ = w.watch(&entry.path(), RecursiveMode::Recursive);
        }
    }
    git_watch.attach(w, ws);
}

fn unwatch_workspace(w: &mut RecommendedWatcher, ws: &Path, git_watch: &mut GitWatchState) {
    if ws.as_os_str().is_empty() {
        return;
    }
    let _ = w.unwatch(ws);
    if let Ok(rd) = std::fs::read_dir(ws) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let _ = w.unwatch(&entry.path());
            }
        }
    }
    git_watch.unwatch(w);
}

pub fn spawn_watcher(
    initial_workspace: PathBuf,
    events_tx: broadcast::Sender<WorkspaceEvent>,
    git_cache: std::sync::Arc<crate::git::GitCache>,
) -> mpsc::Sender<PathBuf> {
    let (retarget_tx, mut retarget_rx) = mpsc::channel::<PathBuf>(8);
    let (tx, rx) = std_mpsc::channel::<Msg>();

    // Forward workspace switches into the same channel as fs events so the watcher thread can
    // block on a single `recv()` and costs zero wakeups while idle.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Some(p) = retarget_rx.blocking_recv() {
                if tx.send(Msg::Retarget(p)).is_err() {
                    break;
                }
            }
        });
    }

    std::thread::spawn(move || {
        let mut current_ws = initial_workspace;
        let mut git_watch = GitWatchState::default();

        let mut watcher: Option<RecommendedWatcher> = notify::recommended_watcher({
            let tx = tx.clone();
            move |res| {
                let _ = tx.send(Msg::Fs(res));
            }
        })
        .ok();

        if let Some(w) = watcher.as_mut() {
            watch_workspace(w, &current_ws, &mut git_watch);
        }

        // path -> sequence number of its latest event, so a batch is reported oldest-first and
        // its last entry is the most recently written file (the UI follows that one).
        let mut pending: HashMap<String, u64> = HashMap::new();
        let mut seq: u64 = 0;
        let mut overflow = false;
        let mut first_event: Option<Instant> = None;
        let mut last_event: Option<Instant> = None;

        loop {
            // Idle: block with no timer. Debouncing: wake only when the batch is due.
            let msg = match (first_event, last_event) {
                (Some(first), Some(last)) => {
                    let due = (last + DEBOUNCE).min(first + MAX_BATCH_AGE);
                    match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                        Ok(m) => Some(m),
                        Err(std_mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                _ => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                },
            };

            match msg {
                Some(Msg::Retarget(new_ws)) => {
                    if let Some(w) = watcher.as_mut() {
                        unwatch_workspace(w, &current_ws, &mut git_watch);
                        watch_workspace(w, &new_ws, &mut git_watch);
                    }
                    current_ws = new_ws;
                    pending.clear();
                    overflow = false;
                    first_event = None;
                    last_event = None;
                }
                Some(Msg::Fs(Ok(event))) => {
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    // Dynamically watch newly created top-level directories AND `.git` (e.g. `git init`).
                    if matches!(event.kind, EventKind::Create(_)) {
                        if let Some(w) = watcher.as_mut() {
                            for p in &event.paths {
                                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                                if name == ".git"
                                    && strip_prefix_norm(&current_ws, p).as_deref() == Some(".git")
                                {
                                    git_watch.attach(w, &current_ws);
                                    let now = Instant::now();
                                    first_event.get_or_insert(now);
                                    last_event = Some(now);
                                    seq += 1;
                                    pending.insert(".git/HEAD".to_string(), seq);
                                    continue;
                                }
                                if p.parent() == Some(current_ws.as_path()) && p.is_dir() {
                                    if name != ".git" && !is_ignored_dir_name(name) {
                                        let _ = w.watch(p, RecursiveMode::Recursive);
                                    }
                                }
                                // If `.git/logs/refs` or `.git/reftable` appeared lazily (e.g. first stash),
                                // re-attach git watches so future updates are caught.
                                if let Some(gd) = git_watch.dirs.clone() {
                                    if p.starts_with(&gd.common_dir)
                                        && matches!(name, "refs" | "logs" | "reftable")
                                    {
                                        git_watch.attach(w, &current_ws);
                                    }
                                }
                            }
                        }
                    }
                    for p in event.paths {
                        let Some(rel) = classify_event_path(&current_ws, git_watch.dirs.as_ref(), &p) else {
                            continue;
                        };
                        let now = Instant::now();
                        first_event.get_or_insert(now);
                        last_event = Some(now);
                        if pending.len() >= MAX_PENDING {
                            overflow = true;
                            continue;
                        }
                        seq += 1;
                        pending.insert(rel, seq);
                    }
                }
                Some(Msg::Fs(Err(_))) => {
                    // Dropped events (e.g. Windows buffer overflow): anything may have changed.
                    let now = Instant::now();
                    first_event.get_or_insert(now);
                    last_event = Some(now);
                    overflow = true;
                }
                None => {}
            }

            if let (Some(first), Some(last)) = (first_event, last_event) {
                let now = Instant::now();
                if now.duration_since(last) >= DEBOUNCE || now.duration_since(first) >= MAX_BATCH_AGE {
                    let paths: Vec<String> = if overflow || pending.len() > MAX_REPORTED_PATHS {
                        Vec::new()
                    } else {
                        let mut ordered: Vec<(String, u64)> = pending.drain().collect();
                        ordered.sort_unstable_by_key(|(_, s)| *s);
                        ordered.into_iter().map(|(p, _)| p).collect()
                    };
                    pending.clear();
                    overflow = false;
                    first_event = None;
                    last_event = None;
                    git_cache.invalidate_volatile();
                    let _ = events_tx.send(WorkspaceEvent::FsChanged { paths });
                }
            }
        }
    });

    retarget_tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_internals_filter() {
        let ws = Path::new("/w");
        let ign = |p: &str| should_ignore_path(ws, &ws.join(p));
        for ok in [
            ".git/HEAD",
            ".git/index",
            ".git/packed-refs",
            ".git/config",
            ".git/FETCH_HEAD",
            ".git/ORIG_HEAD",
            ".git/MERGE_HEAD",
            ".git/refs/heads/main",
            ".git/refs/remotes/origin/main",
            ".git/refs/tags/v1.0.0",
            ".git/refs/stash",
            ".git/logs/refs/stash",
            ".git/reftable/tables.list",
            ".git/rebase-merge",
        ] {
            assert!(!ign(ok), "{ok} must be watched");
        }
        for noisy in [
            ".git/index.lock",
            ".git/objects/ab/cd",
            ".git/logs/HEAD",
            ".git/refs/heads/x.lock",
            ".git/refs/tags/v1.0.0.lock",
            ".git/COMMIT_EDITMSG",
        ] {
            assert!(ign(noisy), "{noisy} must be ignored");
        }
        assert!(!ign("src/main.rs"));
    }

    #[test]
    fn worktree_external_git_dir_normalizes_to_dot_git() {
        let ws = Path::new("/worktrees/feat");
        let dirs = GitDirs {
            git_dir: PathBuf::from("/main/.git/worktrees/feat"),
            common_dir: PathBuf::from("/main/.git"),
        };
        assert_eq!(
            classify_event_path(ws, Some(&dirs), Path::new("/main/.git/refs/tags/v2.0.0")),
            Some(".git/refs/tags/v2.0.0".to_string())
        );
        assert_eq!(
            classify_event_path(ws, Some(&dirs), Path::new("/main/.git/worktrees/feat/HEAD")),
            Some(".git/HEAD".to_string())
        );
        assert_eq!(
            classify_event_path(ws, Some(&dirs), Path::new("/main/.git/objects/aa/bb")),
            None
        );
    }
}
