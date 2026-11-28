use crate::state::AppState;
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};

/// Output kept per session so a reconnecting client can rebuild its screen.
const REPLAY_CAP: usize = 2 * 1024 * 1024;
/// Extra bytes kept beyond `REPLAY_CAP` before the ring is trimmed (amortizes the trim copy).
const REPLAY_SLACK: usize = 256 * 1024;
/// Largest single WebSocket output frame.
const MAX_FRAME: usize = 64 * 1024;
/// Bytes a client may have in flight (sent but not yet written by xterm) before we pause.
const ACK_WINDOW: u64 = 512 * 1024;
/// The PTY reader pauses when the slowest attached client is this far behind.
const FLOOD_LIMIT: u64 = 1536 * 1024;
/// Most detached (no client attached) sessions kept alive at once.
const MAX_DETACHED: usize = 8;
/// Sessions with no attached client for this long are killed.
const DETACHED_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Deserialize)]
pub struct PtyQuery {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub cmd: Option<String>,
    /// Stable client-chosen session id; reconnecting with the same id re-attaches to the shell.
    pub sid: Option<String>,
    /// Working directory relative to the workspace root (tasks run inside sub-packages).
    pub cwd: Option<String>,
    /// JSON object of extra environment variables for the spawned shell.
    pub env: Option<String>,
}

/// Per-session spawn options beyond the shell command: sub-directory and environment.
#[derive(Default, Clone)]
struct SpawnExtra {
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
}

impl SpawnExtra {
    fn apply(&self, c: &mut CommandBuilder) {
        if let Some(dir) = &self.cwd {
            c.cwd(dir);
        }
        for (k, v) in &self.env {
            c.env(k, v);
        }
    }

    fn from_query(workspace: &Path, q: &PtyQuery) -> Self {
        let cwd = q
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != ".")
            .and_then(|rel| crate::state::resolve_safe_path(workspace, rel).ok())
            .filter(|p| p.is_dir());
        let env = q
            .env
            .as_deref()
            .and_then(|raw| serde_json::from_str::<std::collections::HashMap<String, String>>(raw).ok())
            .map(|m| {
                m.into_iter()
                    .filter(|(k, _)| {
                        !k.is_empty()
                            && k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { cwd, env }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientPtyMsg {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    /// Cumulative bytes of output this connection has finished writing into xterm.
    Ack { bytes: u64 },
    Kill,
}

#[derive(Deserialize)]
pub struct KillQuery {
    pub sid: String,
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
    alt_cmds: &'static [&'static str],
    win_install: &'static str,
    unix_install: &'static str,
}

impl CliToolSpec {
    fn all_cmds(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.cmd).chain(self.alt_cmds.iter().copied())
    }
}

const KNOWN_CLI_TOOLS: &[CliToolSpec] = &[
    CliToolSpec {
        id: "antigravity",
        label: "Antigravity",
        cmd: "agy",
        alt_cmds: &[],
        win_install: "irm https://antigravity.google/cli/install.ps1 | iex",
        unix_install: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    },
    CliToolSpec {
        id: "claude",
        label: "Claude Code",
        cmd: "claude",
        alt_cmds: &[],
        win_install: "irm https://claude.ai/install.ps1 | iex",
        unix_install: "curl -fsSL https://claude.ai/install.sh | bash",
    },
    CliToolSpec {
        id: "codex",
        label: "Codex",
        cmd: "codex",
        alt_cmds: &[],
        win_install: "npm install -g @openai/codex",
        unix_install: "npm install -g @openai/codex",
    },
    CliToolSpec {
        id: "gemini",
        label: "Gemini CLI",
        cmd: "gemini",
        alt_cmds: &[],
        win_install: "npm install -g @google/gemini-cli",
        unix_install: "npm install -g @google/gemini-cli",
    },
    CliToolSpec {
        id: "opencode",
        label: "OpenCode",
        cmd: "opencode",
        alt_cmds: &[],
        win_install: "npm install -g opencode-ai",
        unix_install: "curl -fsSL https://opencode.ai/install | bash",
    },
    CliToolSpec {
        id: "cursor",
        label: "Cursor CLI",
        cmd: "cursor-agent",
        alt_cmds: &["agent"],
        win_install: "irm 'https://cursor.com/install?win32=true' | iex",
        unix_install: "curl https://cursor.com/install -fsS | bash",
    },
    CliToolSpec {
        id: "copilot",
        label: "GitHub Copilot",
        cmd: "copilot",
        alt_cmds: &[],
        win_install: "winget install GitHub.Copilot",
        unix_install: "npm install -g @github/copilot",
    },
    CliToolSpec {
        id: "amp",
        label: "Amp",
        cmd: "amp",
        alt_cmds: &[],
        win_install: "npm install -g @ampcode/cli",
        unix_install: "curl -fsSL https://ampcode.com/install.sh | bash",
    },
    CliToolSpec {
        id: "qwen",
        label: "Qwen Code",
        cmd: "qwen",
        alt_cmds: &[],
        win_install: "npm install -g @qwen-code/qwen-code@latest",
        unix_install: "npm install -g @qwen-code/qwen-code@latest",
    },
    CliToolSpec {
        id: "aider",
        label: "Aider",
        cmd: "aider",
        alt_cmds: &[],
        win_install: "pip install aider-chat",
        unix_install: "pip install aider-chat",
    },
    CliToolSpec {
        id: "cline",
        label: "Cline",
        cmd: "cline",
        alt_cmds: &[],
        win_install: "npm install -g cline",
        unix_install: "npm install -g cline",
    },
    CliToolSpec {
        id: "kilo",
        label: "Kilo Code",
        cmd: "kilo",
        alt_cmds: &["kilocode"],
        win_install: "npm install -g @kilocode/cli",
        unix_install: "npm install -g @kilocode/cli",
    },
    CliToolSpec {
        id: "goose",
        label: "Goose",
        cmd: "goose",
        alt_cmds: &[],
        win_install: "irm https://github.com/aaif-goose/goose/releases/download/stable/download_cli.ps1 | iex",
        unix_install: "curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | bash",
    },
    CliToolSpec {
        id: "kiro",
        label: "Kiro",
        cmd: "kiro-cli",
        alt_cmds: &[],
        win_install: "curl -fsSL https://cli.kiro.dev/install | bash",
        unix_install: "curl -fsSL https://cli.kiro.dev/install | bash",
    },
];

const PATH_ENV_TTL: Duration = Duration::from_secs(60);

static PATH_ENV_CACHE: Mutex<Option<(Instant, OsString)>> = Mutex::new(None);
static PATH_COMPUTE_LOCK: Mutex<()> = Mutex::new(());
static CLI_TOOLS_CACHE: Mutex<Option<(Instant, Vec<CliToolStatus>)>> = Mutex::new(None);
static CLI_TOOLS_COMPUTE_LOCK: Mutex<()> = Mutex::new(());

/// PATH extended with the usual CLI-tool install locations. Cached for `PATH_ENV_TTL` with
/// single-flight coalescing so concurrent startup callers share one scan.
pub fn augmented_path_env() -> OsString {
    if let Some((at, p)) = PATH_ENV_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < PATH_ENV_TTL {
            return p.clone();
        }
    }
    let _guard = PATH_COMPUTE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, p)) = PATH_ENV_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < PATH_ENV_TTL {
            return p.clone();
        }
    }
    let p = compute_augmented_path_env();
    *PATH_ENV_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), p.clone()));
    p
}

