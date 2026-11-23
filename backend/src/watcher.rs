use crate::state::{is_ignored_dir_name, WorkspaceEvent};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

/// Quiet period after the last event before a batch is flushed.
const DEBOUNCE: Duration = Duration::from_millis(260);
/// A batch is flushed at the latest this long after its first event, so a continuous stream of
/// writes (build output, `npm install`) cannot starve the UI of updates.
const MAX_BATCH_AGE: Duration = Duration::from_millis(600);
/// Paths reported per event. Past this the batch is reported as "something changed" (empty list),
/// which clients treat as a full refresh.
const MAX_REPORTED_PATHS: usize = 200;
/// Hard cap on the pending set so a huge burst cannot grow memory without bound.
const MAX_PENDING: usize = 2000;

fn should_ignore_path(workspace: &Path, path: &Path) -> bool {
    let rel = match path.strip_prefix(workspace) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for comp in rel.components() {
        if let Some(s) = comp.as_os_str().to_str() {
            if s == ".git" {
                // Allow .git/HEAD and .git/index so branch/staging changes trigger refresh
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if rel_str == ".git/HEAD"
                    || rel_str == ".git/index"
                    || rel_str.starts_with(".git/refs/heads/")
                {
                    return false;
                }
                return true;
            }
            if is_ignored_dir_name(s) {
                return true;
            }
        }
    }
    false
}

enum Msg {
    Fs(notify::Result<Event>),
    Retarget(PathBuf),
}

/// Watch the workspace without descending into dependency / build / VCS-object directories:
/// the root non-recursively, every non-ignored top-level directory recursively, and only the
/// `.git` files that matter (HEAD, index, refs/heads).
fn watch_workspace(w: &mut RecommendedWatcher, ws: &Path) {
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
    let git = ws.join(".git");
    let _ = w.watch(&git, RecursiveMode::NonRecursive);
    let _ = w.watch(&git.join("refs").join("heads"), RecursiveMode::Recursive);
}

fn unwatch_workspace(w: &mut RecommendedWatcher, ws: &Path) {
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
    let git = ws.join(".git");
    let _ = w.unwatch(&git);
    let _ = w.unwatch(&git.join("refs").join("heads"));
}

pub fn spawn_watcher(
    initial_workspace: PathBuf,
    events_tx: broadcast::Sender<WorkspaceEvent>,
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

        let mut watcher: Option<RecommendedWatcher> = notify::recommended_watcher({
            let tx = tx.clone();
            move |res| {
                let _ = tx.send(Msg::Fs(res));
            }
        })
        .ok();

        if let Some(w) = watcher.as_mut() {
            watch_workspace(w, &current_ws);
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
                        unwatch_workspace(w, &current_ws);
                        watch_workspace(w, &new_ws);
                    }
                    current_ws = new_ws;
                    pending.clear();
                    overflow = false;
                    first_event = None;
                    last_event = None;
                }
                Some(Msg::Fs(Ok(event))) => {
                    // A new top-level directory appeared: start watching it too.
                    if matches!(event.kind, EventKind::Create(_)) {
                        if let Some(w) = watcher.as_mut() {
                            for p in &event.paths {
                                if p.parent() == Some(current_ws.as_path()) && p.is_dir() {
                                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                                    if name != ".git" && !is_ignored_dir_name(name) {
                                        let _ = w.watch(p, RecursiveMode::Recursive);
                                    }
                                }
                            }
                        }
                    }
                    for p in event.paths {
                        if should_ignore_path(&current_ws, &p) {
                            continue;
                        }
                        let now = Instant::now();
                        first_event.get_or_insert(now);
                        last_event = Some(now);
                        if pending.len() >= MAX_PENDING {
                            overflow = true;
                            continue;
                        }
                        seq += 1;
                        let rel = p
                            .strip_prefix(&current_ws)
                            .map(|r| r.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"));
                        pending.insert(rel, seq);
                    }
                }
                Some(Msg::Fs(Err(_))) | None => {}
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
                    let _ = events_tx.send(WorkspaceEvent::FsChanged { paths });
                }
            }
        }
    });

    retarget_tx
}
