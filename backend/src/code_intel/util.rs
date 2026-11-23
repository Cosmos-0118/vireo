//! Process execution and workspace path resolution helpers.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

static PATH_PROVIDER: OnceLock<fn() -> OsString> = OnceLock::new();

/// Source of the PATH used when spawning helper tools (rg, ast-grep, build commands). Optional;
/// the provider is expected to cache. Only the main app calls this; the `vx` bin includes this
/// module but never calls it.
#[allow(dead_code)]
pub fn set_path_provider(f: fn() -> OsString) {
    let _ = PATH_PROVIDER.set(f);
}

fn path_env() -> Option<OsString> {
    PATH_PROVIDER.get().map(|f| f())
}

/// Third-party tools shipped next to the executable by the installer / portable ZIP
/// (`tools/git/cmd` for MinGit, `tools/bin` for rg and gh); only the ones present.
fn bundled_tool_dirs() -> Vec<PathBuf> {
    let Some(root) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("tools"))) else {
        return Vec::new();
    };
    [root.join("git").join("cmd"), root.join("bin")].into_iter().filter(|d| d.is_dir()).collect()
}

/// Append the bundled tool directories to this process's PATH so every child process (git, rg,
/// gh, terminals, agent CLIs) can find them. They go last: a copy the user installed keeps
/// precedence, along with its config and credential setup. Call first thing in `main`, before
/// any other thread starts.
pub fn use_bundled_tools() {
    let dirs = bundled_tool_dirs();
    if dirs.is_empty() {
        return;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut all: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let same = |a: &Path, b: &Path| {
        let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
        norm(a) == norm(b)
    };
    let mut changed = false;
    for d in dirs {
        if !all.iter().any(|e| same(e, &d)) {
            all.push(d);
            changed = true;
        }
    }
    if changed {
        if let Ok(joined) = std::env::join_paths(all) {
            std::env::set_var("PATH", joined);
        }
    }
}

#[cfg(windows)]
static RESOLVE_CACHE: Mutex<Option<std::collections::HashMap<String, (Instant, OsString)>>> = Mutex::new(None);

/// Windows: `Command::new("foo")` only tries `foo.exe`, so npm/scoop shims (`foo.cmd`, `foo.bat`)
/// look "not installed". Resolve extension-less names against PATH ourselves.
#[cfg(windows)]
fn resolve_program(program: &str) -> OsString {
    let p = Path::new(program);
    if p.extension().is_some() || p.components().count() > 1 {
        return program.into();
    }
    if let Some((at, hit)) = RESOLVE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(program))
    {
        if at.elapsed() < Duration::from_secs(60) {
            return hit.clone();
        }
    }
    let path = path_env().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
    let mut found: OsString = program.into();
    'outer: for dir in std::env::split_paths(&path) {
        for ext in ["exe", "cmd", "bat"] {
            let cand = dir.join(format!("{program}.{ext}"));
            if cand.is_file() {
                found = cand.into_os_string();
                break 'outer;
            }
        }
    }
    RESOLVE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .insert(program.to_string(), (Instant::now(), found.clone()));
    found
}

pub(crate) fn command(program: &str) -> Command {
    #[cfg(windows)]
    let mut c = Command::new(resolve_program(program));
    #[cfg(not(windows))]
    let mut c = Command::new(program);
    if let Some(p) = path_env() {
        c.env("PATH", p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub(crate) struct RunOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub secs: f64,
}

fn pump<R: Read + Send + 'static>(mut r: R, buf: Arc<Mutex<Vec<u8>>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut b) = buf.lock() {
                if b.len() < 2 * 1024 * 1024 {
                    b.extend_from_slice(&chunk[..n]);
                }
            }
        }
    })
}

pub(crate) fn run_capture(mut cmd: Command, timeout: Duration) -> Result<RunOut, String> {
    let start = Instant::now();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let t1 = child.stdout.take().map(|s| pump(s, out_buf.clone()));
    let t2 = child.stderr.take().map(|s| pump(s, err_buf.clone()));
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if !timed_out {
        for t in [t1, t2].into_iter().flatten() {
            let _ = t.join();
        }
    }
    let take = |b: &Arc<Mutex<Vec<u8>>>| {
        String::from_utf8_lossy(&b.lock().map(|g| g.clone()).unwrap_or_default()).to_string()
    };
    Ok(RunOut {
        code,
        stdout: take(&out_buf),
        stderr: take(&err_buf),
        timed_out,
        secs: start.elapsed().as_secs_f64(),
    })
}

