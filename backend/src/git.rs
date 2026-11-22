use crate::state::{resolve_safe_path, AppState, Checkpoint, WorkspaceEvent};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct GitFileStatus {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Serialize)]
pub struct GitStatusRes {
    pub is_repo: bool,
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub remote_url: String,
    pub files: Vec<GitFileStatus>,
    pub total_additions: usize,
    pub total_deletions: usize,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Deserialize)]
pub struct DiffQuery {
    pub path: String,
    pub staged: Option<bool>,
}

#[derive(Serialize)]
pub struct GitDiffRes {
    pub path: String,
    pub staged: bool,
    pub original: String,
    pub modified: String,
    pub patch: String,
    pub binary: bool,
}

#[derive(Deserialize)]
pub struct StageReq {
    pub paths: Option<Vec<String>>,
    pub all: Option<bool>,
}

#[derive(Deserialize)]
pub struct DiscardReq {
    pub path: String,
    pub untracked: Option<bool>,
}

#[derive(Deserialize)]
pub struct CommitReq {
    pub message: String,
    pub stage_all: Option<bool>,
}

#[derive(Deserialize)]
pub struct PushReq {
    pub set_upstream: Option<bool>,
}

#[derive(Serialize)]
pub struct CommandRes {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Serialize)]
pub struct BranchesRes {
    pub current: String,
    pub branches: Vec<String>,
}

#[derive(Deserialize)]
pub struct CheckoutReq {
    pub branch: String,
    pub create: Option<bool>,
}

#[derive(Serialize)]
pub struct CommitLogItem {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

#[derive(Deserialize)]
pub struct CheckpointReq {
    pub label: Option<String>,
}

#[derive(Deserialize)]
pub struct RollbackReq {
    pub id: String,
}

async fn run_git(ws: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

fn parse_numstat(output: &str) -> HashMap<String, (usize, usize)> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let adds = parts[0].parse::<usize>().unwrap_or(0);
            let dels = parts[1].parse::<usize>().unwrap_or(0);
            let path = parts[2].replace('\\', "/");
            let entry = map.entry(path).or_insert((0, 0));
            entry.0 += adds;
            entry.1 += dels;
        }
    }
    map
}

