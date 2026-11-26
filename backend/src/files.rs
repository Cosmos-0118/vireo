use crate::state::{
    background_command, is_ignored_dir_name, resolve_safe_path, AppState, WorkspaceEvent,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use walkdir::WalkDir;

#[derive(Serialize)]
pub struct WorkspaceInfo {
    pub path: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct SetWorkspaceReq {
    pub path: String,
}

#[derive(Serialize)]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Deserialize)]
pub struct TreeQuery {
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct ReadFileQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct ReadFileRes {
    pub path: String,
    pub content: String,
    pub size: usize,
    pub binary: bool,
}

#[derive(Deserialize)]
pub struct WriteFileReq {
    pub path: String,
    pub content: String,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct SearchReq {
    pub q: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub regex: bool,
    /// Comma separated globs, e.g. `frontend/**, *.rs`
    #[serde(default)]
    pub include: String,
    /// Comma separated globs, e.g. `*.lock, dist/**`
    #[serde(default)]
    pub exclude: String,
    #[serde(default = "default_true")]
    pub use_gitignore: bool,
    /// When set, only these workspace-relative files are searched.
    #[serde(default)]
    pub only_paths: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    /// 1-based column (UTF-16 units) of the first match on the line.
    pub column: usize,
    pub text: String,
    /// Match ranges `[start, end)` in UTF-16 units relative to `text`.
    pub ranges: Vec<(usize, usize)>,
}

#[derive(Serialize)]
pub struct SearchRes {
    pub matches: Vec<SearchMatch>,
    pub total_matches: usize,
    pub total_files: usize,
    pub truncated: bool,
}

#[derive(Deserialize)]
pub struct ReplaceTarget {
    pub path: String,
    /// 1-based lines to touch; `None` replaces in the whole file.
    #[serde(default)]
    pub lines: Option<Vec<usize>>,
}

#[derive(Deserialize)]
pub struct ReplaceReq {
    pub q: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub regex: bool,
    pub replacement: String,
    pub targets: Vec<ReplaceTarget>,
}

#[derive(Serialize)]
pub struct ReplaceRes {
    pub files_changed: usize,
    pub replacements: usize,
    pub skipped: Vec<String>,
}

pub async fn get_workspace(State(state): State<AppState>) -> Json<WorkspaceInfo> {
    let ws = state.workspace_path().await;
    let name = ws
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(if ws.as_os_str().is_empty() { "Vireo" } else { "workspace" })
        .to_string();
    Json(WorkspaceInfo {
        path: ws.to_string_lossy().to_string(),
        name,
    })
}

pub async fn set_workspace(
    State(state): State<AppState>,
    Json(req): Json<SetWorkspaceReq>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let target = PathBuf::from(req.path.trim());
    if !target.exists() || !target.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Directory does not exist: {}", req.path),
        ));
    }
    let canonical = std::fs::canonicalize(&target).unwrap_or(target);
    // Strip Windows extended path prefix \\?\ if present for cleaner display & git compatibility
    let cleaned_str = canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let cleaned_path = PathBuf::from(&cleaned_str);

    {
        let mut ws = state.workspace.write().await;
        *ws = cleaned_path.clone();
    }
    crate::state::save_last_workspace(&cleaned_path);
    state.checkpoints.write().await.clear();
    // Agent processes keep their working directory open; end them so the old folder is released.
    state.sessions.close_all().await;
    let _ = state.watcher_retarget_tx.send(cleaned_path.clone()).await;
    let _ = state.events_tx.send(WorkspaceEvent::WorkspaceSwitched {
        path: cleaned_str.clone(),
    });

    let name = cleaned_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_string();
    Ok(Json(WorkspaceInfo {
        path: cleaned_str,
        name,
    }))
}

pub async fn pick_workspace(
    State(state): State<AppState>,
) -> Result<Json<Option<WorkspaceInfo>>, (StatusCode, String)> {
    let current = state.workspace_path().await;
    let mut dialog = rfd::AsyncFileDialog::new().set_title("Open Folder");
    if !current.as_os_str().is_empty() {
        dialog = dialog.set_directory(&current);
    }
    let picked = dialog.pick_folder().await;

    let Some(folder) = picked else {
        return Ok(Json(None));
    };

    let target = folder.path().to_path_buf();
    let canonical = std::fs::canonicalize(&target).unwrap_or(target);
    let cleaned_str = canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let cleaned_path = PathBuf::from(&cleaned_str);

    {
        let mut ws = state.workspace.write().await;
        *ws = cleaned_path.clone();
    }
    crate::state::save_last_workspace(&cleaned_path);
    state.checkpoints.write().await.clear();
    let _ = state.watcher_retarget_tx.send(cleaned_path.clone()).await;
    let _ = state.events_tx.send(WorkspaceEvent::WorkspaceSwitched {
        path: cleaned_str.clone(),
    });

    let name = cleaned_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_string();
    Ok(Json(Some(WorkspaceInfo {
        path: cleaned_str,
        name,
    })))
}

pub async fn get_tree(
    State(state): State<AppState>,
    Query(q): Query<TreeQuery>,
) -> Result<Json<Vec<TreeEntry>>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let rel = q.path.unwrap_or_default();
    let dir = if rel.is_empty() {
        ws.clone()
    } else {
        resolve_safe_path(&ws, &rel).map_err(|e| (StatusCode::BAD_REQUEST, e))?
    };

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    for item in read_dir.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        let meta = match item.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        if is_dir && is_ignored_dir_name(&name) {
            continue;
        }
        let item_path = item.path();
        let rel_path = item_path
            .strip_prefix(&ws)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| name.clone());
        entries.push(TreeEntry {
            name,
            path: rel_path,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(entries))
}

