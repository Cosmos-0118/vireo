use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

#[derive(Deserialize)]
pub struct PtyQuery {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub cmd: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientPtyMsg {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug, Clone, Serialize)]
pub struct CliToolStatus {
    pub id: String,
    pub label: String,
    pub cmd: String,
    pub installed: bool,
    pub path: Option<String>,
    pub install_hint: String,
}

struct CliToolSpec {
    id: &'static str,
    label: &'static str,
    cmd: &'static str,
    win_install: &'static str,
    unix_install: &'static str,
}

const KNOWN_CLI_TOOLS: &[CliToolSpec] = &[
    CliToolSpec {
        id: "claude",
        label: "Claude",
        cmd: "claude",
        win_install: "irm https://claude.ai/install.ps1 | iex",
        unix_install: "curl -fsSL https://claude.ai/install.sh | bash",
    },
    CliToolSpec {
        id: "codex",
        label: "Codex",
        cmd: "codex",
        win_install: "npm install -g @openai/codex",
        unix_install: "npm install -g @openai/codex",
    },
    CliToolSpec {
        id: "gemini",
        label: "Gemini",
        cmd: "gemini",
        win_install: "npm install -g @google/gemini-cli",
        unix_install: "npm install -g @google/gemini-cli",
    },
    CliToolSpec {
        id: "aider",
        label: "Aider",
        cmd: "aider",
        win_install: "pip install aider-chat",
        unix_install: "pip install aider-chat",
    },
    CliToolSpec {
        id: "opencode",
        label: "OpenCode",
        cmd: "opencode",
        win_install: "npm install -g opencode-ai",
        unix_install: "curl -fsSL https://opencode.ai/install | bash",
    },
];