/// Drop every cached PATH / CLI-tool lookup so the next call rescans the disk.
pub fn invalidate_path_caches() {
    *PATH_ENV_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CLI_TOOLS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn has_fallback_dirs(cmd: &str) -> bool {
    matches!(cmd, "claude" | "codex" | "agy" | "cursor-agent" | "agent" | "copilot")
}

fn compute_augmented_path_env() -> OsString {
    let existing_raw = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Apps opened from Finder/Dock get launchd's bare PATH. Put the user's login-shell PATH
    // first so git/rg/gh/agents resolve the same way they do in their terminal.
    #[cfg(target_os = "macos")]
    for d in login_shell_path() {
        push_unique_dir(&mut dirs, d);
    }
    for d in std::env::split_paths(&existing_raw) {
        push_unique_dir(&mut dirs, d);
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            candidates.push(profile.join(".local").join("bin"));
            candidates.push(profile.join(".gemini").join("antigravity").join("bin"));
            candidates.push(profile.join(".opencode").join("bin"));
            candidates.push(profile.join(".amp").join("bin"));
            candidates.push(profile.join(".kiro").join("bin"));
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
            candidates.push(appdata.join("Antigravity").join("bin"));
            candidates.push(appdata.join("npm"));
            for py in ["Python313", "Python312", "Python311"] {
                candidates.push(appdata.join("Python").join(py).join("Scripts"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join("Microsoft").join("WinGet").join("Links"));
            candidates.push(local.join("agy").join("bin"));
            candidates.push(local.join("cursor-agent"));
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
            candidates.push(home.join(".gemini").join("antigravity").join("bin"));
            candidates.push(home.join(".opencode").join("bin"));
            candidates.push(home.join(".amp").join("bin"));
            candidates.push(home.join(".kiro").join("bin"));
            candidates.push(home.join(".bun").join("bin"));
            candidates.push(home.join(".volta").join("bin"));
            candidates.push(home.join(".cargo").join("bin"));
            candidates.push(home.join(".npm-global").join("bin"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
    }

    for cand in candidates {
        if cand.is_dir() {
            push_unique_dir(&mut dirs, cand);
        }
    }

    // Agent CLIs that ship inside desktop apps / IDE extensions live in version- or
    // hash-named folders that are never on PATH. Only probe the few tools that actually have
    // bundle fallback locations instead of scanning all 14 tools across every PATH dir.
    for spec in KNOWN_CLI_TOOLS {
        if !spec.all_cmds().any(has_fallback_dirs) {
            continue;
        }
        if spec.all_cmds().any(|c| find_in_dirs(c, dirs.iter().cloned()).is_some()) {
            continue;
        }
        for c in spec.all_cmds().filter(|c| has_fallback_dirs(c)) {
            if let Some(found) = find_in_dirs(c, fallback_tool_dirs(c)) {
                if let Some(parent) = found.parent() {
                    push_unique_dir(&mut dirs, parent.to_path_buf());
                }
                break;
            }
        }
    }

    // `vx` (Vireo's token-lean code tools) ships next to the main executable.
    if let Some(dir) = vx_dir() {
        push_unique_dir(&mut dirs, dir);
    }

    std::env::join_paths(dirs).unwrap_or(existing_raw)
}

/// The PATH a login shell sets up (Homebrew, nvm, pyenv, ... from ~/.zprofile / ~/.zshrc), read
/// once per launch. Runs on the startup PATH thread, never on the UI path; gives up after 3 s so a
/// slow or interactive shell config cannot stall tool lookups.
#[cfg(target_os = "macos")]
fn login_shell_path() -> Vec<PathBuf> {
    static CACHE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            const MARK: &str = "__VIREO_PATH__";
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
            let Ok(mut child) = std::process::Command::new(&shell)
                .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            else {
                return Vec::new();
            };
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Vec::new();
                    }
                }
            }
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = stdout.read_to_string(&mut out);
            }
            out.split(MARK)
                .nth(1)
                .map(|p| std::env::split_paths(p).filter(|d| d.is_absolute()).collect())
                .unwrap_or_default()
        })
        .clone()
}

