use crate::state::{is_ignored_dir_name, resolve_safe_path, AppState, WorkspaceEvent};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;
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

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[derive(Serialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub text: String,
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
        .unwrap_or("workspace")
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
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Open Repository Folder — Vireo")
        .set_directory(&current)
        .pick_folder()
        .await;

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
    let bytes = tokio::fs::read(&full)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let is_binary = bytes.iter().take(4096).any(|&b| b == 0);
    if is_binary {
        return Ok(Json(ReadFileRes {
            path: q.path,
            content: String::new(),
            size: bytes.len(),
            binary: true,
        }));
    }

    // Cap display size at 2MB to stay fast
    let truncated = if bytes.len() > 2 * 1024 * 1024 {
        String::from_utf8_lossy(&bytes[..2 * 1024 * 1024]).to_string()
    } else {
        String::from_utf8_lossy(&bytes).to_string()
    };

    Ok(Json(ReadFileRes {
        path: q.path,
        content: truncated,
        size: bytes.len(),
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

pub async fn search_files(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Json<Vec<SearchMatch>> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Json(Vec::new());
    }
    let ws = state.workspace_path().await;

    // Try ripgrep first for instant speed
    let rg_out = Command::new("rg")
        .args([
            "--json",
            "--smart-case",
            "--max-count",
            "8",
            "--max-filesize",
            "1M",
            "--glob",
            "!.git",
            "--glob",
            "!node_modules",
            "--glob",
            "!target",
            "--glob",
            "!dist",
            "--",
            &query,
        ])
        .current_dir(&ws)
        .output()
        .await;

    if let Ok(output) = rg_out {
        let mut matches = Vec::new();
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("type").and_then(|t| t.as_str()) == Some("match") {
                    let data = &val["data"];
                    let path = data["path"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .replace('\\', "/");
                    let line_num = data["line_number"].as_u64().unwrap_or(1) as usize;
                    let col = data["submatches"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|m| m["start"].as_u64())
                        .unwrap_or(0) as usize
                        + 1;
                    let text = data["lines"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .trim_end_matches(['\r', '\n'])
                        .chars()
                        .take(200)
                        .collect();
                    matches.push(SearchMatch {
                        path,
                        line: line_num,
                        column: col,
                        text,
                    });
                    if matches.len() >= 120 {
                        break;
                    }
                }
            }
        }
        return Json(matches);
    }

    // Fallback pure-Rust search if ripgrep is unavailable
    let q_lower = query.to_lowercase();
    let matches = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let walker = WalkDir::new(&ws).max_depth(6).into_iter().filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && is_ignored_dir_name(&name))
        });

        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.metadata().map(|m| m.len() > 512_000).unwrap_or(true) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let rel = entry
                    .path()
                    .strip_prefix(&ws)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                for (idx, line_str) in content.lines().enumerate() {
                    if let Some(pos) = line_str.to_lowercase().find(&q_lower) {
                        out.push(SearchMatch {
                            path: rel.clone(),
                            line: idx + 1,
                            column: pos + 1,
                            text: line_str.chars().take(200).collect(),
                        });
                        if out.len() >= 100 {
                            return out;
                        }
                    }
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default();

    Json(matches)
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