pub(crate) fn shell_command(cmd: &str) -> Command {
    if cfg!(windows) {
        let mut c = command("powershell.exe");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd]);
        c
    } else {
        let mut c = command("sh");
        c.args(["-lc", cmd]);
        c
    }
}

// ---------------------------------------------------------------------------------------------
// Paths & smart monorepo resolution
// ---------------------------------------------------------------------------------------------

pub(crate) fn norm_slashes(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string()
}

pub(crate) fn ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".next" | ".nuxt" | "__pycache__"
            | ".venv" | "venv" | ".turbo" | ".cache" | "coverage" | "obj" | ".gradle" | ".vs" | ".idea"
    )
}

/// Resolve a workspace-relative path (or an absolute path inside the workspace).
pub fn resolve(ws: &Path, p: &str) -> Result<PathBuf, String> {
    let cleaned = p.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    if cleaned.is_empty() {
        return Err("empty path".into());
    }
    let pb = Path::new(&cleaned);
    if pb.is_absolute() {
        let ws_s = norm_slashes(ws);
        let p_s = norm_slashes(pb);
        if p_s.eq_ignore_ascii_case(&ws_s) {
            return Ok(ws.to_path_buf());
        }
        if let Some(head) = p_s.get(..ws_s.len()) {
            if head.eq_ignore_ascii_case(&ws_s) && p_s.as_bytes().get(ws_s.len()) == Some(&b'/') {
                return resolve(ws, &p_s[ws_s.len() + 1..]);
            }
        }
        return Err(format!("path is outside the workspace: {p}"));
    }
    let mut out = ws.to_path_buf();
    for comp in pb.components() {
        match comp {
            std::path::Component::Normal(c) => out.push(c),
            std::path::Component::CurDir => {}
            _ => return Err("path traversal is not allowed".into()),
        }
    }
    Ok(out)
}

pub(crate) fn rel_of(ws: &Path, full: &Path) -> String {
    let w = norm_slashes(ws);
    let f = norm_slashes(full);
    if f.eq_ignore_ascii_case(&w) {
        return ".".to_string();
    }
    if f.len() > w.len() && f[..w.len()].eq_ignore_ascii_case(&w) && f.as_bytes()[w.len()] == b'/' {
        return f[w.len() + 1..].to_string();
    }
    f
}

/// Resolve a path trying `cwd` first (when inside `ws`), then `ws`.
pub(crate) fn resolve_with_cwd(ws: &Path, cwd: Option<&Path>, p: &str) -> Result<PathBuf, String> {
    let cleaned = p.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    if cleaned.is_empty() {
        return Err("empty path".into());
    }
    if Path::new(&cleaned).is_absolute() {
        return resolve(ws, &cleaned);
    }
    if let Some(c) = cwd {
        if let Ok(from_cwd) = resolve(ws, &c.join(&cleaned).to_string_lossy()) {
            if from_cwd.exists() {
                return Ok(from_cwd);
            }
        }
    }
    resolve(ws, &cleaned)
}

/// Top-level non-ignored directories inside the workspace (e.g. `["backend/", "frontend/"]`).
pub(crate) fn top_level_dirs(ws: &Path) -> Vec<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(ws)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.') && !ignored_dir(n))
        .map(|n| format!("{n}/"))
        .collect();
    dirs.sort();
    dirs
}

/// Find workspace-relative files and/or directories matching `raw` by path suffix (e.g.
/// `src/components/BranchBar.tsx` -> `frontend/src/components/BranchBar.tsx`) or basename.
pub(crate) fn find_path_candidates(
    ws: &Path,
    raw: &str,
    want_files: bool,
    want_dirs: bool,
) -> Vec<String> {
    let mut q = raw.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    while let Some(rest) = q.strip_prefix("./") {
        q = rest.to_string();
    }
    let q = q.trim_end_matches('/');
    if q.is_empty() {
        return Vec::new();
    }
    let q_low = q.to_lowercase();
    let suffix = format!("/{q_low}");
    let has_slash = q_low.contains('/');
    let mut exact_suffix = Vec::new();
    let mut basename_hits = Vec::new();
    let mut seen = 0usize;
    let walk = WalkDir::new(ws).sort_by_file_name().into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        e.depth() == 0 || !(name.starts_with('.') || (e.file_type().is_dir() && ignored_dir(&name)))
    });
    for e in walk.flatten() {
        if e.depth() == 0 {
            continue;
        }
        seen += 1;
        if seen > 6000 {
            break;
        }
        let is_dir = e.file_type().is_dir();
        let is_file = e.file_type().is_file();
        if (is_dir && !want_dirs) || (is_file && !want_files) || (!is_dir && !is_file) {
            continue;
        }
        let rel = rel_of(ws, e.path());
        let rel_low = rel.to_lowercase();
        if rel_low == q_low || rel_low.ends_with(&suffix) {
            exact_suffix.push(rel);
        } else if !has_slash {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name == q_low {
                basename_hits.push(rel);
            }
        }
    }
    let mut out = if !exact_suffix.is_empty() { exact_suffix } else { basename_hits };
    out.sort();
    out.dedup();
    out
}
/// Prefix an op's output with a `(resolved ...)` path note, when there is one.
pub(crate) fn with_note(note: Option<String>, out: String) -> String {
    match note {
        Some(n) => format!("{n}\n{out}"),
        None => out,
    }
}


