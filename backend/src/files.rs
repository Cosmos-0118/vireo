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

#[derive(Serialize)]
pub struct TaskItem {
    pub name: String,
    pub command: String,
    pub source: String,
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

/// Largest pasted/dropped chat image accepted (bytes).
pub const ATTACHMENT_MAX_BYTES: usize = 20 * 1024 * 1024;

#[derive(Deserialize)]
pub struct AttachmentQuery {
    pub ext: Option<String>,
}

/// Saves a pasted chat image (raw request body) to a temp folder and returns its absolute
/// path, so the agent can be pointed at it. Only common image extensions are accepted.
pub async fn save_attachment(
    Query(q): Query<AttachmentQuery>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty image".into()));
    }
    let ext = q
        .ext
        .as_deref()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"))
        .unwrap_or_else(|| "png".into());
    let dir = std::env::temp_dir().join("vireo-attachments");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let full = dir.join(format!("paste-{nanos}.{ext}"));
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

/// Converts a byte offset within `text` into UTF-16 code units.
fn utf16_offset(text: &str, byte_idx: usize) -> usize {
    let idx = byte_idx.min(text.len());
    text.char_indices()
        .take_while(|(i, _)| *i < idx)
        .map(|(_, c)| c.len_utf16())
        .sum()
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

    let rg_out = cmd.current_dir(&ws).output().await;

    if let Ok(output) = rg_out {
        // rg exits 0 on matches, 1 on none, 2 on error (it may still emit partial results).
        if output.status.code() == Some(2) && output.stdout.is_empty() {
            let err = String::from_utf8_lossy(&output.stderr);
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

        let mut matches: Vec<SearchMatch> = Vec::new();
        let mut truncated = false;
        let mut per_file: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if val.get("type").and_then(|t| t.as_str()) != Some("match") {
                continue;
            }
            let data = &val["data"];
            let Some(raw_text) = data["lines"]["text"].as_str() else {
                continue; // non-UTF8 line
            };
            if matches.len() >= SEARCH_MAX_TOTAL {
                truncated = true;
                break;
            }
            let path = data["path"]["text"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("./")
                .replace('\\', "/");
            let line_num = data["line_number"].as_u64().unwrap_or(1) as usize;
            let full = raw_text.trim_end_matches(['\r', '\n']);
            let mut ranges: Vec<(usize, usize)> = data["submatches"]
                .as_array()
                .map(|subs| {
                    subs.iter()
                        .filter_map(|m| {
                            let s = m["start"].as_u64()? as usize;
                            let e = m["end"].as_u64()? as usize;
                            Some((utf16_offset(full, s), utf16_offset(full, e)))
                        })
                        .collect()
                })
                .unwrap_or_default();
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
                let ranges: Vec<(usize, usize)> = matcher
                    .find_iter(line_str)
                    .map(|m| {
                        (
                            utf16_offset(line_str, m.start()),
                            utf16_offset(line_str, m.end()),
                        )
                    })
                    .collect();
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

pub async fn get_tasks(State(state): State<AppState>) -> Json<Vec<TaskItem>> {
    let ws = state.workspace_path().await;
    let mut tasks = Vec::new();

    // Check package.json
    let pkg_path = ws.join("package.json");
    if let Ok(raw) = tokio::fs::read_to_string(&pkg_path).await {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
                for (name, _) in scripts {
                    tasks.push(TaskItem {
                        name: format!("npm: {}", name),
                        command: format!("npm run {}", name),
                        source: "package.json".to_string(),
                    });
                }
            }
        }
    }

    // Check frontend/package.json (common monorepo layout)
    let fe_pkg = ws.join("frontend").join("package.json");
    if let Ok(raw) = tokio::fs::read_to_string(&fe_pkg).await {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
                for (name, _) in scripts {
                    tasks.push(TaskItem {
                        name: format!("frontend: {}", name),
                        command: format!("npm --prefix frontend run {}", name),
                        source: "frontend/package.json".to_string(),
                    });
                }
            }
        }
    }

    // Check Cargo.toml
    if ws.join("Cargo.toml").exists() {
        for (name, cmd) in [
            ("cargo check", "cargo check"),
            ("cargo test", "cargo test"),
            ("cargo build", "cargo build"),
            ("cargo run", "cargo run"),
        ] {
            tasks.push(TaskItem {
                name: name.to_string(),
                command: cmd.to_string(),
                source: "Cargo.toml".to_string(),
            });
        }
    }

    // Check backend/Cargo.toml
    if ws.join("backend").join("Cargo.toml").exists() {
        tasks.push(TaskItem {
            name: "backend: cargo check".to_string(),
            command: "cargo check --manifest-path backend/Cargo.toml".to_string(),
            source: "backend/Cargo.toml".to_string(),
        });
        tasks.push(TaskItem {
            name: "backend: cargo test".to_string(),
            command: "cargo test --manifest-path backend/Cargo.toml".to_string(),
            source: "backend/Cargo.toml".to_string(),
        });
    }

    // Check Python
    if ws.join("pyproject.toml").exists() || ws.join("pytest.ini").exists() {
        tasks.push(TaskItem {
            name: "pytest".to_string(),
            command: "pytest".to_string(),
            source: "python".to_string(),
        });
    }

    Json(tasks)
}