pub async fn list_files(State(state): State<AppState>) -> Json<Vec<String>> {
    let ws = state.workspace_path().await;
    let files = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let walker = WalkDir::new(&ws)
            .max_depth(8)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && is_ignored_dir_name(&name))
            });

        for entry in walker.flatten() {
            if entry.file_type().is_file() {
                if let Ok(rel) = entry.path().strip_prefix(&ws) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                    if out.len() >= 4000 {
                        break;
                    }
                }
            }
        }
        out.sort();
        out
    })
    .await
    .unwrap_or_default();

    Json(files)
}

pub async fn read_file(
    State(state): State<AppState>,
    Query(q): Query<ReadFileQuery>,
) -> Result<Json<ReadFileRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let full = resolve_safe_path(&ws, &q.path).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    // Cap display size at 2MB to stay fast: never read more than that, even for huge files.
    const CAP: u64 = 2 * 1024 * 1024;
    let not_found = |e: std::io::Error| (StatusCode::NOT_FOUND, e.to_string());
    let file = tokio::fs::File::open(&full).await.map_err(not_found)?;
    let total = file.metadata().await.map_err(not_found)?.len() as usize;
    let mut bytes = Vec::with_capacity((total as u64).min(CAP) as usize);
    tokio::io::AsyncReadExt::read_to_end(&mut tokio::io::AsyncReadExt::take(file, CAP), &mut bytes)
        .await
        .map_err(not_found)?;

    let is_binary = bytes.iter().take(4096).any(|&b| b == 0);
    if is_binary {
        return Ok(Json(ReadFileRes {
            path: q.path,
            content: String::new(),
            size: total,
            binary: true,
        }));
    }

    Ok(Json(ReadFileRes {
        path: q.path,
        content: String::from_utf8_lossy(&bytes).to_string(),
        size: total,
        binary: false,
    }))
}

/// Streams a workspace image (used by the Markdown preview for relative `![](img.png)` links).
/// Only `image/*` files up to 20 MB are served; everything else is refused.
pub async fn read_raw(
    State(state): State<AppState>,
    Query(q): Query<ReadFileQuery>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let full = resolve_safe_path(&ws, &q.path).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let mime = mime_guess::from_path(&full).first_or_octet_stream();
    if mime.type_() != mime_guess::mime::IMAGE {
        return Err((StatusCode::UNSUPPORTED_MEDIA_TYPE, "Only images are served".into()));
    }
    let meta = tokio::fs::metadata(&full)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    if meta.len() > 20 * 1024 * 1024 {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "Image too large".into()));
    }
    let bytes = tokio::fs::read(&full)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(axum::response::Response::builder()
        .header("Content-Type", mime.as_ref())
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Security-Policy", "sandbox")
        .header("Cache-Control", "no-cache")
        .body(axum::body::Body::from(bytes))
        .unwrap())
}

pub async fn write_file(
    State(state): State<AppState>,
    Json(req): Json<WriteFileReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let full = resolve_safe_path(&ws, &req.path).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    tokio::fs::write(&full, req.content.as_bytes())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.notify_fs_changed(vec![req.path.clone()]);
    Ok(Json(serde_json::json!({ "ok": true, "path": req.path })))
}

