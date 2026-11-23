//! Spawning and tearing down the child processes behind agent sessions.

use crate::cli_agent::CliKind;
use crate::state::background_command;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

/// How much of a child's stderr is kept for error messages.
const STDERR_KEEP: usize = 16 * 1024;

/// A running agent CLI with its pipes taken out of the [`Child`].
pub struct Spawned {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub pid: Option<u32>,
    /// First bytes of stderr, filled in the background.
    pub stderr: Arc<Mutex<String>>,
}

/// Finds the CLI's executable on PATH (and in the usual install dirs).
pub async fn resolve_exe(kind: CliKind) -> Option<PathBuf> {
    let cmds = kind.cmds();
    tokio::task::spawn_blocking(move || crate::pty::resolve_cli_tool_any(cmds)).await.ok().flatten()
}

pub fn spawn(exe: &Path, args: &[String], ws: &Path, remove_env: &[&str]) -> std::io::Result<Spawned> {
    let mut command = background_command(&exe.to_string_lossy());
    command
        .args(args)
        .current_dir(ws)
        .env("PATH", crate::pty::augmented_path_env())
        .env("VIREO_WORKSPACE", ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for k in remove_env {
        command.env_remove(k);
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        drain_stderr(err, stderr.clone());
    }
    Ok(Spawned { child, stdin, stdout, pid, stderr })
}

/// Keeps reading stderr so the child never blocks on a full pipe, remembering the first bytes.
fn drain_stderr(mut err: ChildStderr, keep: Arc<Mutex<String>>) {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match err.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut s) = keep.lock() {
                        if s.len() < STDERR_KEEP {
                            s.push_str(&String::from_utf8_lossy(&buf[..n]));
                        }
                    }
                }
            }
        }
    });
}

/// Ends the CLI and everything it spawned.
#[cfg(windows)]
pub async fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = background_command("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).status().await;
    }
}

#[cfg(not(windows))]
pub async fn kill_tree(_pid: Option<u32>) {}

/// Error text for a CLI that failed to start or stopped early, with its stderr if any.
pub fn failure_text(label: &str, code: Option<i32>, stderr: &Arc<Mutex<String>>) -> String {
    let detail = stderr.lock().map(|s| s.trim().to_string()).unwrap_or_default();
    let code = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    if detail.is_empty() {
        format!("{label} exited unexpectedly (code {code}).")
    } else {
        format!("{label} exited with code {code}: {}", crate::cli_agent::tail_chars(&detail, 800))
    }
}