#[derive(Serialize)]
pub struct ShellInfo {
    /// Display name for a plain terminal tab ("zsh", "bash", "pwsh").
    pub name: String,
}

pub fn default_shell_name() -> String {
    // Short executable name, matching what `spawn` actually starts: pwsh (PowerShell 7) when it
    // is on PATH, otherwise Windows PowerShell. Cached: the PATH scan only needs to run once.
    #[cfg(windows)]
    {
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        NAME.get_or_init(|| {
            let pwsh = find_command_in_path("pwsh.exe", &augmented_path_env()).is_some();
            if pwsh { "pwsh" } else { "powershell" }.to_string()
        })
        .clone()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| Path::new(&s).file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "bash".to_string())
    }
}

/// `GET /api/terminal/shell`: what a new terminal tab runs, for its tab title.
pub async fn shell_info() -> Json<ShellInfo> {
    Json(ShellInfo {
        name: default_shell_name(),
    })
}

/// Directory holding the `vx` helper binary (next to this executable), if it was built.
pub fn vx_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let name = if cfg!(windows) { "vx.exe" } else { "vx" };
    dir.join(name).is_file().then_some(dir)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/']))
    } else {
        a == b
    }
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, cand: PathBuf) {
    if !dirs.iter().any(|d| same_dir(d, &cand)) {
        dirs.push(cand);
    }
}

