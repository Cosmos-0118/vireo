//! In-app credential prompts for git and ssh, the way VS Code does it.
//!
//! Git commands Vireo starts get `GIT_ASKPASS` / `SSH_ASKPASS` pointing at this executable, plus
//! the local server URL and a per-launch token. When git or ssh needs a secret (an SSH key
//! passphrase for fetch/push or for `gpg.format=ssh` commit signing, an HTTPS username or
//! password, a host-key confirmation), it runs `vireo "<prompt>"`. That helper process posts the
//! prompt to the running app, the UI shows a dialog, and the answer goes back on stdout.
//! Nothing is stored: every prompt is answered fresh (an ssh-agent / Keychain caches it).

use crate::state::{AppState, WorkspaceEvent};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::oneshot;

const URL_ENV: &str = "VIREO_ASKPASS_URL";
const TOKEN_ENV: &str = "VIREO_ASKPASS_TOKEN";
const TOKEN_HEADER: &str = "x-vireo-askpass-token";
/// How long a prompt waits for the user before git is told it was cancelled.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);

struct Config {
    exe: std::path::PathBuf,
    url: String,
    token: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

struct Pending {
    prompt: String,
    secret: bool,
    tx: oneshot::Sender<Option<String>>,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Enables prompts once the local server is listening on `port`. Before this (or if the
/// executable path is unknown), git commands keep the old non-interactive behavior.
pub fn init(port: u16) {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = CONFIG.set(Config {
        exe,
        url: format!("http://127.0.0.1:{port}/api/askpass"),
        token: uuid::Uuid::new_v4().simple().to_string(),
    });
}

/// Points git's and ssh's credential prompts at the in-app dialog.
pub fn apply_env(cmd: &mut tokio::process::Command) {
    let Some(cfg) = CONFIG.get() else { return };
    cmd.env("GIT_ASKPASS", &cfg.exe)
        .env("SSH_ASKPASS", &cfg.exe)
        // OpenSSH 8.4+: use the askpass program even without a DISPLAY or with a terminal.
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(URL_ENV, &cfg.url)
        .env(TOKEN_ENV, &cfg.token);
}

/// Undoes [`apply_env`] for background commands that must never pop up a dialog.
pub fn disable_env(cmd: &mut tokio::process::Command) {
    cmd.env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env_remove(URL_ENV)
        .env_remove(TOKEN_ENV)
        .env("SSH_ASKPASS_REQUIRE", "never");
}

/// Hidden input unless the prompt clearly asks for something that is not a secret.
fn is_secret(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    !(p.contains("username") || p.contains("(yes/no"))
}

// ---------------------------------------------------------------------------------------------
// Helper mode: this executable started by git/ssh as GIT_ASKPASS / SSH_ASKPASS
// ---------------------------------------------------------------------------------------------

/// If this process was started as the askpass helper, answers the prompt and returns the exit
/// code. Call first thing in `main`.
pub fn helper_exit_code() -> Option<i32> {
    let url = std::env::var(URL_ENV).ok()?;
    let token = std::env::var(TOKEN_ENV).ok()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // git and ssh pass exactly one argument, the prompt text.
    if args.len() > 1 {
        return None;
    }
    let prompt = args.into_iter().next().unwrap_or_else(|| "Password:".to_string());
    match request_answer(&url, &token, &prompt) {
        Ok(Some(answer)) => {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
            Some(0)
        }
        Ok(None) => Some(1),
        Err(e) => {
            eprintln!("vireo askpass: {e}");
            Some(1)
        }
    }
}

/// Minimal blocking HTTP/1.1 POST to the app's loopback server (no async runtime needed here).
fn request_answer(url: &str, token: &str, prompt: &str) -> Result<Option<String>, String> {
    let rest = url.strip_prefix("http://").ok_or("bad askpass URL")?;
    let (host, path) = rest.split_once('/').ok_or("bad askpass URL")?;
    let body = serde_json::json!({ "prompt": prompt }).to_string();
    let mut stream =
        std::net::TcpStream::connect(host).map_err(|e| format!("cannot reach Vireo: {e}"))?;
    let request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         {TOKEN_HEADER}: {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("malformed response")?;
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Ok(None);
    }
    let answer: AnswerBody = serde_json::from_str(body).map_err(|e| e.to_string())?;
    Ok(Some(answer.value))
}

#[derive(Serialize, Deserialize)]
struct AnswerBody {
    value: String,
}

// ---------------------------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PromptReq {
    pub prompt: String,
}

/// Removes the prompt (and closes its dialog) however the request ends, including when the
/// helper disconnects because git was killed.
struct PendingGuard {
    id: String,
    events: tokio::sync::broadcast::Sender<WorkspaceEvent>,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = pending().lock() {
            map.remove(&self.id);
        }
        let _ = self.events.send(WorkspaceEvent::AskpassClosed { id: self.id.clone() });
    }
}

/// `POST /api/askpass` from the helper: shows the prompt and waits for the user's answer.
pub async fn prompt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PromptReq>,
) -> Response {
    let Some(cfg) = CONFIG.get() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let authorized = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t == cfg.token);
    if !authorized {
        return StatusCode::FORBIDDEN.into_response();
    }

    let id = uuid::Uuid::new_v4().simple().to_string();
    let prompt = req.prompt.trim().to_string();
    let secret = is_secret(&prompt);
    let (tx, rx) = oneshot::channel();
    if let Ok(mut map) = pending().lock() {
        map.insert(id.clone(), Pending { prompt: prompt.clone(), secret, tx });
    }
    let _guard = PendingGuard { id: id.clone(), events: state.events_tx.clone() };
    let _ = state.events_tx.send(WorkspaceEvent::AskpassPrompt { id, prompt, secret });
    if let Some(win) = state.main_window.get() {
        let _ = win.unminimize();
        let _ = win.set_focus();
    }

    match tokio::time::timeout(ANSWER_TIMEOUT, rx).await {
        Ok(Ok(Some(value))) => Json(AnswerBody { value }).into_response(),
        _ => StatusCode::GONE.into_response(),
    }
}

#[derive(Serialize)]
pub struct PendingPrompt {
    pub id: String,
    pub prompt: String,
    pub secret: bool,
}

/// `GET /api/askpass/pending`: prompts still open (for a UI that (re)connected late).
pub async fn list_pending() -> Json<Vec<PendingPrompt>> {
    let list = pending()
        .lock()
        .map(|map| {
            map.iter()
                .map(|(id, p)| PendingPrompt { id: id.clone(), prompt: p.prompt.clone(), secret: p.secret })
                .collect()
        })
        .unwrap_or_default();
    Json(list)
}

#[derive(Deserialize)]
pub struct AnswerReq {
    pub id: String,
    /// `None` cancels the prompt.
    pub value: Option<String>,
}

/// `POST /api/askpass/answer` from the UI.
pub async fn answer(Json(req): Json<AnswerReq>) -> StatusCode {
    let entry = pending().lock().ok().and_then(|mut map| map.remove(&req.id));
    match entry {
        Some(p) => {
            let _ = p.tx.send(req.value);
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_detection() {
        assert!(is_secret("Enter passphrase for \"/Users/me/.ssh/id_ed25519\":"));
        assert!(is_secret("Password for 'https://me@github.com': "));
        assert!(!is_secret("Username for 'https://github.com': "));
        assert!(!is_secret(
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?"
        ));
    }
}