/// Largest pasted/dropped chat attachment accepted (bytes).
pub const ATTACHMENT_MAX_BYTES: usize = 20 * 1024 * 1024;

#[derive(Deserialize)]
pub struct AttachmentQuery {
    pub ext: Option<String>,
    /// Original file name (dropped files); only a sanitized stem is kept in the saved name.
    pub name: Option<String>,
}

/// Saves a pasted/dropped file (raw request body) to a temp folder and returns its absolute
/// path, so the agent can be pointed at it. Extension and name are sanitized to plain
/// alphanumerics so the result can never escape the folder or be an odd executable name.
pub async fn save_attachment(
    Query(q): Query<AttachmentQuery>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty file".into()));
    }
    let ext = q
        .ext
        .as_deref()
        .map(|e| {
            e.trim_start_matches('.')
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(10)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "png".into());
    let stem: String = q
        .name
        .as_deref()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(40)
        .collect();
    let stem = if stem.is_empty() { "paste".to_string() } else { stem };
    let dir = std::env::temp_dir().join("vireo-attachments");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let full = dir.join(format!("{stem}-{nanos}.{ext}"));
    tokio::fs::write(&full, &body)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "path": full.to_string_lossy() })))
}

const SEARCH_MAX_TOTAL: usize = 500;
const SEARCH_MAX_PER_FILE: usize = 50;
const SEARCH_MAX_TEXT_UNITS: usize = 500;
const SEARCH_MAX_ONLY_PATHS: usize = 800;

fn split_globs(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|g| g.trim().replace('\\', "/"))
        .filter(|g| !g.is_empty() && !g.chars().any(char::is_control))
        .collect()
}

fn utf16_offset(text: &str, byte_idx: usize) -> usize {
    let idx = byte_idx.min(text.len());
    text.char_indices()
        .take_while(|(i, _)| *i < idx)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

/// Subset of rg's `--json` message we read (unknown fields are ignored).
#[derive(Deserialize)]
struct RgMsg {
    #[serde(rename = "type")]
    kind: String,
    data: Option<RgData>,
}
#[derive(Deserialize)]
struct RgData {
    path: Option<RgText>,
    lines: Option<RgText>,
    line_number: Option<u64>,
    submatches: Option<Vec<RgSub>>,
}
#[derive(Deserialize)]
struct RgText {
    text: Option<String>,
}
#[derive(Deserialize)]
struct RgSub {
    start: u64,
    end: u64,
}

/// `cap_units`: once a submatch starts at or past this many UTF-16 units (the displayed text is
/// truncated there) later ones are dropped without walking further, so a huge minified line costs
/// O(min(L, cap)) for the tail. The first submatch is always kept (it supplies the column).
fn utf16_ranges(full: &str, subs: &[(usize, usize)], cap_units: usize) -> Vec<(usize, usize)> {
    // One forward cursor over `full` (rg reports submatches in order), so the whole line is
    // walked once: O(L + K) instead of O(K * L). Out-of-order offsets rewind the cursor.
    let (mut bpos, mut upos, mut last_start) = (0usize, 0usize, 0usize);
    let mut at = |byte_idx: usize| -> usize {
        let idx = byte_idx.min(full.len());
        if bpos > 0 && idx <= last_start {
            (bpos, upos, last_start) = (0, 0, 0);
        }
        // Counts chars that start before `idx` (a mid-char `idx` includes that char).
        while bpos < idx {
            let Some(c) = full[bpos..].chars().next() else { break };
            last_start = bpos;
            upos += c.len_utf16();
            bpos += c.len_utf8();
        }
        upos
    };
    // Sorted input (what rg emits): everything after a past-the-cap start starts later still.
    let sorted = subs.windows(2).all(|w| w[0].0 <= w[1].0);
    let mut out = Vec::with_capacity(subs.len().min(64));
    for &(s, e) in subs {
        let start = at(s);
        if sorted && !out.is_empty() && start >= cap_units {
            break;
        }
        out.push((start, at(e)));
    }
    out
}

fn truncate_utf16(text: &str, max_units: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max_units {
            break;
        }
        out.push(c);
    }
    out
}

fn build_matcher(
    q: &str,
    regex: bool,
    whole_word: bool,
    case_sensitive: bool,
) -> Result<regex::Regex, String> {
    let mut pat = if regex {
        q.to_string()
    } else {
        regex::escape(q)
    };
    if whole_word {
        // Same half-boundary semantics as `rg --word-regexp`.
        pat = format!(r"\b{{start-half}}(?:{})\b{{end-half}}", pat);
    }
    regex::RegexBuilder::new(&pat)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| e.to_string())
}