/// Resolve an existing file (or dir) for any operation, read or write: the exact path (cwd, then
/// workspace root) wins; otherwise a single unambiguous suffix/basename match in the workspace is
/// used and a one-line `(resolved ...)` note is returned. Ambiguous or unknown paths are errors that
/// list the candidates, so a write never lands on a guess.
pub(crate) fn resolve_existing(
    ws: &Path,
    cwd: Option<&Path>,
    p: &str,
    want_files: bool,
    want_dirs: bool,
) -> Result<(PathBuf, Option<String>), String> {
    let full = resolve_with_cwd(ws, cwd, p)?;
    let kind_ok = |f: &Path| (want_files && f.is_file()) || (want_dirs && f.is_dir());
    if kind_ok(&full) {
        return Ok((full, None));
    }
    let typed = rel_of(ws, &full);
    if full.is_dir() && !want_dirs {
        return Err(format!("{typed} is a directory (list it with: vx map --dir {typed})"));
    }
    let cands = find_path_candidates(ws, &typed, want_files, want_dirs);
    if cands.len() == 1 {
        let hit = resolve(ws, &cands[0])?;
        return Ok((hit, Some(format!("(resolved `{typed}` -> `{}`)", cands[0]))));
    }
    Err(missing_path_err(ws, &typed, want_files, want_dirs))
}

/// Where a new file should go. Prefers a location whose parent folder already exists (cwd, then
/// workspace root, then a unique suffix match of the parent folder, e.g. `components/ui/X.tsx` ->
/// `frontend/src/components/ui/X.tsx`), so a monorepo-relative path doesn't silently create a
/// parallel tree at the root. Falls back to the workspace-relative path (parents are created).
pub(crate) fn resolve_new_file(ws: &Path, cwd: Option<&Path>, p: &str) -> Result<(PathBuf, Option<String>), String> {
    let cleaned = p.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    let from_ws = resolve(ws, &cleaned)?;
    if Path::new(&cleaned).is_absolute() {
        return Ok((from_ws, None));
    }
    if let Some(c) = cwd {
        if let Ok(from_cwd) = resolve(ws, &c.join(&cleaned).to_string_lossy()) {
            if from_cwd.exists() || from_cwd.parent().is_some_and(Path::is_dir) {
                return Ok((from_cwd, None));
            }
        }
    }
    if from_ws.exists() || from_ws.parent().is_some_and(Path::is_dir) {
        return Ok((from_ws, None));
    }
    if let Some((parent, name)) = cleaned.trim_end_matches('/').rsplit_once('/') {
        let cands = find_path_candidates(ws, parent, false, true);
        if cands.len() == 1 {
            let rel = format!("{}/{name}", cands[0]);
            return Ok((resolve(ws, &rel)?, Some(format!("(resolved `{cleaned}` -> `{rel}`)"))));
        }
    }
    Ok((from_ws, None))
}

/// Format an actionable "not found" error with candidate matches or top-level workspace dirs.
pub(crate) fn missing_path_err(ws: &Path, display: &str, want_files: bool, want_dirs: bool) -> String {
    let cands = find_path_candidates(ws, display, want_files, want_dirs);
    if !cands.is_empty() {
        let shown: Vec<&str> = cands.iter().take(6).map(String::as_str).collect();
        let more = if cands.len() > 6 { format!(" (+{} more)", cands.len() - 6) } else { String::new() };
        return format!("{display} not found. Did you mean: {}{more}?", shown.join(", "));
    }
    let tops = top_level_dirs(ws);
    if !tops.is_empty() {
        format!("{display} not found (workspace root dirs: {})", tops.join(", "))
    } else {
        format!("{display} not found")
    }
}