/// Existing child directories of `dir` (optionally filtered by lowercase name prefix before
/// calling `metadata()`), most recently modified first.
fn subdirs_newest_filtered(dir: &Path, prefix: Option<&str>) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            if let Some(pref) = prefix {
                let name = e.file_name();
                if !name.to_string_lossy().to_ascii_lowercase().starts_with(pref) {
                    return false;
                }
            }
            e.file_type().map(|t| t.is_dir()).unwrap_or(false)
        })
        .map(|e| {
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (mtime, e.path())
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

fn subdirs_newest(dir: &Path) -> Vec<PathBuf> {
    subdirs_newest_filtered(dir, None)
}

/// Directories to probe, newest first, when `cmd` is not reachable through PATH.
fn fallback_tool_dirs(cmd: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from);

    #[cfg(windows)]
    {
        let profile = env_dir("USERPROFILE");
        let local = env_dir("LOCALAPPDATA");
        let roaming = env_dir("APPDATA");

        if let Some(local) = &local {
            out.push(local.join("Microsoft").join("WinGet").join("Links"));
        }
        match cmd {
            "claude" => {
                // Claude desktop bundles the Claude Code CLI: <root>/claude-code/<version>/<hash>/claude.exe
                let mut roots: Vec<PathBuf> = Vec::new();
                if let Some(local) = &local {
                    for data in subdirs_newest(&local.join("Claude-Data")) {
                        roots.push(data.join("claude-code"));
                    }
                }
                if let Some(roaming) = &roaming {
                    roots.push(roaming.join("Claude").join("claude-code"));
                }
                for root in roots {
                    for ver in subdirs_newest(&root) {
                        out.push(ver.clone());
                        out.extend(subdirs_newest(&ver));
                    }
                }
                if let Some(profile) = &profile {
                    out.push(profile.join(".claude").join("local"));
                }
                out.extend(extension_dirs(profile.as_deref(), "anthropic.claude-code-", &["resources", "native-binary"]));
            }
            "codex" => {
                // Codex desktop keeps the CLI in a hash-named folder: OpenAI/Codex/bin/<hash>/codex.exe
                if let Some(local) = &local {
                    out.extend(subdirs_newest(&local.join("OpenAI").join("Codex").join("bin")));
                }
                for bin in extension_dirs(profile.as_deref(), "openai.chatgpt-", &["bin"]) {
                    out.extend(subdirs_newest(&bin));
                    out.push(bin);
                }
            }
            "agy" => {
                if let Some(local) = &local {
                    out.push(local.join("agy").join("bin"));
                }
                if let Some(roaming) = &roaming {
                    out.push(roaming.join("Antigravity").join("bin"));
                }
                if let Some(profile) = &profile {
                    out.push(profile.join(".gemini").join("antigravity").join("bin"));
                }
            }
            "cursor-agent" | "agent" => {
                if let Some(local) = &local {
                    out.push(local.join("cursor-agent"));
                }
                if let Some(profile) = &profile {
                    out.push(profile.join(".local").join("bin"));
                }
            }
            "copilot" => {
                if let Some(local) = &local {
                    for pkg in subdirs_newest_filtered(
                        &local.join("Microsoft").join("WinGet").join("Packages"),
                        Some("github.copilot"),
                    ) {
                        out.push(pkg.clone());
                        out.extend(subdirs_newest(&pkg));
                    }
                }
            }
            _ => {}
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = env_dir("HOME") {
            match cmd {
                "claude" => out.push(home.join(".claude").join("local")),
                "agy" => {
                    out.push(home.join(".gemini").join("antigravity").join("bin"));
                    out.push(home.join(".local").join("bin"));
                }
                _ => {}
            }
        }
    }

    out.into_iter().filter(|d| d.is_dir()).collect()
}

/// `<home>/.vscode|.cursor/extensions/<prefix>*/<sub...>` directories, newest extension first.
#[cfg(windows)]
fn extension_dirs(profile: Option<&Path>, prefix: &str, sub: &[&str]) -> Vec<PathBuf> {
    let Some(profile) = profile else { return Vec::new() };
    let mut out = Vec::new();
    for ide in [".vscode", ".cursor", ".vscode-insiders", ".windsurf"] {
        for ext in subdirs_newest_filtered(&profile.join(ide).join("extensions"), Some(prefix)) {
            let mut p = ext;
            for s in sub {
                p = p.join(s);
            }
            out.push(p);
        }
    }
    out
}

/// Resolve the first matching binary from a list of candidate command names.
pub fn resolve_cli_tool_any(cmds: &[&str]) -> Option<PathBuf> {
    let path_env = augmented_path_env();
    cmds.iter().find_map(|c| find_command_in_path(c, &path_env))
}

fn find_command_in_path(cmd_name: &str, path_env: &OsStr) -> Option<PathBuf> {
    find_in_dirs(cmd_name, std::env::split_paths(path_env))
}

fn find_in_dirs(cmd_name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    if cmd_name.is_empty() {
        return None;
    }
    for dir in dirs {
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

fn find_known_tool(first_token: &str) -> Option<(&'static CliToolSpec, &'static str)> {
    let lower = first_token.trim_matches(['"', '\'']).to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".ps1"))
        .unwrap_or(&lower);
    for spec in KNOWN_CLI_TOOLS {
        if let Some(matched) = spec.all_cmds().find(|c| *c == stem) {
            return Some((spec, matched));
        }
    }
    None
}

#[derive(Deserialize)]
pub struct CliToolsQuery {
    #[serde(default)]
    refresh: Option<String>,
}

pub async fn list_cli_tools(Query(q): Query<CliToolsQuery>) -> Json<Vec<CliToolStatus>> {
    let refresh = matches!(q.refresh.as_deref(), Some("1" | "true"));
    if refresh {
        invalidate_path_caches();
    } else if let Some((at, items)) = CLI_TOOLS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < PATH_ENV_TTL {
            return Json(items.clone());
        }
    }
    let items = tokio::task::spawn_blocking(move || {
        let _guard = CLI_TOOLS_COMPUTE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !refresh {
            if let Some((at, items)) = CLI_TOOLS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                if at.elapsed() < PATH_ENV_TTL {
                    return items.clone();
                }
            }
        }
        let path_env = augmented_path_env();
        let computed = KNOWN_CLI_TOOLS
            .iter()
            .map(|spec| {
                let resolved = spec
                    .all_cmds()
                    .find_map(|c| find_command_in_path(c, &path_env).map(|p| (c, p)));
                let (cmd, path) = match resolved {
                    Some((c, p)) => (c, Some(p)),
                    None => (spec.cmd, None),
                };
                CliToolStatus {
                    id: spec.id.to_string(),
                    label: spec.label.to_string(),
                    cmd: cmd.to_string(),
                    installed: path.is_some(),
                    path: path.map(|p| p.to_string_lossy().to_string()),
                    install_hint: if cfg!(windows) {
                        spec.win_install.to_string()
                    } else {
                        spec.unix_install.to_string()
                    },
                }
            })
            .collect::<Vec<_>>();
        if !computed.is_empty() {
            *CLI_TOOLS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), computed.clone()));
        }
        computed
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
        if let Some((spec, matched_cmd)) = find_known_tool(first_tok) {
            let script = format!(
                "{utf8}; if (Get-Command '{exe}' -ErrorAction SilentlyContinue) {{ {cmd} }} else {{ Write-Host \"`n[Vireo] '{exe}' is not installed or not on PATH.\" -ForegroundColor Yellow; Write-Host \"Install with: {hint}\" -ForegroundColor Cyan; Write-Host \"After installing, click {label} above or run '{exe}' right here.`n\" -ForegroundColor DarkGray }}",
                utf8 = utf8_init,
                exe = matched_cmd,
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
    c.env("VIREO_WORKSPACE", workspace);
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
        if let Some((spec, matched_cmd)) = find_known_tool(first_tok) {
            let script = format!(
                "if command -v {exe} >/dev/null 2>&1; then {cmd}; else printf '\\n\\033[33m[Vireo] '\\''{exe}'\\'' is not installed or not on PATH.\\033[0m\\n\\033[36mInstall with: {hint}\\033[0m\\n\\n'; fi; exec \"{shell}\" -l",
                exe = matched_cmd,
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
    c.env("VIREO_WORKSPACE", workspace);
    c.env("TERM", "xterm-256color");
    c.env("COLORTERM", "truecolor");
    c.env("FORCE_COLOR", "1");
    c.env("PYTHONUTF8", "1");
    c.env("PYTHONIOENCODING", "utf-8");
    c
}

// ---------------------------------------------------------------------------
// Persistent PTY sessions
//
// A shell lives in a `PtySession` keyed by a client-chosen `sid`. WebSockets attach to and
// detach from it; the process only dies on an explicit kill, when it exits, or when it has
// been detached for `DETACHED_TTL`. Output is kept in a bounded ring so a re-attaching client
// can rebuild its screen, and delivery is flow-controlled with client ACKs.
// ---------------------------------------------------------------------------

/// Set once app exit begins; no new shell is spawned after that.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// How long a shell gets to exit by itself after its PTY is closed, before it is terminated.
const SHELL_EXIT_GRACE: Duration = Duration::from_millis(1200);

enum PtyCmd {
    Input(Vec<u8>),
    Resize(u16, u16),
    Kill,
    /// The child process ended (sent by its waiter thread).
    Exited,
}

struct Ring {
    data: Vec<u8>,
    /// Absolute stream offset of `data[0]`.
    base: u64,
}

impl Ring {
    fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }

    fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > REPLAY_CAP + REPLAY_SLACK {
            let drop = self.data.len() - REPLAY_CAP;
            self.data.drain(..drop);
            self.base += drop as u64;
            // `drain` keeps the old capacity: release it so a busy PTY does not pin the peak.
            self.data.shrink_to(REPLAY_CAP + REPLAY_SLACK * 2);
        }
    }

    /// Up to `max` bytes starting at absolute offset `from` (clamped to what is still held).
    fn read(&self, from: u64, max: usize) -> (Vec<u8>, u64) {
        let from = from.max(self.base);
        let start = (from - self.base) as usize;
        if start >= self.data.len() {
            return (Vec::new(), from);
        }
        let end = (start + max).min(self.data.len());
        (self.data[start..end].to_vec(), self.base + end as u64)
    }

    /// Where a new subscriber should start replaying. If the ring has wrapped, begin after
    /// the next newline so the replay does not open in the middle of an escape sequence.
    fn replay_start(&self) -> u64 {
        if self.base == 0 {
            return 0;
        }
        let scan = self.data.len().min(4096);
        match self.data[..scan].iter().position(|b| *b == b'\n') {
            Some(i) => self.base + i as u64 + 1,
            None => self.base,
        }
    }
}

struct PtySession {
    id: String,
    cmd_tx: std_mpsc::Sender<PtyCmd>,
    ring: Mutex<Ring>,
    /// Bumped (to the ring end) whenever output arrives or the process exits.
    updates: watch::Sender<u64>,
    exited: AtomicBool,
    /// Set by the control thread once the shell is gone and the PTY is closed.
    closed: Arc<AtomicBool>,
    /// Read cursor of every attached client (conn id -> absolute offset).
    cursors: Mutex<HashMap<u64, Arc<AtomicU64>>>,
    detached_since: Mutex<Option<Instant>>,
}

impl PtySession {
    fn send(&self, cmd: PtyCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    fn kill(&self) {
        self.send(PtyCmd::Kill);
    }

    /// How far the slowest attached client trails the newest output.
    fn slowest_lag(&self) -> u64 {
        let end = self.ring.lock().unwrap().end();
        let cursors = self.cursors.lock().unwrap();
        cursors
            .values()
            .map(|c| end.saturating_sub(c.load(Ordering::Relaxed)))
            .max()
            .unwrap_or(0)
    }
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<PtySession>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<PtySession>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// App exit: ends every shell and waits (up to `timeout`) for each to close cleanly, so the
/// shells are not terminated mid-write when the process goes away.
pub fn shutdown_all(timeout: Duration) {
    // From here on nothing may start a shell: a process born while the app is exiting would be
    // torn down mid-startup.
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let all: Vec<Arc<PtySession>> = match sessions().lock() {
        Ok(mut map) => map.drain().map(|(_, s)| s).collect(),
        Err(_) => return,
    };
    for s in &all {
        s.kill();
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !all.iter().all(|s| s.closed.load(Ordering::Acquire)) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn remove_session(session: &Arc<PtySession>) {
    let mut map = sessions().lock().unwrap();
    if map.get(&session.id).is_some_and(|s| Arc::ptr_eq(s, session)) {
        map.remove(&session.id);
    }
}

fn ensure_reaper() {
    static START: Once = Once::new();
    START.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let stale: Vec<Arc<PtySession>> = {
                    let mut map = sessions().lock().unwrap();
                    let mut ids: Vec<String> = map
                        .iter()
                        .filter(|(_, s)| {
                            s.cursors.lock().unwrap().is_empty()
                                && s.detached_since
                                    .lock()
                                    .unwrap()
                                    .is_some_and(|t| t.elapsed() > DETACHED_TTL)
                        })
                        .map(|(id, _)| id.clone())
                        .collect();
                    // Also cap how many detached shells may linger (each pins a shell process
                    // and a replay buffer): drop the longest-detached beyond the cap.
                    let mut detached: Vec<(Instant, String)> = map
                        .iter()
                        .filter(|(id, s)| !ids.contains(id) && s.cursors.lock().unwrap().is_empty())
                        .filter_map(|(id, s)| {
                            s.detached_since.lock().unwrap().map(|t| (t, id.clone()))
                        })
                        .collect();
                    if detached.len() > MAX_DETACHED {
                        detached.sort();
                        let excess = detached.len() - MAX_DETACHED;
                        ids.extend(detached.into_iter().take(excess).map(|(_, id)| id));
                    }
                    ids.iter().filter_map(|id| map.remove(id)).collect()
                };
                for s in stale {
                    s.kill();
                }
            }
        });
    });
}