pub fn augmented_path_env() -> OsString {
    let existing_raw = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&existing_raw).collect();

    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            candidates.push(profile.join(".local").join("bin"));
            candidates.push(profile.join(".opencode").join("bin"));
            candidates.push(profile.join(".bun").join("bin"));
            candidates.push(profile.join(".volta").join("bin"));
            candidates.push(profile.join(".cargo").join("bin"));
            candidates.push(profile.join("scoop").join("shims"));
            candidates.push(profile.join("scoop").join("persist").join("nodejs-lts").join("bin"));
            candidates.push(profile.join("scoop").join("apps").join("nodejs-lts").join("current").join("bin"));
            candidates.push(profile.join("scoop").join("persist").join("nodejs").join("bin"));
            candidates.push(profile.join("scoop").join("apps").join("nodejs").join("current").join("bin"));
        }
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            candidates.push(appdata.join("npm"));
            for py in ["Python313", "Python312", "Python311"] {
                candidates.push(appdata.join("Python").join(py).join("Scripts"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join("pnpm"));
            for py in ["Python313", "Python312", "Python311"] {
                candidates.push(local.join("Programs").join("Python").join(py).join("Scripts"));
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            candidates.push(home.join(".local").join("bin"));
            candidates.push(home.join(".opencode").join("bin"));
            candidates.push(home.join(".bun").join("bin"));
            candidates.push(home.join(".volta").join("bin"));
            candidates.push(home.join(".cargo").join("bin"));
            candidates.push(home.join(".npm-global").join("bin"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
    }

    for cand in candidates {
        if !cand.is_dir() {
            continue;
        }
        let already = dirs.iter().any(|d| {
            if cfg!(windows) {
                d.to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .eq_ignore_ascii_case(cand.to_string_lossy().trim_end_matches(['\\', '/']))
            } else {
                d == &cand
            }
        });
        if !already {
            dirs.push(cand);
        }
    }

    std::env::join_paths(dirs).unwrap_or(existing_raw)
}

fn find_command_in_path(cmd_name: &str, path_env: &OsStr) -> Option<PathBuf> {
    if cmd_name.is_empty() {
        return None;
    }
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            let has_ext = Path::new(cmd_name).extension().is_some();
            if has_ext {
                let p = dir.join(cmd_name);
                if p.is_file() {
                    return Some(p);
                }
            } else {
                for ext in ["exe", "cmd", "bat", "ps1", "com"] {
                    let p = dir.join(format!("{}.{}", cmd_name, ext));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            let p = dir.join(cmd_name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn find_known_tool(first_token: &str) -> Option<&'static CliToolSpec> {
    let lower = first_token.trim_matches(['"', '\'']).to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".ps1"))
        .unwrap_or(&lower);
    KNOWN_CLI_TOOLS.iter().find(|t| t.cmd == stem)
}

pub async fn list_cli_tools() -> Json<Vec<CliToolStatus>> {
    let items = tokio::task::spawn_blocking(|| {
        let path_env = augmented_path_env();
        KNOWN_CLI_TOOLS
            .iter()
            .map(|spec| {
                let found = find_command_in_path(spec.cmd, &path_env);
                CliToolStatus {
                    id: spec.id.to_string(),
                    label: spec.label.to_string(),
                    cmd: spec.cmd.to_string(),
                    installed: found.is_some(),
                    path: found.map(|p| p.to_string_lossy().to_string()),
                    install_hint: if cfg!(windows) {
                        spec.win_install.to_string()
                    } else {
                        spec.unix_install.to_string()
                    },
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    Json(items)
}

#[cfg(windows)]
fn build_windows_shell(
    shell_exe: &str,
    workspace: &Path,
    path_env: &OsStr,
    custom_cmd: Option<&str>,
) -> CommandBuilder {
    let mut c = CommandBuilder::new(shell_exe);
    c.arg("-NoLogo");
    c.arg("-ExecutionPolicy");
    c.arg("Bypass");

    let utf8_init = "[Console]::InputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false)";

    if let Some(custom) = custom_cmd.map(str::trim).filter(|s| !s.is_empty()) {
        c.arg("-NoExit");
        c.arg("-Command");
        let first_tok = custom.split_whitespace().next().unwrap_or("");
        if let Some(spec) = find_known_tool(first_tok) {
            let script = format!(
                "{utf8}; if (Get-Command '{exe}' -ErrorAction SilentlyContinue) {{ {cmd} }} else {{ Write-Host \"`n[Vireo] '{exe}' is not installed or not on PATH.\" -ForegroundColor Yellow; Write-Host \"Install with: {hint}\" -ForegroundColor Cyan; Write-Host \"After installing, click {label} above or run '{exe}' right here.`n\" -ForegroundColor DarkGray }}",
                utf8 = utf8_init,
                exe = spec.cmd,
                cmd = custom,
                hint = spec.win_install,
                label = spec.label,
            );
            c.arg(script);
        } else {
            c.arg(format!("{}; {}", utf8_init, custom));
        }
    } else {
        c.arg("-NoExit");
        c.arg("-Command");
        c.arg(utf8_init);
    }

    c.cwd(workspace);
    c.env("PATH", path_env);
    c.env("TERM", "xterm-256color");
    c.env("COLORTERM", "truecolor");
    c.env("FORCE_COLOR", "1");
    c.env("PYTHONUTF8", "1");
    c.env("PYTHONIOENCODING", "utf-8");
    c
}

#[cfg(not(windows))]
fn build_unix_shell(
    workspace: &Path,
    path_env: &OsStr,
    custom_cmd: Option<&str>,
) -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut c = CommandBuilder::new(&shell);
    if let Some(custom) = custom_cmd.map(str::trim).filter(|s| !s.is_empty()) {
        c.arg("-lc");
        let first_tok = custom.split_whitespace().next().unwrap_or("");
        if let Some(spec) = find_known_tool(first_tok) {
            let script = format!(
                "if command -v {exe} >/dev/null 2>&1; then {cmd}; else printf '\\n\\033[33m[Vireo] '\\''{exe}'\\'' is not installed or not on PATH.\\033[0m\\n\\033[36mInstall with: {hint}\\033[0m\\n\\n'; fi; exec \"{shell}\" -l",
                exe = spec.cmd,
                cmd = custom,
                hint = spec.unix_install,
                shell = shell,
            );
            c.arg(script);
        } else {
            c.arg(format!("{}; exec \"{}\" -l", custom, shell));
        }
    }
    c.cwd(workspace);
    c.env("PATH", path_env);
    c.env("TERM", "xterm-256color");
    c.env("COLORTERM", "truecolor");
    c.env("FORCE_COLOR", "1");
    c.env("PYTHONUTF8", "1");
    c.env("PYTHONIOENCODING", "utf-8");
    c
}

pub async fn ws_pty(
    ws: WebSocketUpgrade,
    Query(q): Query<PtyQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_pty_socket(socket, q, state))
}

async fn handle_pty_socket(socket: WebSocket, q: PtyQuery, state: AppState) {
    let workspace = state.workspace_path().await;
    let cols = q.cols.unwrap_or(110).max(20);
    let rows = q.rows.unwrap_or(28).max(5);

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let (mut tx, _) = socket.split();
            let _ = tx
                .send(Message::Text(format!("\r\n[PTY error: {}]\r\n", e).into()))
                .await;
            return;
        }
    };

    let path_env = augmented_path_env();
    let custom_cmd = q.cmd.as_deref();

    #[cfg(windows)]
    let spawn_result = {
        let has_pwsh = find_command_in_path("pwsh.exe", &path_env).is_some();
        if has_pwsh {
            let pwsh_cmd = build_windows_shell("pwsh.exe", &workspace, &path_env, custom_cmd);
            match pair.slave.spawn_command(pwsh_cmd) {
                Ok(child) => Ok(child),
                Err(_) => {
                    let ps_cmd =
                        build_windows_shell("powershell.exe", &workspace, &path_env, custom_cmd);
                    pair.slave.spawn_command(ps_cmd)
                }
            }
        } else {
            let ps_cmd = build_windows_shell("powershell.exe", &workspace, &path_env, custom_cmd);
            pair.slave.spawn_command(ps_cmd)
        }
    };

    #[cfg(not(windows))]
    let spawn_result = {
        let cmd = build_unix_shell(&workspace, &path_env, custom_cmd);
        pair.slave.spawn_command(cmd)
    };

    let mut child = match spawn_result {
        Ok(c) => c,
        Err(e) => {
            let (mut tx, _) = socket.split();
            let _ = tx
                .send(Message::Text(
                    format!("\r\n[Failed to spawn shell: {}]\r\n", e).into(),
                ))
                .await;
            return;
        }
    };
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(_) => return,
    };
    let master = pair.master;

    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, mut in_rx) = mpsc::channel::<ClientPtyMsg>(256);

    // Blocking reader thread -> async channel (preserves split multi-byte UTF-8 sequences)
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    if !pending.is_empty() {
                        let tail = String::from_utf8_lossy(&pending).to_string();
                        let _ = out_tx.blocking_send(tail);
                    }
                    break;
                }
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    let mut out = String::new();
                    loop {
                        match std::str::from_utf8(&pending) {
                            Ok(valid) => {
                                out.push_str(valid);
                                pending.clear();
                                break;
                            }
                            Err(err) => {
                                let valid_up_to = err.valid_up_to();
                                if valid_up_to > 0 {
                                    out.push_str(
                                        std::str::from_utf8(&pending[..valid_up_to]).unwrap(),
                                    );
                                }
                                if let Some(error_len) = err.error_len() {
                                    out.push('\u{FFFD}');
                                    pending.drain(..valid_up_to + error_len);
                                } else {
                                    pending.drain(..valid_up_to);
                                    break;
                                }
                            }
                        }
                    }
                    if !out.is_empty() && out_tx.blocking_send(out).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Blocking writer / resizer thread
    std::thread::spawn(move || {
        while let Some(msg) = in_rx.blocking_recv() {
            match msg {
                ClientPtyMsg::Input { data } => {
                    if writer.write_all(data.as_bytes()).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                ClientPtyMsg::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows: rows.max(4),
                        cols: cols.max(10),
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }
        let _ = child.kill();
    });

    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut send_task = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Text(chunk.into())).await.is_err() {
                break;
            }
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(txt) => {
                    let s = txt.to_string();
                    let parsed = serde_json::from_str::<ClientPtyMsg>(&s)
                        .unwrap_or(ClientPtyMsg::Input { data: s });
                    if in_tx.send(parsed).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
        },
        _ = &mut recv_task => {
            send_task.abort();
        },
    }
}