pub async fn get_status(State(state): State<AppState>) -> Json<GitStatusRes> {
    let ws = state.workspace_path().await;
    let checkpoints = state.checkpoints.read().await.clone();

    let (is_repo, _, _) = run_git(&ws, &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap_or((false, String::new(), String::new()));

    if !is_repo {
        return Json(GitStatusRes {
            is_repo: false,
            branch: String::new(),
            ahead: 0,
            behind: 0,
            remote_url: String::new(),
            files: Vec::new(),
            total_additions: 0,
            total_deletions: 0,
            checkpoints,
        });
    }

    let (_, branch_out, _) = run_git(&ws, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or((false, "main".to_string(), String::new()));
    let branch = branch_out.trim().to_string();

    let (ab_ok, ab_out, _) = run_git(
        &ws,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .await
    .unwrap_or((false, String::new(), String::new()));
    let (behind, ahead) = if ab_ok {
        let parts: Vec<&str> = ab_out.trim().split_whitespace().collect();
        if parts.len() == 2 {
            (
                parts[0].parse::<usize>().unwrap_or(0),
                parts[1].parse::<usize>().unwrap_or(0),
            )
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let (_, remote_out, _) = run_git(&ws, &["remote", "get-url", "origin"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let remote_url = remote_out.trim().to_string();

    let (_, porcelain, _) = run_git(&ws, &["status", "--porcelain=v1", "-uall"])
        .await
        .unwrap_or((false, String::new(), String::new()));

    let (_, unstaged_stat, _) = run_git(&ws, &["diff", "--numstat"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let (_, staged_stat, _) = run_git(&ws, &["diff", "--cached", "--numstat"])
        .await
        .unwrap_or((false, String::new(), String::new()));

    let mut stats = parse_numstat(&unstaged_stat);
    for (k, (a, d)) in parse_numstat(&staged_stat) {
        let e = stats.entry(k).or_insert((0, 0));
        e.0 += a;
        e.1 += d;
    }

    let mut files = Vec::new();
    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;

    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let x = &line[0..1];
        let y = &line[1..2];
        let mut raw_path = line[3..].trim().to_string();
        if raw_path.starts_with('"') && raw_path.ends_with('"') && raw_path.len() >= 2 {
            raw_path = raw_path[1..raw_path.len() - 1].to_string();
        }
        if let Some((_, new_part)) = raw_path.split_once(" -> ") {
            raw_path = new_part.trim().trim_matches('"').to_string();
        }
        raw_path = raw_path.replace('\\', "/");

        let untracked = x == "?" && y == "?";
        let staged = !untracked && x != " ";
        let unstaged = !untracked && y != " ";

        let (mut adds, dels) = stats.get(&raw_path).copied().unwrap_or((0, 0));
        if untracked {
            if let Ok(full) = resolve_safe_path(&ws, &raw_path) {
                if let Ok(content) = std::fs::read_to_string(&full) {
                    adds = content.lines().count();
                }
            }
        }
        total_additions += adds;
        total_deletions += dels;

        files.push(GitFileStatus {
            path: raw_path,
            index_status: x.to_string(),
            worktree_status: y.to_string(),
            staged,
            unstaged,
            untracked,
            additions: adds,
            deletions: dels,
        });
    }

    Json(GitStatusRes {
        is_repo: true,
        branch,
        ahead,
        behind,
        remote_url,
        files,
        total_additions,
        total_deletions,
        checkpoints,
    })
}

pub async fn get_diff(
    State(state): State<AppState>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<GitDiffRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let rel = q.path.replace('\\', "/");
    let staged = q.staged.unwrap_or(false);

    let (original, modified, patch) = if staged {
        let (ok_orig, orig_out, _) = run_git(&ws, &["show", &format!("HEAD:{}", rel)])
            .await
            .unwrap_or((false, String::new(), String::new()));
        let (ok_mod, mod_out, _) = run_git(&ws, &["show", &format!(":{}", rel)])
            .await
            .unwrap_or((false, String::new(), String::new()));
        let (_, patch_out, _) = run_git(&ws, &["diff", "--cached", "--", &rel])
            .await
            .unwrap_or((false, String::new(), String::new()));
        (
            if ok_orig { orig_out } else { String::new() },
            if ok_mod { mod_out } else { String::new() },
            patch_out,
        )
    } else {
        // Compare HEAD (or index if newly staged) vs working tree
        let (ok_idx, idx_out, _) = run_git(&ws, &["show", &format!("HEAD:{}", rel)])
            .await
            .unwrap_or((false, String::new(), String::new()));
        let original_text = if ok_idx {
            idx_out
        } else {
            let (ok_stage, stage_out, _) = run_git(&ws, &["show", &format!(":{}", rel)])
                .await
                .unwrap_or((false, String::new(), String::new()));
            if ok_stage {
                stage_out
            } else {
                String::new()
            }
        };

        let modified_text = match resolve_safe_path(&ws, &rel) {
            Ok(full) => match tokio::fs::read(&full).await {
                Ok(bytes) => {
                    if bytes.iter().take(4096).any(|&b| b == 0) {
                        return Ok(Json(GitDiffRes {
                            path: rel,
                            staged,
                            original: String::new(),
                            modified: String::new(),
                            patch: String::new(),
                            binary: true,
                        }));
                    }
                    String::from_utf8_lossy(&bytes).to_string()
                }
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };

        let (_, patch_out, _) = run_git(&ws, &["diff", "HEAD", "--", &rel])
            .await
            .unwrap_or((false, String::new(), String::new()));
        (original_text, modified_text, patch_out)
    };

    Ok(Json(GitDiffRes {
        path: rel,
        staged,
        original,
        modified,
        patch,
        binary: false,
    }))
}

pub async fn stage_files(
    State(state): State<AppState>,
    Json(req): Json<StageReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let res = if req.all.unwrap_or(false) {
        run_git(&ws, &["add", "-A"]).await
    } else if let Some(paths) = req.paths {
        let mut args = vec!["add", "--"];
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        args.extend(refs);
        run_git(&ws, &args).await
    } else {
        return Err((StatusCode::BAD_REQUEST, "No paths provided".to_string()));
    };

    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:stage".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn unstage_files(
    State(state): State<AppState>,
    Json(req): Json<StageReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let res = if req.all.unwrap_or(false) {
        run_git(&ws, &["reset", "HEAD"]).await
    } else if let Some(paths) = req.paths {
        let mut args = vec!["reset", "HEAD", "--"];
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        args.extend(refs);
        run_git(&ws, &args).await
    } else {
        return Err((StatusCode::BAD_REQUEST, "No paths provided".to_string()));
    };

    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:unstage".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn discard_file(
    State(state): State<AppState>,
    Json(req): Json<DiscardReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    if req.untracked.unwrap_or(false) {
        let full = resolve_safe_path(&ws, &req.path).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        if full.exists() {
            tokio::fs::remove_file(&full)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        state.notify_fs_changed(vec![req.path]);
        return Ok(Json(CommandRes {
            ok: true,
            stdout: "Removed untracked file".to_string(),
            stderr: String::new(),
        }));
    }

    let _ = run_git(&ws, &["reset", "HEAD", "--", &req.path]).await;
    let (ok, stdout, stderr) = run_git(&ws, &["checkout", "--", &req.path])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec![req.path]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn commit_changes(
    State(state): State<AppState>,
    Json(req): Json<CommitReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let msg = req.message.trim();
    if msg.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Commit message cannot be empty".to_string()));
    }
    let ws = state.workspace_path().await;
    if req.stage_all.unwrap_or(false) {
        let _ = run_git(&ws, &["add", "-A"]).await;
    }
    let (ok, stdout, stderr) = run_git(&ws, &["commit", "-m", msg])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:commit".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn push_changes(
    State(state): State<AppState>,
    Json(req): Json<PushReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let res = if req.set_upstream.unwrap_or(false) {
        let (_, branch_out, _) = run_git(&ws, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .unwrap_or((false, "main".to_string(), String::new()));
        let b = branch_out.trim().to_string();
        run_git(&ws, &["push", "-u", "origin", &b]).await
    } else {
        run_git(&ws, &["push"]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:push".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn pull_changes(
    State(state): State<AppState>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let (ok, stdout, stderr) = run_git(&ws, &["pull"])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:pull".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn get_branches(State(state): State<AppState>) -> Json<BranchesRes> {
    let ws = state.workspace_path().await;
    let (_, cur, _) = run_git(&ws, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let (_, list, _) = run_git(&ws, &["branch", "--format=%(refname:short)"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let branches = list
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Json(BranchesRes {
        current: cur.trim().to_string(),
        branches,
    })
}

pub async fn checkout_branch(
    State(state): State<AppState>,
    Json(req): Json<CheckoutReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let b = req.branch.trim();
    if b.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Branch name required".to_string()));
    }
    let res = if req.create.unwrap_or(false) {
        run_git(&ws, &["checkout", "-b", b]).await
    } else {
        run_git(&ws, &["checkout", b]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:checkout".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn get_log(State(state): State<AppState>) -> Json<Vec<CommitLogItem>> {
    let ws = state.workspace_path().await;
    let (ok, out, _) = run_git(
        &ws,
        &["log", "-n", "20", "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ar"],
    )
    .await
    .unwrap_or((false, String::new(), String::new()));
    if !ok {
        return Json(Vec::new());
    }
    let mut items = Vec::new();
    for line in out.lines() {
        let p: Vec<&str> = line.split('\x1f').collect();
        if p.len() == 5 {
            items.push(CommitLogItem {
                hash: p[0].to_string(),
                short_hash: p[1].to_string(),
                message: p[2].to_string(),
                author: p[3].to_string(),
                date: p[4].to_string(),
            });
        }
    }
    Json(items)
}

/// Non-destructive git working-tree snapshot using a temporary GIT_INDEX_FILE.
pub async fn create_checkpoint_internal(
    state: &AppState,
    label: String,
) -> Result<Checkpoint, String> {
    let ws = state.workspace_path().await;
    let (is_repo, _, _) = run_git(&ws, &["rev-parse", "--is-inside-work-tree"]).await?;
    if !is_repo {
        return Err("Not a git repository".to_string());
    }

    let tmp_index = std::env::temp_dir().join(format!("litecode-idx-{}", uuid::Uuid::new_v4()));
    let add_out = Command::new("git")
        .args(["add", "-A"])
        .env("GIT_INDEX_FILE", &tmp_index)
        .current_dir(&ws)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !add_out.status.success() {
        let _ = std::fs::remove_file(&tmp_index);
        return Err(String::from_utf8_lossy(&add_out.stderr).to_string());
    }

    let tree_out = Command::new("git")
        .args(["write-tree"])
        .env("GIT_INDEX_FILE", &tmp_index)
        .current_dir(&ws)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp_index);

    if !tree_out.status.success() {
        return Err(String::from_utf8_lossy(&tree_out.stderr).to_string());
    }
    let tree_sha = String::from_utf8_lossy(&tree_out.stdout).trim().to_string();

    let cp = Checkpoint {
        id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        label,
        tree_sha,
        created_at: Utc::now().format("%H:%M:%S").to_string(),
    };

    {
        let mut cps = state.checkpoints.write().await;
        cps.insert(0, cp.clone());
        cps.truncate(20);
    }
    let _ = state.events_tx.send(WorkspaceEvent::CheckpointCreated {
        checkpoint: cp.clone(),
    });
    Ok(cp)
}

pub async fn create_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<CheckpointReq>,
) -> Result<Json<Checkpoint>, (StatusCode, String)> {
    let label = req
        .label
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Manual checkpoint".to_string());
    create_checkpoint_internal(&state, label)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

pub async fn rollback_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<RollbackReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let cp = {
        let cps = state.checkpoints.read().await;
        cps.iter().find(|c| c.id == req.id).cloned()
    }
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Checkpoint not found".to_string()))?;

    // Restore files from saved tree without changing HEAD commit
    let (ok, stdout, stderr) = run_git(&ws, &["read-tree", "--reset", "-u", &cp.tree_sha])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    // Reset index back to HEAD so working tree shows exact diff vs HEAD
    let _ = run_git(&ws, &["reset", "HEAD"]).await;
    state.notify_fs_changed(vec!["git:rollback".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}