fn spawn_session(
    id: &str,
    cols: u16,
    rows: u16,
    workspace: &Path,
    custom_cmd: Option<&str>,
    extra: &SpawnExtra,
) -> Result<Arc<PtySession>, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("PTY error: {}", e))?;

    let path_env = augmented_path_env();

    #[cfg(windows)]
    let spawn_result = {
        let has_pwsh = find_command_in_path("pwsh.exe", &path_env).is_some();
        if has_pwsh {
            let mut pwsh_cmd = build_windows_shell("pwsh.exe", workspace, &path_env, custom_cmd);
            extra.apply(&mut pwsh_cmd);
            match pair.slave.spawn_command(pwsh_cmd) {
                Ok(child) => Ok(child),
                Err(_) => {
                    let mut ps_cmd =
                        build_windows_shell("powershell.exe", workspace, &path_env, custom_cmd);
                    extra.apply(&mut ps_cmd);
                    pair.slave.spawn_command(ps_cmd)
                }
            }
        } else {
            let mut ps_cmd = build_windows_shell("powershell.exe", workspace, &path_env, custom_cmd);
            extra.apply(&mut ps_cmd);
            pair.slave.spawn_command(ps_cmd)
        }
    };

    #[cfg(not(windows))]
    let spawn_result = {
        let mut cmd = build_unix_shell(workspace, &path_env, custom_cmd);
        extra.apply(&mut cmd);
        pair.slave.spawn_command(cmd)
    };

    let child = spawn_result.map_err(|e| format!("Failed to spawn shell: {}", e))?;
    drop(pair.slave);
    // The shell and everything it launches, so teardown can end them together.
    #[cfg(windows)]
    let tree = child.process_id().and_then(crate::jobobject::ProcessTree::adopt);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("PTY reader error: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("PTY writer error: {}", e))?;
    let master = pair.master;

    let (cmd_tx, cmd_rx) = std_mpsc::channel::<PtyCmd>();
    let (updates, _) = watch::channel(0u64);
    let session = Arc::new(PtySession {
        id: id.to_string(),
        cmd_tx,
        ring: Mutex::new(Ring {
            data: Vec::new(),
            base: 0,
        }),
        updates,
        exited: AtomicBool::new(false),
        closed: Arc::new(AtomicBool::new(false)),
        cursors: Mutex::new(HashMap::new()),
        detached_since: Mutex::new(Some(Instant::now())),
    });

    // Reader: PTY -> ring. Pauses (back-pressuring the child) while the slowest attached
    // client is too far behind, so a runaway `cat` cannot flood the browser.
    {
        let session = session.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                while !session.exited.load(Ordering::SeqCst) && session.slowest_lag() > FLOOD_LIMIT
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let end = {
                            let mut ring = session.ring.lock().unwrap();
                            ring.push(&buf[..n]);
                            ring.end()
                        };
                        session.updates.send_replace(end);
                    }
                }
            }
            session.exited.store(true, Ordering::SeqCst);
            remove_session(&session);
            session.kill(); // make sure the control thread reaps the child
            let end = session.ring.lock().unwrap().end();
            session.updates.send_replace(end);
        });
    }

    // Control thread: input, resize, kill, and child-exit detection. ConPTY does not close
    // the output pipe when the child exits, so poll for it and then drop the master.
    let mut killer = child.clone_killer();
    let child_done = Arc::new(AtomicBool::new(false));
    {
        // Block on the child's exit instead of polling `try_wait` every 400 ms.
        let exit_tx = session.cmd_tx.clone();
        let done = child_done.clone();
        let mut child = child;
        std::thread::spawn(move || {
            let _ = child.wait();
            done.store(true, Ordering::Release);
            let _ = exit_tx.send(PtyCmd::Exited);
        });
    }
    let closed = session.closed.clone();
    std::thread::spawn(move || {
        let mut last_size = (cols, rows);
        loop {
            match cmd_rx.recv() {
                Ok(PtyCmd::Input(data)) => {
                    if writer.write_all(&data).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                Ok(PtyCmd::Resize(c, r)) => {
                    let size = (c.max(10), r.max(4));
                    // Redundant resizes make ConPTY redraw the prompt; skip them.
                    if size != last_size {
                        last_size = size;
                        let _ = master.resize(PtySize {
                            rows: size.1,
                            cols: size.0,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }
                Ok(PtyCmd::Exited) => {
                    // let ConPTY flush the final output before closing it
                    std::thread::sleep(Duration::from_millis(200));
                    break;
                }
                Ok(PtyCmd::Kill) | Err(_) => break,
            }
        }
        // Close the pseudoconsole first: ConPTY then sends its processes a close event and they
        // exit on their own (history saved, no half-written output). ClosePseudoConsole can
        // block until every console client has left, so it runs on a helper thread while this
        // one enforces the grace period.
        drop(writer);
        let (gone_tx, gone_rx) = std_mpsc::channel::<()>();
        std::thread::spawn(move || {
            drop(master);
            let _ = gone_tx.send(());
        });
        let console_gone = gone_rx.recv_timeout(SHELL_EXIT_GRACE).is_ok();
        if !console_gone || !child_done.load(Ordering::Acquire) {
            // Still running: end the shell and everything it started while their console is
            // still alive. If the console host disappears under a process that is still
            // handling its close event (e.g. the app exits first), Windows fails that process
            // with 0xc0000142 "The application was unable to start correctly".
            #[cfg(windows)]
            if let Some(tree) = &tree {
                tree.terminate();
            }
            let _ = killer.kill();
            // With the clients gone, closing the console finishes promptly.
            let _ = gone_rx.recv_timeout(Duration::from_millis(1000));
        }
        closed.store(true, Ordering::Release);
    });

    Ok(session)
}

fn get_or_create(
    sid: &str,
    cols: u16,
    rows: u16,
    workspace: &Path,
    custom_cmd: Option<&str>,
    extra: &SpawnExtra,
) -> Result<Arc<PtySession>, String> {
    if let Some(existing) = sessions().lock().unwrap().get(sid) {
        if !existing.exited.load(Ordering::SeqCst) {
            return Ok(existing.clone());
        }
    }
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return Err("Vireo is shutting down".into());
    }
    let session = spawn_session(sid, cols, rows, workspace, custom_cmd, extra)?;
    let mut map = sessions().lock().unwrap();
    // Exit began while this shell was starting: `shutdown_all` already drained the map and will
    // never see it, so end it here.
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        session.kill();
        return Err("Vireo is shutting down".into());
    }
    if let Some(existing) = map.get(sid) {
        if !existing.exited.load(Ordering::SeqCst) {
            session.kill();
            return Ok(existing.clone());
        }
    }
    map.insert(sid.to_string(), session.clone());
    Ok(session)
}

/// Pre-spawns the default terminal tab's shell in the background so PowerShell / `$SHELL` boots
/// in parallel with WebView2 creation and is ready as soon as the UI connects.
pub fn prewarm_default_shell(workspace: PathBuf) {
    if workspace.as_os_str().is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let sid = format!("term::{}::shell-1::0::", workspace.to_string_lossy());
        let extra = SpawnExtra::default();
        let _ = get_or_create(&sid, 110, 28, &workspace, None, &extra);
    });
}