pub async fn search_files(
    State(state): State<AppState>,
    Json(req): Json<SearchReq>,
) -> Result<Json<SearchRes>, (StatusCode, String)> {
    let query = req.q.clone();
    let empty = || {
        Json(SearchRes {
            matches: Vec::new(),
            total_matches: 0,
            total_files: 0,
            truncated: false,
        })
    };
    if query.is_empty() {
        return Ok(empty());
    }
    if let Some(paths) = &req.only_paths {
        if paths.is_empty() {
            return Ok(empty());
        }
    }
    let ws = state.workspace_path().await;

    let mut cmd = background_command("rg");
    cmd.args(["--json", "--max-count"]);
    cmd.arg(SEARCH_MAX_PER_FILE.to_string());
    cmd.args(["--max-filesize", "1M"]);
    cmd.arg(if req.case_sensitive {
        "--case-sensitive"
    } else {
        "--ignore-case"
    });
    if !req.regex {
        cmd.arg("--fixed-strings");
    }
    if req.whole_word {
        cmd.arg("--word-regexp");
    }
    if !req.use_gitignore {
        cmd.arg("--no-ignore-vcs");
    }
    // Later globs win in ripgrep: user includes first so they can never re-include
    // the built-in excluded directories, then built-ins, then user excludes.
    for g in split_globs(&req.include) {
        cmd.args(["--glob", &g]);
    }
    for g in ["!.git", "!node_modules", "!target", "!dist"] {
        cmd.args(["--glob", g]);
    }
    for g in split_globs(&req.exclude) {
        let g = g.trim_start_matches('!');
        cmd.args(["--glob", &format!("!{}", g)]);
    }
    // `--regexp=` binds the query to the flag, and `--` ends options, so neither the query nor a
    // path can be read as an rg option.
    cmd.arg(format!("--regexp={query}"));
    cmd.arg("--");
    if let Some(paths) = &req.only_paths {
        // Pass the workspace-confined path we resolved, never the raw request string.
        for p in paths.iter().take(SEARCH_MAX_ONLY_PATHS) {
            if let Ok(full) = resolve_safe_path(&ws, p) {
                // Relative to the workspace (rg's cwd) so result paths keep their usual form.
                match full.strip_prefix(&ws) {
                    Ok(rel) if rel.as_os_str().is_empty() => cmd.arg("."),
                    Ok(rel) => cmd.arg(rel),
                    Err(_) => cmd.arg(&full),
                };
            }
        }
    }

    cmd.current_dir(&ws)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Ok(mut child) = cmd.spawn() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        // Drain stderr concurrently so a chatty rg can never block on a full pipe.
        let stderr_task = child.stderr.take().map(|mut s| {
            tokio::spawn(async move {
                let mut b = Vec::new();
                let _ = s.read_to_end(&mut b).await;
                b
            })
        });

        let mut matches: Vec<SearchMatch> = Vec::new();
        let mut truncated = false;
        let mut killed = false;
        let mut saw_output = false;
        let mut per_file: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // Stream rg's JSON and stop it as soon as the result cap is hit, instead of letting it
        // scan the whole repo and buffering every match in memory.
        if let Some(out) = child.stdout.take() {
            let mut reader = tokio::io::BufReader::with_capacity(64 * 1024, out);
            let mut buf: Vec<u8> = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                saw_output = true;
                let Ok(msg) = serde_json::from_slice::<RgMsg>(&buf) else {
                    continue;
                };
                if msg.kind != "match" {
                    continue;
                }
                let Some(data) = msg.data else { continue };
                let Some(raw_text) = data.lines.and_then(|l| l.text) else {
                    continue; // non-UTF8 line
                };
                if matches.len() >= SEARCH_MAX_TOTAL {
                    truncated = true;
                    killed = true;
                    break;
                }
                let path = data
                    .path
                    .and_then(|p| p.text)
                    .unwrap_or_default()
                    .trim_start_matches("./")
                    .replace('\\', "/");
                let line_num = data.line_number.unwrap_or(1) as usize;
                let full = raw_text.trim_end_matches(['\r', '\n']);
                let raw: Vec<(usize, usize)> = data
                    .submatches
                    .unwrap_or_default()
                    .iter()
                    .map(|m| (m.start as usize, m.end as usize))
                    .collect();
                let mut ranges = utf16_ranges(full, &raw, SEARCH_MAX_TEXT_UNITS);
                let column = ranges.first().map(|r| r.0).unwrap_or(0) + 1;
                let text = truncate_utf16(full, SEARCH_MAX_TEXT_UNITS);
                let text_units: usize = text.chars().map(|c| c.len_utf16()).sum();
                ranges.retain(|r| r.0 < text_units);
                for r in ranges.iter_mut() {
                    r.1 = r.1.min(text_units);
                }
                let count = per_file.entry(path.clone()).or_insert(0);
                *count += 1;
                if *count >= SEARCH_MAX_PER_FILE {
                    truncated = true;
                }
                matches.push(SearchMatch {
                    path,
                    line: line_num,
                    column,
                    text,
                    ranges,
                });
            }
        }
        if killed {
            let _ = child.kill().await;
        }
        let status = child.wait().await.ok();
        let stderr_bytes = match stderr_task {
            Some(t) => t.await.unwrap_or_default(),
            None => Vec::new(),
        };

        // rg exits 0 on matches, 1 on none, 2 on error (it may still emit partial results).
        if !killed && status.and_then(|s| s.code()) == Some(2) && !saw_output {
            let err = String::from_utf8_lossy(&stderr_bytes);
            let detail = err
                .lines()
                .map(str::trim)
                .rev()
                .find(|l| l.starts_with("error:"))
                .map(|l| l.trim_start_matches("error:").trim());
            let head = err
                .lines()
                .map(|l| l.trim().trim_start_matches("rg:").trim())
                .find(|l| !l.is_empty())
                .unwrap_or("Search failed")
                .trim_end_matches(':');
            let msg = match detail {
                Some(d) if !d.is_empty() => format!("{head}: {d}"),
                _ => head.to_string(),
            };
            return Err((StatusCode::BAD_REQUEST, msg));
        }

        // rg searches in parallel; keep file order stable (line order is preserved per file).
        matches.sort_by(|a, b| a.path.cmp(&b.path));
        let total_files = per_file.len();
        let total_matches = matches.len();
        return Ok(Json(SearchRes {
            matches,
            total_matches,
            total_files,
            truncated,
        }));
    }

    // Fallback pure-Rust search if ripgrep is unavailable (globs are not supported here).
    let matcher = build_matcher(&query, req.regex, req.whole_word, req.case_sensitive)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let only: Option<Vec<String>> = req.only_paths.clone();
    let res = tokio::task::spawn_blocking(move || {
        let mut out: Vec<SearchMatch> = Vec::new();
        let mut files = std::collections::HashSet::new();
        let mut truncated = false;
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(list) = only {
            candidates.extend(list.iter().map(|p| ws.join(p)));
        } else {
            let walker = WalkDir::new(&ws).max_depth(8).into_iter().filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && is_ignored_dir_name(&name))
            });
            for entry in walker.flatten() {
                if entry.file_type().is_file()
                    && entry.metadata().map(|m| m.len() <= 1_000_000).unwrap_or(false)
                {
                    candidates.push(entry.path().to_path_buf());
                }
            }
        }
        'files: for file in candidates {
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            let rel = file
                .strip_prefix(&ws)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let mut in_file = 0;
            for (idx, line_str) in content.lines().enumerate() {
                let raw: Vec<(usize, usize)> = matcher.find_iter(line_str).map(|m| (m.start(), m.end())).collect();
                let ranges = utf16_ranges(line_str, &raw, SEARCH_MAX_TEXT_UNITS);
                if ranges.is_empty() {
                    continue;
                }
                if out.len() >= SEARCH_MAX_TOTAL {
                    truncated = true;
                    break 'files;
                }
                files.insert(rel.clone());
                out.push(SearchMatch {
                    path: rel.clone(),
                    line: idx + 1,
                    column: ranges[0].0 + 1,
                    text: truncate_utf16(line_str, SEARCH_MAX_TEXT_UNITS),
                    ranges,
                });
                in_file += 1;
                if in_file >= SEARCH_MAX_PER_FILE {
                    truncated = true;
                    break;
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        let total_matches = out.len();
        SearchRes {
            matches: out,
            total_matches,
            total_files: files.len(),
            truncated,
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(res))
}

pub async fn replace_in_files(
    State(state): State<AppState>,
    Json(req): Json<ReplaceReq>,
) -> Result<Json<ReplaceRes>, (StatusCode, String)> {
    if req.q.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty search query".to_string()));
    }
    let matcher = build_matcher(&req.q, req.regex, req.whole_word, req.case_sensitive)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let ws = state.workspace_path().await;

    let mut files_changed = 0;
    let mut replacements = 0;
    let mut skipped = Vec::new();
    let mut changed_paths = Vec::new();

    for target in &req.targets {
        let full = match resolve_safe_path(&ws, &target.path) {
            Ok(p) => p,
            Err(_) => {
                skipped.push(target.path.clone());
                continue;
            }
        };
        let Ok(bytes) = tokio::fs::read(&full).await else {
            skipped.push(target.path.clone());
            continue;
        };
        let Ok(content) = String::from_utf8(bytes) else {
            skipped.push(target.path.clone());
            continue;
        };
        let only_lines: Option<std::collections::HashSet<usize>> =
            target.lines.as_ref().map(|l| l.iter().copied().collect());

        let mut out = String::with_capacity(content.len());
        let mut file_count = 0;
        for (idx, raw) in content.split_inclusive('\n').enumerate() {
            if let Some(set) = &only_lines {
                if !set.contains(&(idx + 1)) {
                    out.push_str(raw);
                    continue;
                }
            }
            let body = raw.trim_end_matches(['\r', '\n']);
            let eol = &raw[body.len()..];
            let n = matcher.find_iter(body).count();
            if n == 0 {
                out.push_str(raw);
                continue;
            }
            file_count += n;
            let replaced = if req.regex {
                matcher.replace_all(body, req.replacement.as_str())
            } else {
                matcher.replace_all(body, regex::NoExpand(req.replacement.as_str()))
            };
            out.push_str(&replaced);
            out.push_str(eol);
        }
        if file_count == 0 {
            continue;
        }
        if tokio::fs::write(&full, out.as_bytes()).await.is_err() {
            skipped.push(target.path.clone());
            continue;
        }
        files_changed += 1;
        replacements += file_count;
        changed_paths.push(target.path.clone());
    }

    if !changed_paths.is_empty() {
        state.notify_fs_changed(changed_paths);
    }
    Ok(Json(ReplaceRes {
        files_changed,
        replacements,
        skipped,
    }))
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    fn utf16_ranges_match_reference() {
        let line = "aé😀b😀😀cd é x😀y;";
        let n = line.len();
        let mut subs = Vec::new();
        for s in 0..=n {
            for e in s..=n.min(s + 9) {
                subs.push((s, e));
            }
        }
        // Sorted, reversed and shuffled orders must all agree with the per-offset reference.
        let mut shuffled = subs.clone();
        shuffled.sort_by_key(|&(s, e)| (s * 7 + e * 13) % 31);
        let mut reversed = subs.clone();
        reversed.reverse();
        for set in [&subs, &reversed, &shuffled] {
            let want: Vec<_> = set.iter().map(|&(s, e)| (utf16_offset(line, s), utf16_offset(line, e))).collect();
            assert_eq!(utf16_ranges(line, set, usize::MAX), want);
        }
        assert_eq!(utf16_ranges(line, &[(0, 0), (n + 50, n + 99)], usize::MAX), vec![(0, 0), (utf16_offset(line, n), utf16_offset(line, n))]);
        // Cap: sorted submatches past it are dropped, the first one is always kept.
        let sorted: Vec<_> = (0..n.saturating_sub(2)).step_by(3).map(|s| (s, s + 1)).filter(|&(s, e)| line.is_char_boundary(s) && line.is_char_boundary(e)).collect();
        let capped = utf16_ranges(line, &sorted, 8);
        assert!(capped.iter().skip(1).all(|r| r.0 < 8 + 2));
        assert_eq!(capped.first(), sorted.first().map(|&(s, e)| (utf16_offset(line, s), utf16_offset(line, e))).as_ref());
        let far = utf16_ranges(line, &[(n - 1, n)], 0);
        assert_eq!(far.len(), 1);
    }

    #[test]
    #[ignore]
    fn bench_utf16_ranges() {
        // Minified-JS-like line: 200 KB with a match every 8 bytes, some multi-byte chars.
        let line: String = (0..25_000).map(|i| if i % 50 == 0 { "é=ab;" } else { "a=b;;" }).collect::<String>().repeat(1);
        let subs: Vec<(usize, usize)> = (0..line.len() / 8).map(|i| (i * 8, i * 8 + 2)).filter(|(_, e)| line.is_char_boundary(*e) && line.is_char_boundary(*e - 2)).collect();
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            std::hint::black_box(utf16_ranges(&line, &subs, usize::MAX));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("BENCH utf16_ranges({} bytes line, {} submatches) {best:.2} ms", line.len(), subs.len());
    }
}
