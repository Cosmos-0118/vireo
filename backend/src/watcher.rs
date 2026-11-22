use crate::state::{is_ignored_dir_name, WorkspaceEvent};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

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

pub fn spawn_watcher(
    initial_workspace: PathBuf,
    events_tx: broadcast::Sender<WorkspaceEvent>,
) -> mpsc::Sender<PathBuf> {
    let (retarget_tx, mut retarget_rx) = mpsc::channel::<PathBuf>(8);

    std::thread::spawn(move || {
        let mut current_ws = initial_workspace;
        let (fs_tx, fs_rx) = std_mpsc::channel::<notify::Result<Event>>();

        let mut watcher: Option<RecommendedWatcher> = notify::recommended_watcher({
            let tx = fs_tx.clone();
            move |res| {
                let _ = tx.send(res);
            }
        })
        .ok();

        if let Some(w) = watcher.as_mut() {
            let _ = w.watch(&current_ws, RecursiveMode::Recursive);
        }

        let mut pending: HashSet<String> = HashSet::new();
        let mut last_event: Option<Instant> = None;

        loop {
            // Check if workspace changed
            if let Ok(new_ws) = retarget_rx.try_recv() {
                if let Some(w) = watcher.as_mut() {
                    let _ = w.unwatch(&current_ws);
                    let _ = w.watch(&new_ws, RecursiveMode::Recursive);
                }
                current_ws = new_ws;
                pending.clear();
                last_event = None;
            }

            match fs_rx.recv_timeout(Duration::from_millis(120)) {
                Ok(Ok(event)) => {
                    for p in event.paths {
                        if !should_ignore_path(&current_ws, &p) {
                            let rel = p
                                .strip_prefix(&current_ws)
                                .map(|r| r.to_string_lossy().replace('\\', "/"))
                                .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"));
                            pending.insert(rel);
                            last_event = Some(Instant::now());
                        }
                    }
                }
                Ok(Err(_)) | Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if let Some(t) = last_event {
                if t.elapsed() >= Duration::from_millis(260) && !pending.is_empty() {
                    let paths: Vec<String> = pending.drain().take(50).collect();
                    let _ = events_tx.send(WorkspaceEvent::FsChanged { paths });
                    last_event = None;
                }
            }
        }
    });

    retarget_tx
}