/// Registers an attached client's cursor and unregisters it on drop.
struct Attachment {
    session: Arc<PtySession>,
    conn_id: u64,
    cursor: Arc<AtomicU64>,
}

impl Attachment {
    fn new(session: Arc<PtySession>, start: u64) -> Self {
        static NEXT_CONN: AtomicU64 = AtomicU64::new(1);
        let conn_id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
        let cursor = Arc::new(AtomicU64::new(start));
        session
            .cursors
            .lock()
            .unwrap()
            .insert(conn_id, cursor.clone());
        *session.detached_since.lock().unwrap() = None;
        Self {
            session,
            conn_id,
            cursor,
        }
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let mut cursors = self.session.cursors.lock().unwrap();
        cursors.remove(&self.conn_id);
        if cursors.is_empty() {
            *self.session.detached_since.lock().unwrap() = Some(Instant::now());
        }
    }
}

pub async fn ws_pty(
    ws: WebSocketUpgrade,
    Query(q): Query<PtyQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_pty_socket(socket, q, state))
}

/// Explicitly end a session (tab closed, agent relaunched, workspace changed).
pub async fn kill_pty(Query(q): Query<KillQuery>) -> StatusCode {
    let session = sessions().lock().unwrap().remove(&q.sid);
    if let Some(s) = session {
        s.kill();
    }
    StatusCode::NO_CONTENT
}

async fn handle_pty_socket(socket: WebSocket, q: PtyQuery, state: AppState) {
    static NEXT_ANON: AtomicUsize = AtomicUsize::new(1);
    ensure_reaper();

    let workspace = state.workspace_path().await;
    let cols = q.cols.unwrap_or(110).max(20);
    let rows = q.rows.unwrap_or(28).max(5);

    // Without a sid the shell is private to this socket and dies with it.
    let ephemeral = q.sid.as_deref().map_or(true, str::is_empty);
    let sid = match q.sid.clone().filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => format!("anon-{}", NEXT_ANON.fetch_add(1, Ordering::Relaxed)),
    };
    let custom_cmd = q.cmd.clone();
    let extra = SpawnExtra::from_query(&workspace, &q);

    let created = tokio::task::spawn_blocking({
        let sid = sid.clone();
        move || get_or_create(&sid, cols, rows, &workspace, custom_cmd.as_deref(), &extra)
    })
    .await;

    let session = match created {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let (mut tx, _) = socket.split();
            let _ = tx
                .send(Message::Text(format!("\r\n[{}]\r\n", e).into()))
                .await;
            let _ = tx.close().await;
            return;
        }
        Err(_) => return,
    };

    let (replay_start, replay_end) = {
        let ring = session.ring.lock().unwrap();
        (ring.replay_start(), ring.end())
    };
    let attachment = Attachment::new(session.clone(), replay_start);
    let cursor = attachment.cursor.clone();
    let mut updates = session.updates.subscribe();

    let acked = Arc::new(AtomicU64::new(0));
    let ack_notify = Arc::new(Notify::new());
    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut send_task = {
        let session = session.clone();
        let acked = acked.clone();
        let ack_notify = ack_notify.clone();
        tokio::spawn(async move {
            let mut pos = replay_start;
            let mut sent_bytes: u64 = 0;
            let mut ready_sent = false;
            loop {
                // Marks the end of the replay so the client can stop muting terminal replies.
                if !ready_sent && pos >= replay_end {
                    ready_sent = true;
                    if ws_tx
                        .send(Message::Text(r#"{"type":"ready"}"#.into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }

                // Flow control: wait for the client to catch up on what is already in flight.
                while sent_bytes.saturating_sub(acked.load(Ordering::Relaxed)) > ACK_WINDOW {
                    let _ =
                        tokio::time::timeout(Duration::from_millis(250), ack_notify.notified())
                            .await;
                }

                // Read `exited` before the ring so output written just before exit is not lost.
                let exited = session.exited.load(Ordering::SeqCst);
                let (chunk, next) = session.ring.lock().unwrap().read(pos, MAX_FRAME);
                if chunk.is_empty() {
                    if exited {
                        break;
                    }
                    if updates.changed().await.is_err() {
                        break;
                    }
                    continue;
                }

                // Output that wrapped out of the ring while we were stalled is simply skipped.
                sent_bytes += chunk.len() as u64;
                pos = next;
                cursor.store(pos, Ordering::Relaxed);
                if ws_tx.send(Message::Binary(chunk.into())).await.is_err() {
                    return;
                }
            }
            let _ = ws_tx
                .send(Message::Close(Some(CloseFrame {
                    code: 1000,
                    reason: "exited".into(),
                })))
                .await;
        })
    };

    let mut recv_task = {
        let session = session.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = ws_rx.next().await {
                match msg {
                    Message::Text(txt) => match serde_json::from_str::<ClientPtyMsg>(&txt) {
                        Ok(ClientPtyMsg::Input { data }) => {
                            session.send(PtyCmd::Input(data.into_bytes()))
                        }
                        Ok(ClientPtyMsg::Resize { cols, rows }) => {
                            session.send(PtyCmd::Resize(cols, rows))
                        }
                        Ok(ClientPtyMsg::Ack { bytes }) => {
                            acked.fetch_max(bytes, Ordering::Relaxed);
                            ack_notify.notify_one();
                        }
                        Ok(ClientPtyMsg::Kill) => session.kill(),
                        Err(_) => session.send(PtyCmd::Input(txt.as_str().as_bytes().to_vec())),
                    },
                    Message::Binary(bytes) => session.send(PtyCmd::Input(bytes.to_vec())),
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
    };

    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
        },
        _ = &mut recv_task => {
            send_task.abort();
        },
    }

    drop(attachment);
    if ephemeral {
        remove_session(&session);
        session.kill();
    }
}
