use crate::state::{background_command, resolve_safe_path, AppState, Checkpoint};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

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

#[derive(Debug, Clone, Serialize)]
pub struct StashItem {
    pub index: String,
    pub message: String,
    pub date: String,
}

#[derive(Debug, Serialize)]
pub struct GitStatusRes {
    pub is_repo: bool,
    pub branch: String,
    pub head_hash: String,
    pub ahead: usize,
    pub behind: usize,
    pub remote_url: String,
    pub state: Option<String>,
    pub files: Vec<GitFileStatus>,
    pub total_additions: usize,
    pub total_deletions: usize,
    pub checkpoints: Vec<Checkpoint>,
    pub stashes: Vec<StashItem>,
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
pub struct DiscardAllReq {
    pub include_untracked: Option<bool>,
}

#[derive(Deserialize)]
pub struct CommitReq {
    pub message: String,
    pub stage_all: Option<bool>,
    pub amend: Option<bool>,
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
    pub remote_branches: Vec<String>,
    /// Rich per-branch info, newest commit first.
    pub details: Vec<BranchInfo>,
    /// Upstream of the checked-out branch (e.g. "origin/main"), empty if none.
    pub upstream: String,
    /// True when the configured upstream no longer exists on the remote.
    pub upstream_gone: bool,
    /// HEAD is not on a branch.
    pub detached: bool,
    pub head_short: String,
    /// Remote default branch (from refs/remotes/origin/HEAD), e.g. "main".
    pub default_branch: String,
    /// Configured remotes (e.g. ["origin"]).
    pub remotes: Vec<String>,
    /// Unix seconds of the last fetch (mtime of FETCH_HEAD), if any.
    pub last_fetched: Option<i64>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct BranchInfo {
    pub name: String,
    pub kind: String, // "local" | "remote"
    pub is_current: bool,
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    pub upstream_gone: bool,
    pub timestamp: i64,
    pub relative_date: String,
    pub subject: String,
}

/// Parses `git for-each-ref` output produced with BRANCH_REF_FORMAT.
fn parse_branch_refs(raw: &str) -> Vec<BranchInfo> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let f: Vec<&str> = line.split('\u{1f}').collect();
        if f.len() < 7 {
            continue;
        }
        let full = f[0].trim();
        let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
            ("local", n)
        } else if let Some(n) = full.strip_prefix("refs/remotes/") {
            ("remote", n)
        } else {
            continue;
        };
        if kind == "remote" && (name.ends_with("/HEAD") || !name.contains('/')) {
            continue;
        }
        let track = f[2].trim();
        let mut ahead = 0;
        let mut behind = 0;
        let upstream_gone = track == "gone";
        for part in track.split(',') {
            let p = part.trim();
            if let Some(n) = p.strip_prefix("ahead ") {
                ahead = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = p.strip_prefix("behind ") {
                behind = n.trim().parse().unwrap_or(0);
            }
        }
        out.push(BranchInfo {
            name: name.to_string(),
            kind: kind.to_string(),
            is_current: f[3].trim() == "*",
            upstream: f[1].trim().to_string(),
            ahead,
            behind,
            upstream_gone,
            timestamp: f[4].trim().parse().unwrap_or(0),
            relative_date: f[5].trim().to_string(),
            subject: f[6..].join("\u{1f}").trim().to_string(),
        });
    }
    out
}

const BRANCH_REF_FORMAT: &str = "--format=%(refname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)%1f%(committerdate:unix)%1f%(committerdate:relative)%1f%(subject)";

#[derive(Deserialize, Default)]
pub struct PullReq {
    /// "merge" | "rebase" | "ff-only"; omitted = git's configured default
    pub mode: Option<String>,
}

#[derive(Deserialize)]
pub struct CheckoutReq {
    pub branch: String,
    pub create: Option<bool>,
    pub start_point: Option<String>,
}

#[derive(Deserialize)]
pub struct DeleteBranchReq {
    pub name: String,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitRef {
    pub name: String,
    pub kind: String, // "head" | "local" | "remote" | "tag"
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitLogItem {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<GitRef>,
    pub message: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
    pub iso_date: String,
    pub timestamp: i64,
    pub is_head: bool,
}

#[derive(Deserialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
    pub skip: Option<usize>,
    pub all: Option<bool>,
    pub search: Option<String>,
    pub branch: Option<String>,
}

#[derive(Serialize)]
pub struct CommitLogRes {
    pub commits: Vec<CommitLogItem>,
    pub has_more: bool,
    pub head_hash: String,
}

#[derive(Deserialize)]
pub struct CommitQuery {
    pub hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitFileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

#[derive(Serialize)]
pub struct CommitDetailRes {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<GitRef>,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
    pub iso_date: String,
    pub files: Vec<CommitFileChange>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

#[derive(Deserialize)]
pub struct CommitDiffQuery {
    pub hash: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Serialize)]
pub struct CommitPatchRes {
    pub hash: String,
    pub patch: String,
}

#[derive(Deserialize)]
pub struct CompareFilesQuery {
    pub base: String,
}

#[derive(Deserialize)]
pub struct CompareDiffQuery {
    pub base: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Deserialize)]
pub struct CherryPickReq {
    pub hash: String,
    pub no_commit: Option<bool>,
}

#[derive(Deserialize)]
pub struct RevertReq {
    pub hash: String,
    pub no_commit: Option<bool>,
}

#[derive(Deserialize)]
pub struct ResetReq {
    pub target: String,
    pub mode: String, // "soft" | "mixed" | "hard"
}

#[derive(Deserialize)]
pub struct MergeReq {
    pub target: String,
}

#[derive(Deserialize)]
pub struct TagReq {
    pub name: String,
    pub target: String,
    pub message: Option<String>,
}

#[derive(Deserialize)]
pub struct StashReq {
    pub action: String, // "push" | "pop" | "apply" | "drop"
    pub message: Option<String>,
    pub index: Option<String>,
}

#[derive(Deserialize)]
pub struct OperationReq {
    pub kind: String,   // "cherry-pick" | "merge" | "revert" | "rebase"
    pub action: String, // "continue" | "abort"
}

#[derive(Deserialize)]
pub struct ApplyPatchReq {
    pub patch: String,
    pub cached: Option<bool>,
    pub reverse: Option<bool>,
}

#[derive(Deserialize)]
pub struct OpenExternalReq {
    pub url: String,
}

async fn run_git(ws: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = background_command("git")
        .args(args)
        .current_dir(ws)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

async fn run_git_bytes(ws: &Path, args: &[&str]) -> Result<(bool, Vec<u8>, String), String> {
    let output = background_command("git")
        .args(args)
        .current_dir(ws)
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {}", e))?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), output.stdout, stderr))
}

async fn run_git_with_stdin(
    ws: &Path,
    args: &[&str],
    stdin_data: &[u8],
) -> Result<(bool, String, String), String> {
    let mut child = background_command("git")
        .args(args)
        .current_dir(ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn git: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data)
            .await
            .map_err(|e| format!("Failed to write stdin to git: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for git: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|&b| b == 0)
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

fn parse_refs(raw: &str, remotes: &[String]) -> (Vec<GitRef>, bool) {
    let mut refs = Vec::new();
    let mut is_head = false;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (refs, is_head);
    }

    for part in trimmed.split(',') {
        let tok = part.trim();
        if tok.is_empty() {
            continue;
        }
        if let Some(branch) = tok.strip_prefix("HEAD -> ") {
            is_head = true;
            refs.push(GitRef {
                name: branch.trim().to_string(),
                kind: "local".to_string(),
                is_current: true,
            });
        } else if tok == "HEAD" {
            is_head = true;
            refs.push(GitRef {
                name: "HEAD".to_string(),
                kind: "head".to_string(),
                is_current: true,
            });
        } else if let Some(tag) = tok.strip_prefix("tag: ") {
            refs.push(GitRef {
                name: tag.trim().to_string(),
                kind: "tag".to_string(),
                is_current: false,
            });
        } else if tok.ends_with("/HEAD") || tok.contains(" -> ") {
            // Skip symbolic remote HEAD pointers like origin/HEAD
            continue;
        } else {
            let is_remote = remotes
                .iter()
                .any(|r| tok.starts_with(&format!("{}/", r)));
            refs.push(GitRef {
                name: tok.to_string(),
                kind: if is_remote {
                    "remote".to_string()
                } else {
                    "local".to_string()
                },
                is_current: false,
            });
        }
    }

    (refs, is_head)
}

async fn get_remotes(ws: &Path) -> Vec<String> {
    let (ok, out, _) = run_git(ws, &["remote"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if !ok {
        return Vec::new();
    }
    out.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

async fn detect_git_state(ws: &Path) -> Option<String> {
    let (ok, git_dir_out, _) = run_git(ws, &["rev-parse", "--git-dir"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if !ok {
        return None;
    }
    let git_dir_raw = git_dir_out.trim();
    let git_dir = if Path::new(git_dir_raw).is_absolute() {
        std::path::PathBuf::from(git_dir_raw)
    } else {
        ws.join(git_dir_raw)
    };

    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some("cherry-pick".to_string())
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some("merge".to_string())
    } else if git_dir.join("REVERT_HEAD").exists() {
        Some("revert".to_string())
    } else if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some("rebase".to_string())
    } else {
        None
    }
}

/// Parse NUL-delimited `--name-status -M -z` and `--numstat -M -z` outputs into CommitFileChange list.
fn parse_z_diff_files(name_status_bytes: &[u8], numstat_bytes: &[u8]) -> Vec<CommitFileChange> {
    // 1. Parse numstat -z -> map new_path -> (adds, dels, binary)
    let mut numstat_map: HashMap<String, (usize, usize, bool)> = HashMap::new();
    let ns_tokens: Vec<String> = numstat_bytes
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();
    let mut idx = 0;
    while idx < ns_tokens.len() {
        let tok = &ns_tokens[idx];
        if tok.is_empty() {
            idx += 1;
            continue;
        }
        let parts: Vec<&str> = tok.split('\t').collect();
        if parts.len() >= 3 {
            let binary = parts[0] == "-" || parts[1] == "-";
            let adds = parts[0].parse::<usize>().unwrap_or(0);
            let dels = parts[1].parse::<usize>().unwrap_or(0);
            if parts[2].is_empty() {
                // Rename/copy entry in -z mode: next two tokens are old_path and new_path
                if idx + 2 < ns_tokens.len() {
                    let new_path = ns_tokens[idx + 2].replace('\\', "/");
                    numstat_map.insert(new_path, (adds, dels, binary));
                    idx += 3;
                    continue;
                }
            } else {
                let path = parts[2].replace('\\', "/");
                numstat_map.insert(path, (adds, dels, binary));
            }
        }
        idx += 1;
    }

    // 2. Parse name-status -z
    let mut files = Vec::new();
    let st_tokens: Vec<String> = name_status_bytes
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();
    let mut i = 0;
    while i < st_tokens.len() {
        let status_tok = st_tokens[i].trim();
        if status_tok.is_empty() {
            i += 1;
            continue;
        }
        let code = status_tok.chars().next().unwrap_or('M').to_string();
        if (code == "R" || code == "C") && i + 2 < st_tokens.len() {
            let old_p = st_tokens[i + 1].replace('\\', "/");
            let new_p = st_tokens[i + 2].replace('\\', "/");
            let (adds, dels, binary) = numstat_map.get(&new_p).copied().unwrap_or((0, 0, false));
            files.push(CommitFileChange {
                path: new_p,
                old_path: Some(old_p),
                status: code,
                additions: adds,
                deletions: dels,
                binary,
            });
            i += 3;
        } else if i + 1 < st_tokens.len() {
            let p = st_tokens[i + 1].replace('\\', "/");
            let (adds, dels, binary) = numstat_map.get(&p).copied().unwrap_or((0, 0, false));
            files.push(CommitFileChange {
                path: p,
                old_path: None,
                status: code,
                additions: adds,
                deletions: dels,
                binary,
            });
            i += 2;
        } else {
            break;
        }
    }

    files
}

/// Current branch name. Works on an unborn branch (fresh `git init`, no commits);
/// returns "HEAD" only when HEAD is truly detached.
async fn current_branch(ws: &Path) -> String {
    if let Ok((true, out, _)) = run_git(ws, &["symbolic-ref", "--short", "-q", "HEAD"]).await {
        let b = out.trim();
        if !b.is_empty() {
            return b.to_string();
        }
    }
    match run_git(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).await {
        Ok((_, out, _)) if !out.trim().is_empty() => out.trim().to_string(),
        _ => "HEAD".to_string(),
    }
}

fn validate_git_ref_arg(s: &str) -> Result<&str, (StatusCode, String)> {
    let t = s.trim();
    if t.is_empty() || t.starts_with('-') || t.contains('\0') || t.contains('\n') || t.contains('\r')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid git ref or hash".to_string()));
    }
    Ok(t)
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
            head_hash: String::new(),
            ahead: 0,
            behind: 0,
            remote_url: String::new(),
            state: None,
            files: Vec::new(),
            total_additions: 0,
            total_deletions: 0,
            checkpoints,
            stashes: Vec::new(),
        });
    }

    let branch = current_branch(&ws).await;

    let (head_ok, head_out, _) = run_git(&ws, &["rev-parse", "HEAD"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let head_hash = if head_ok {
        head_out.trim().to_string()
    } else {
        String::new()
    };

    let git_state = detect_git_state(&ws).await;

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

    // Stashes list
    let (st_ok, st_out, _) = run_git(&ws, &["stash", "list", "--format=%gd%x1f%gs%x1f%ar"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let mut stashes = Vec::new();
    if st_ok {
        for line in st_out.lines() {
            let p: Vec<&str> = line.split('\x1f').collect();
            if p.len() >= 3 {
                stashes.push(StashItem {
                    index: p[0].to_string(),
                    message: p[1].to_string(),
                    date: p[2].to_string(),
                });
            }
        }
    }

    Json(GitStatusRes {
        is_repo: true,
        branch,
        head_hash,
        ahead,
        behind,
        remote_url,
        state: git_state,
        files,
        total_additions,
        total_deletions,
        checkpoints,
        stashes,
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
        let (ok_orig, orig_bytes, _) = run_git_bytes(&ws, &["show", &format!("HEAD:{}", rel)])
            .await
            .unwrap_or((false, Vec::new(), String::new()));
        let (ok_mod, mod_bytes, _) = run_git_bytes(&ws, &["show", &format!(":{}", rel)])
            .await
            .unwrap_or((false, Vec::new(), String::new()));
        if (ok_orig && is_binary_bytes(&orig_bytes)) || (ok_mod && is_binary_bytes(&mod_bytes)) {
            return Ok(Json(GitDiffRes {
                path: rel,
                staged,
                original: String::new(),
                modified: String::new(),
                patch: String::new(),
                binary: true,
            }));
        }
        let (_, patch_out, _) = run_git(&ws, &["diff", "--cached", "--", &rel])
            .await
            .unwrap_or((false, String::new(), String::new()));
        (
            if ok_orig {
                String::from_utf8_lossy(&orig_bytes).to_string()
            } else {
                String::new()
            },
            if ok_mod {
                String::from_utf8_lossy(&mod_bytes).to_string()
            } else {
                String::new()
            },
            patch_out,
        )
    } else {
        // Compare index (if file is staged too, VS Code compares index vs working tree) or HEAD vs working tree
        let (ok_idx, idx_bytes, _) = run_git_bytes(&ws, &["show", &format!(":{}", rel)])
            .await
            .unwrap_or((false, Vec::new(), String::new()));
        let (orig_ok, orig_bytes) = if ok_idx {
            (true, idx_bytes)
        } else {
            let (ok_head, head_bytes, _) =
                run_git_bytes(&ws, &["show", &format!("HEAD:{}", rel)])
                    .await
                    .unwrap_or((false, Vec::new(), String::new()));
            (ok_head, head_bytes)
        };
        if orig_ok && is_binary_bytes(&orig_bytes) {
            return Ok(Json(GitDiffRes {
                path: rel,
                staged,
                original: String::new(),
                modified: String::new(),
                patch: String::new(),
                binary: true,
            }));
        }
        let original_text = if orig_ok {
            String::from_utf8_lossy(&orig_bytes).to_string()
        } else {
            String::new()
        };

        let modified_text = match resolve_safe_path(&ws, &rel) {
            Ok(full) => match tokio::fs::read(&full).await {
                Ok(bytes) => {
                    if is_binary_bytes(&bytes) {
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

        let (_, patch_out, _) = run_git(&ws, &["diff", "--", &rel])
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

pub async fn discard_all(
    State(state): State<AppState>,
    Json(req): Json<DiscardAllReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let (ok, stdout, stderr) = run_git(&ws, &["checkout", "--", "."])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if ok && req.include_untracked.unwrap_or(false) {
        let _ = run_git(&ws, &["clean", "-fd"]).await;
    }
    state.notify_fs_changed(vec!["git:discard-all".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn commit_changes(
    State(state): State<AppState>,
    Json(req): Json<CommitReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let msg = req.message.trim();
    if msg.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Commit message cannot be empty".to_string(),
        ));
    }
    let ws = state.workspace_path().await;
    if req.stage_all.unwrap_or(false) {
        let _ = run_git(&ws, &["add", "-A"]).await;
    }
    let res = if req.amend.unwrap_or(false) {
        run_git(&ws, &["commit", "--amend", "-m", msg]).await
    } else {
        run_git(&ws, &["commit", "-m", msg]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:commit".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn push_changes(
    State(state): State<AppState>,
    Json(req): Json<PushReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let res = if req.set_upstream.unwrap_or(false) {
        let b = current_branch(&ws).await;
        let remote = pick_remote(&ws).await;
        run_git(&ws, &["push", "-u", &remote, &b]).await
    } else {
        run_git(&ws, &["push"]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:push".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn pull_changes(
    State(state): State<AppState>,
    body: Option<Json<PullReq>>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let req = body.map(|Json(r)| r).unwrap_or_default();
    let mut args: Vec<&str> = vec!["pull"];
    match req.mode.as_deref() {
        Some("rebase") => args.push("--rebase"),
        Some("merge") => args.push("--no-rebase"),
        Some("ff-only") => args.push("--ff-only"),
        _ => {}
    }
    let (ok, stdout, stderr) = run_git(&ws, &args)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:pull".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn fetch_changes(
    State(state): State<AppState>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let (ok, stdout, stderr) = run_git(&ws, &["fetch", "--all", "--prune"])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:fetch".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn get_branches(State(state): State<AppState>) -> Json<BranchesRes> {
    let ws = state.workspace_path().await;
    let cur = current_branch(&ws).await;
    let (_, list, _) = run_git(&ws, &["branch", "--format=%(refname:short)"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let branches = list
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let (_, rlist, _) = run_git(&ws, &["branch", "-r", "--format=%(refname:short)"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let remote_branches = rlist
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.ends_with("/HEAD"))
        .collect();

    let (_, refs_out, _) = run_git(
        &ws,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            BRANCH_REF_FORMAT,
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await
    .unwrap_or((false, String::new(), String::new()));
    let details = parse_branch_refs(&refs_out);

    let current = cur;
    let detached = current == "HEAD";
    let (upstream, upstream_gone) = details
        .iter()
        .find(|b| b.kind == "local" && b.is_current)
        .map(|b| (b.upstream.clone(), b.upstream_gone))
        .unwrap_or_default();

    let (_, head_short, _) = run_git(&ws, &["rev-parse", "--short", "HEAD"])
        .await
        .unwrap_or((false, String::new(), String::new()));

    let (_, remotes_out, _) = run_git(&ws, &["remote"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let remotes: Vec<String> = remotes_out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let (dh_ok, dh_out, _) =
        run_git(&ws, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .await
            .unwrap_or((false, String::new(), String::new()));
    let default_branch = if dh_ok && !dh_out.trim().is_empty() {
        dh_out.trim().trim_start_matches("origin/").to_string()
    } else if details.iter().any(|b| b.kind == "local" && b.name == "main") {
        "main".to_string()
    } else if details.iter().any(|b| b.kind == "local" && b.name == "master") {
        "master".to_string()
    } else {
        String::new()
    };

    let (fh_ok, fh_out, _) = run_git(&ws, &["rev-parse", "--git-path", "FETCH_HEAD"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let last_fetched = if fh_ok {
        let p = std::path::PathBuf::from(fh_out.trim());
        let p = if p.is_absolute() { p } else { ws.join(p) };
        std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
    } else {
        None
    };

    Json(BranchesRes {
        current,
        branches,
        remote_branches,
        details,
        upstream,
        upstream_gone,
        detached,
        head_short: head_short.trim().to_string(),
        default_branch,
        remotes,
        last_fetched,
    })
}

pub async fn checkout_branch(
    State(state): State<AppState>,
    Json(req): Json<CheckoutReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let b = validate_git_ref_arg(&req.branch)?;
    let res = if req.create.unwrap_or(false) {
        if let Some(ref sp) = req.start_point {
            let start = validate_git_ref_arg(sp)?;
            run_git(&ws, &["checkout", "-b", b, start]).await
        } else {
            run_git(&ws, &["checkout", "-b", b]).await
        }
    } else {
        run_git(&ws, &["checkout", b]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:checkout".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn delete_branch(
    State(state): State<AppState>,
    Json(req): Json<DeleteBranchReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let name = validate_git_ref_arg(&req.name)?;
    let flag = if req.force.unwrap_or(false) { "-D" } else { "-d" };
    let (ok, stdout, stderr) = run_git(&ws, &["branch", flag, name])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:branch-delete".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn get_log(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> Json<CommitLogRes> {
    let ws = state.workspace_path().await;
    let limit = q.limit.unwrap_or(80).clamp(1, 500);
    let skip = q.skip.unwrap_or(0);
    let all = q.all.unwrap_or(true);

    let (head_ok, head_out, _) = run_git(&ws, &["rev-parse", "HEAD"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let head_hash = if head_ok {
        head_out.trim().to_string()
    } else {
        String::new()
    };

    let remotes = get_remotes(&ws).await;

    // Fetch limit + 1 to accurately compute `has_more`
    let fetch_n = (limit + 1).to_string();
    let skip_str = skip.to_string();

    let mut args: Vec<String> = vec![
        "log".to_string(),
        "--topo-order".to_string(),
        "--decorate=short".to_string(),
        format!("-n{}", fetch_n),
        format!("--skip={}", skip_str),
        "--format=%H%x1f%h%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ae%x1f%ar%x1f%aI%x1f%at".to_string(),
    ];

    if let Some(ref b) = q.branch {
        let b_trim = b.trim();
        if !b_trim.is_empty() && !b_trim.starts_with('-') {
            args.push(b_trim.to_string());
        } else if all {
            // Stashes are not history; keep them out of the graph.
            args.push("--exclude=refs/stash".to_string());
            args.push("--all".to_string());
        }
    } else if all {
        args.push("--exclude=refs/stash".to_string());
        args.push("--all".to_string());
    }

    let search_lower = q
        .search
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    // When searching, fetch a larger window and filter across hash, message, author, and refs
    if search_lower.is_some() {
        args[3] = "-n500".to_string();
        args[4] = "--skip=0".to_string();
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (ok, out, _) = run_git(&ws, &arg_refs)
        .await
        .unwrap_or((false, String::new(), String::new()));
    if !ok {
        return Json(CommitLogRes {
            commits: Vec::new(),
            has_more: false,
            head_hash,
        });
    }

    let mut items = Vec::new();
    for line in out.lines() {
        let p: Vec<&str> = line.split('\x1f').collect();
        if p.len() >= 10 {
            let hash = p[0].to_string();
            let short_hash = p[1].to_string();
            let parents: Vec<String> = p[2]
                .split_whitespace()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let (refs, ref_is_head) = parse_refs(p[3], &remotes);
            let is_head = ref_is_head || (!head_hash.is_empty() && hash == head_hash);
            let message = p[4].to_string();
            let author = p[5].to_string();
            let author_email = p[6].to_string();
            let date = p[7].to_string();
            let iso_date = p[8].to_string();
            let timestamp = p[9].parse::<i64>().unwrap_or(0);

            if let Some(ref needle) = search_lower {
                let matched = hash.to_lowercase().contains(needle)
                    || short_hash.to_lowercase().contains(needle)
                    || message.to_lowercase().contains(needle)
                    || author.to_lowercase().contains(needle)
                    || refs.iter().any(|r| r.name.to_lowercase().contains(needle));
                if !matched {
                    continue;
                }
            }

            items.push(CommitLogItem {
                hash,
                short_hash,
                parents,
                refs,
                message,
                author,
                author_email,
                date,
                iso_date,
                timestamp,
                is_head,
            });
        }
    }

    let has_more = if search_lower.is_some() {
        false
    } else if items.len() > limit {
        items.truncate(limit);
        true
    } else {
        false
    };

    Json(CommitLogRes {
        commits: items,
        has_more,
        head_hash,
    })
}

pub async fn get_commit_detail(
    State(state): State<AppState>,
    Query(q): Query<CommitQuery>,
) -> Result<Json<CommitDetailRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&q.hash)?;
    let remotes = get_remotes(&ws).await;

    let (ok, meta_out, meta_err) = run_git(
        &ws,
        &[
            "show",
            "-s",
            "--decorate=short",
            "--format=%H%x1f%h%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ae%x1f%ar%x1f%aI%x1f%b",
            hash,
        ],
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if !ok {
        return Err((
            StatusCode::NOT_FOUND,
            if meta_err.is_empty() {
                "Commit not found".to_string()
            } else {
                meta_err
            },
        ));
    }

    let parts: Vec<&str> = meta_out.splitn(10, '\x1f').collect();
    if parts.len() < 10 {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to parse commit metadata".to_string(),
        ));
    }

    let full_hash = parts[0].trim().to_string();
    let short_hash = parts[1].trim().to_string();
    let parents: Vec<String> = parts[2]
        .split_whitespace()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let (refs, _) = parse_refs(parts[3], &remotes);
    let subject = parts[4].to_string();
    let author = parts[5].to_string();
    let author_email = parts[6].to_string();
    let date = parts[7].to_string();
    let iso_date = parts[8].to_string();
    let body = parts[9].trim().to_string();

    let diff_args_base: Vec<&str> = if parents.is_empty() {
        vec!["diff-tree", "--no-commit-id", "-r", "--root", "-M", "-z"]
    } else {
        vec![
            "diff-tree",
            "--no-commit-id",
            "-r",
            "-m",
            "--first-parent",
            "-M",
            "-z",
        ]
    };

    let mut st_args = diff_args_base.clone();
    st_args.push("--name-status");
    st_args.push(&full_hash);
    let (_, st_bytes, _) = run_git_bytes(&ws, &st_args)
        .await
        .unwrap_or((false, Vec::new(), String::new()));

    let mut ns_args = diff_args_base;
    ns_args.push("--numstat");
    ns_args.push(&full_hash);
    let (_, ns_bytes, _) = run_git_bytes(&ws, &ns_args)
        .await
        .unwrap_or((false, Vec::new(), String::new()));

    let files = parse_z_diff_files(&st_bytes, &ns_bytes);
    let total_additions = files.iter().map(|f| f.additions).sum();
    let total_deletions = files.iter().map(|f| f.deletions).sum();

    Ok(Json(CommitDetailRes {
        hash: full_hash,
        short_hash,
        parents,
        refs,
        subject,
        body,
        author,
        author_email,
        date,
        iso_date,
        files,
        total_additions,
        total_deletions,
    }))
}

pub async fn get_commit_diff(
    State(state): State<AppState>,
    Query(q): Query<CommitDiffQuery>,
) -> Result<Json<GitDiffRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&q.hash)?;
    let rel = q.path.replace('\\', "/");
    let old_rel = q
        .old_path
        .as_deref()
        .map(|p| p.replace('\\', "/"))
        .unwrap_or_else(|| rel.clone());

    let parent_ref = format!("{}^1", hash);
    let (has_parent, _, _) = run_git(&ws, &["rev-parse", "--verify", &parent_ref])
        .await
        .unwrap_or((false, String::new(), String::new()));

    let (ok_orig, orig_bytes) = if has_parent {
        let (ok, bytes, _) = run_git_bytes(&ws, &["show", &format!("{}:{}", parent_ref, old_rel)])
            .await
            .unwrap_or((false, Vec::new(), String::new()));
        (ok, bytes)
    } else {
        (false, Vec::new())
    };

    let (ok_mod, mod_bytes, _) = run_git_bytes(&ws, &["show", &format!("{}:{}", hash, rel)])
        .await
        .unwrap_or((false, Vec::new(), String::new()));

    if (ok_orig && is_binary_bytes(&orig_bytes)) || (ok_mod && is_binary_bytes(&mod_bytes)) {
        return Ok(Json(GitDiffRes {
            path: rel,
            staged: false,
            original: String::new(),
            modified: String::new(),
            patch: String::new(),
            binary: true,
        }));
    }

    let (_, patch_out, _) = if has_parent {
        if old_rel != rel {
            run_git(&ws, &["diff", &parent_ref, hash, "--", &old_rel, &rel])
                .await
                .unwrap_or((false, String::new(), String::new()))
        } else {
            run_git(&ws, &["diff", &parent_ref, hash, "--", &rel])
                .await
                .unwrap_or((false, String::new(), String::new()))
        }
    } else {
        run_git(&ws, &["show", "--format=", hash, "--", &rel])
            .await
            .unwrap_or((false, String::new(), String::new()))
    };

    Ok(Json(GitDiffRes {
        path: rel,
        staged: false,
        original: if ok_orig {
            String::from_utf8_lossy(&orig_bytes).to_string()
        } else {
            String::new()
        },
        modified: if ok_mod {
            String::from_utf8_lossy(&mod_bytes).to_string()
        } else {
            String::new()
        },
        patch: patch_out,
        binary: false,
    }))
}

pub async fn get_commit_patch(
    State(state): State<AppState>,
    Query(q): Query<CommitQuery>,
) -> Result<Json<CommitPatchRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&q.hash)?;
    let (ok, out, err) = run_git(&ws, &["show", "--patch", hash])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !ok {
        return Err((StatusCode::BAD_REQUEST, err));
    }
    Ok(Json(CommitPatchRes {
        hash: hash.to_string(),
        patch: out,
    }))
}

pub async fn compare_files(
    State(state): State<AppState>,
    Query(q): Query<CompareFilesQuery>,
) -> Result<Json<Vec<CommitFileChange>>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let base = validate_git_ref_arg(&q.base)?;

    let (_, st_bytes, _) = run_git_bytes(&ws, &["diff", "-M", "-z", "--name-status", base])
        .await
        .unwrap_or((false, Vec::new(), String::new()));
    let (_, ns_bytes, _) = run_git_bytes(&ws, &["diff", "-M", "-z", "--numstat", base])
        .await
        .unwrap_or((false, Vec::new(), String::new()));

    Ok(Json(parse_z_diff_files(&st_bytes, &ns_bytes)))
}

pub async fn compare_diff(
    State(state): State<AppState>,
    Query(q): Query<CompareDiffQuery>,
) -> Result<Json<GitDiffRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let base = validate_git_ref_arg(&q.base)?;
    let rel = q.path.replace('\\', "/");
    let old_rel = q
        .old_path
        .as_deref()
        .map(|p| p.replace('\\', "/"))
        .unwrap_or_else(|| rel.clone());

    let (ok_orig, orig_bytes, _) = run_git_bytes(&ws, &["show", &format!("{}:{}", base, old_rel)])
        .await
        .unwrap_or((false, Vec::new(), String::new()));
    if ok_orig && is_binary_bytes(&orig_bytes) {
        return Ok(Json(GitDiffRes {
            path: rel,
            staged: false,
            original: String::new(),
            modified: String::new(),
            patch: String::new(),
            binary: true,
        }));
    }

    let modified_text = match resolve_safe_path(&ws, &rel) {
        Ok(full) => match tokio::fs::read(&full).await {
            Ok(bytes) => {
                if is_binary_bytes(&bytes) {
                    return Ok(Json(GitDiffRes {
                        path: rel,
                        staged: false,
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

    let (_, patch_out, _) = run_git(&ws, &["diff", base, "--", &rel])
        .await
        .unwrap_or((false, String::new(), String::new()));

    Ok(Json(GitDiffRes {
        path: rel,
        staged: false,
        original: if ok_orig {
            String::from_utf8_lossy(&orig_bytes).to_string()
        } else {
            String::new()
        },
        modified: modified_text,
        patch: patch_out,
        binary: false,
    }))
}

pub async fn cherry_pick(
    State(state): State<AppState>,
    Json(req): Json<CherryPickReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&req.hash)?;
    let mut args = vec!["cherry-pick"];
    if req.no_commit.unwrap_or(false) {
        args.push("-n");
    }
    // Check if commit is a merge commit (>1 parent)
    let (p_ok, p_out, _) = run_git(&ws, &["rev-list", "--parents", "-n", "1", hash])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if p_ok && p_out.split_whitespace().count() > 2 {
        args.push("-m");
        args.push("1");
    }
    args.push(hash);

    let (ok, stdout, stderr) = run_git(&ws, &args)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:cherry-pick".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn revert_commit(
    State(state): State<AppState>,
    Json(req): Json<RevertReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&req.hash)?;
    let mut args = vec!["revert", "--no-edit"];
    if req.no_commit.unwrap_or(false) {
        args.push("-n");
    }
    let (p_ok, p_out, _) = run_git(&ws, &["rev-list", "--parents", "-n", "1", hash])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if p_ok && p_out.split_whitespace().count() > 2 {
        args.push("-m");
        args.push("1");
    }
    args.push(hash);

    let (ok, stdout, stderr) = run_git(&ws, &args)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:revert".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn reset_branch(
    State(state): State<AppState>,
    Json(req): Json<ResetReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let target = validate_git_ref_arg(&req.target)?;
    let flag = match req.mode.as_str() {
        "soft" => "--soft",
        "mixed" => "--mixed",
        "hard" => "--hard",
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Invalid reset mode (expected soft, mixed, or hard)".to_string(),
            ))
        }
    };
    let (ok, stdout, stderr) = run_git(&ws, &["reset", flag, target])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:reset".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn merge_ref(
    State(state): State<AppState>,
    Json(req): Json<MergeReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let target = validate_git_ref_arg(&req.target)?;
    let (ok, stdout, stderr) = run_git(&ws, &["merge", target])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:merge".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn create_tag(
    State(state): State<AppState>,
    Json(req): Json<TagReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let name = validate_git_ref_arg(&req.name)?;
    let target = validate_git_ref_arg(&req.target)?;
    let res = if let Some(msg) = req.message.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        run_git(&ws, &["tag", "-a", name, target, "-m", msg]).await
    } else {
        run_git(&ws, &["tag", name, target]).await
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:tag".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn git_stash(
    State(state): State<AppState>,
    Json(req): Json<StashReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let res = match req.action.as_str() {
        "push" => {
            if let Some(msg) = req.message.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                run_git(&ws, &["stash", "push", "-u", "-m", msg]).await
            } else {
                run_git(&ws, &["stash", "push", "-u"]).await
            }
        }
        "pop" => {
            if let Some(ref idx) = req.index {
                let idx_v = validate_git_ref_arg(idx)?;
                run_git(&ws, &["stash", "pop", idx_v]).await
            } else {
                run_git(&ws, &["stash", "pop"]).await
            }
        }
        "apply" => {
            if let Some(ref idx) = req.index {
                let idx_v = validate_git_ref_arg(idx)?;
                run_git(&ws, &["stash", "apply", idx_v]).await
            } else {
                run_git(&ws, &["stash", "apply"]).await
            }
        }
        "drop" => {
            let idx_v = validate_git_ref_arg(req.index.as_deref().unwrap_or("stash@{0}"))?;
            run_git(&ws, &["stash", "drop", idx_v]).await
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Invalid stash action".to_string(),
            ))
        }
    };
    let (ok, stdout, stderr) = res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:stash".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn git_operation(
    State(state): State<AppState>,
    Json(req): Json<OperationReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let cmd = match req.kind.as_str() {
        "cherry-pick" | "merge" | "revert" | "rebase" => req.kind.as_str(),
        _ => return Err((StatusCode::BAD_REQUEST, "Invalid git operation kind".to_string())),
    };
    let flag = match req.action.as_str() {
        "continue" => "--continue",
        "abort" => "--abort",
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Invalid git operation action".to_string(),
            ))
        }
    };
    let (ok, stdout, stderr) = run_git(&ws, &[cmd, flag])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec![format!("git:{}:{}", cmd, req.action)]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn apply_patch(
    State(state): State<AppState>,
    Json(req): Json<ApplyPatchReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    if req.patch.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Patch cannot be empty".to_string()));
    }
    let mut args = vec!["apply", "--unidiff-zero", "--whitespace=nowarn"];
    if req.cached.unwrap_or(false) {
        args.push("--cached");
    }
    if req.reverse.unwrap_or(false) {
        args.push("-R");
    }
    args.push("-");

    let (ok, stdout, stderr) = run_git_with_stdin(&ws, &args, req.patch.as_bytes())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:apply-patch".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

/// Prefer "origin", else the first configured remote, else "origin".
async fn pick_remote(ws: &Path) -> String {
    let remotes = get_remotes(ws).await;
    if remotes.iter().any(|r| r == "origin") {
        "origin".to_string()
    } else {
        remotes
            .into_iter()
            .next()
            .unwrap_or_else(|| "origin".to_string())
    }
}

async fn run_gh(ws: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = background_command("gh")
        .args(args)
        .current_dir(ws)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .output()
        .await
        .map_err(|e| format!("Failed to run gh: {}", e))?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

#[derive(Deserialize, Default)]
pub struct InitReq {
    pub branch: Option<String>,
}

pub async fn init_repo(
    State(state): State<AppState>,
    body: Option<Json<InitReq>>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let req = body.map(|Json(r)| r).unwrap_or_default();
    let branch = req.branch.unwrap_or_else(|| "main".to_string());
    let branch = validate_git_ref_arg(&branch)?.to_string();
    let (ok, stdout, stderr) = run_git(&ws, &["init", "-b", &branch])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:init".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

#[derive(Serialize)]
pub struct GithubStatusRes {
    pub installed: bool,
    pub authenticated: bool,
    pub login: String,
}

pub async fn github_status(State(state): State<AppState>) -> Json<GithubStatusRes> {
    let ws = state.workspace_path().await;
    let installed = run_gh(&ws, &["--version"])
        .await
        .map(|r| r.0)
        .unwrap_or(false);
    if !installed {
        return Json(GithubStatusRes {
            installed: false,
            authenticated: false,
            login: String::new(),
        });
    }
    let (ok, out, _) = run_gh(&ws, &["api", "user", "--jq", ".login"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    let login = out.trim().to_string();
    Json(GithubStatusRes {
        installed: true,
        authenticated: ok && !login.is_empty(),
        login,
    })
}

#[derive(Deserialize)]
pub struct RemoteAddReq {
    pub name: Option<String>,
    pub url: String,
}

fn valid_remote_url(url: &str) -> bool {
    let ok_scheme = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
        || url.starts_with("git@");
    ok_scheme && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub async fn add_remote(
    State(state): State<AppState>,
    Json(req): Json<RemoteAddReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let name = req.name.as_deref().unwrap_or("origin").trim().to_string();
    let url = req.url.trim().to_string();
    if name.is_empty()
        || name.starts_with('-')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid remote name".to_string()));
    }
    if !valid_remote_url(&url) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Enter a valid remote URL (https://..., ssh://... or git@host:owner/repo.git)"
                .to_string(),
        ));
    }
    let (is_repo, _, _) = run_git(&ws, &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if !is_repo {
        let (ok, stdout, stderr) = run_git(&ws, &["init", "-b", "main"])
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if !ok {
            return Ok(Json(CommandRes { ok, stdout, stderr }));
        }
    }
    let existing = get_remotes(&ws).await;
    let args: Vec<&str> = if existing.iter().any(|r| r == &name) {
        vec!["remote", "set-url", &name, &url]
    } else {
        vec!["remote", "add", &name, &url]
    };
    let (ok, stdout, stderr) = run_git(&ws, &args)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.notify_fs_changed(vec!["git:remote".to_string()]);
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

#[derive(Deserialize)]
pub struct PublishGithubReq {
    pub name: String,
    pub private: bool,
    pub description: Option<String>,
}

/// Create a GitHub repository with `gh` and wire it up as `origin`.
/// Initializes the repo and makes an initial commit first when needed.
pub async fn publish_github(
    State(state): State<AppState>,
    Json(req): Json<PublishGithubReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let name = req.name.trim().to_string();
    let valid_name = !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid_name {
        return Err((
            StatusCode::BAD_REQUEST,
            "Repository name may only contain letters, numbers, '.', '-' and '_'".to_string(),
        ));
    }
    let fail = |msg: &str| {
        Json(CommandRes {
            ok: false,
            stdout: String::new(),
            stderr: msg.to_string(),
        })
    };
    let ise = |e: String| (StatusCode::INTERNAL_SERVER_ERROR, e);

    let gh = github_status(State(state.clone())).await.0;
    if !gh.installed {
        return Ok(fail(
            "GitHub CLI (gh) is not installed. Install it from https://cli.github.com, or add a remote URL instead.",
        ));
    }
    if !gh.authenticated {
        return Ok(fail(
            "GitHub CLI is not signed in. Run `gh auth login` in the terminal, then try again.",
        ));
    }

    let (is_repo, _, _) = run_git(&ws, &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap_or((false, String::new(), String::new()));
    if !is_repo {
        let (ok, _, err) = run_git(&ws, &["init", "-b", "main"]).await.map_err(ise)?;
        if !ok {
            return Ok(fail(&err));
        }
    }

    if !get_remotes(&ws).await.is_empty() {
        return Ok(fail(
            "This repository already has a remote. Use Push / Publish Branch instead.",
        ));
    }

    let has_commits = run_git(&ws, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .map(|r| r.0)
        .unwrap_or(false);
    let mut committed_initial = false;
    if !has_commits {
        let _ = run_git(&ws, &["add", "-A"]).await;
        // `diff --cached --quiet` exits 0 when nothing is staged.
        let nothing_staged = run_git(&ws, &["diff", "--cached", "--quiet"])
            .await
            .map(|r| r.0)
            .unwrap_or(true);
        if !nothing_staged {
            let (ok, out, err) = run_git(&ws, &["commit", "-m", "Initial commit"])
                .await
                .map_err(ise)?;
            if !ok {
                let msg = if err.trim().is_empty() { out } else { err };
                return Ok(fail(&format!(
                    "Could not create the initial commit: {}",
                    msg.trim()
                )));
            }
            committed_initial = true;
        }
    }
    let can_push = has_commits || committed_initial;

    let visibility = if req.private { "--private" } else { "--public" };
    let mut args: Vec<&str> = vec![
        "repo",
        "create",
        &name,
        visibility,
        "--source=.",
        "--remote=origin",
    ];
    let desc = req
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if let Some(d) = desc {
        args.push("--description");
        args.push(d);
    }
    if can_push {
        args.push("--push");
    }
    let (ok, stdout, stderr) = run_gh(&ws, &args).await.map_err(ise)?;
    state.notify_fs_changed(vec!["git:publish".to_string()]);
    let stdout = if ok && !can_push {
        format!(
            "{}\nNo commits yet. Commit your files, then publish the branch.",
            stdout.trim()
        )
    } else {
        stdout
    };
    Ok(Json(CommandRes { ok, stdout, stderr }))
}

pub async fn open_external(
    Json(req): Json<OpenExternalReq>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let url = req.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(|c| c.is_control() || matches!(c, '"' | '\'' | '`' | ';'))
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid external URL".to_string()));
    }

    #[cfg(target_os = "windows")]
    let res = background_command("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let res = background_command("open").arg(url).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let res = background_command("xdg-open").arg(url).spawn();

    match res {
        Ok(_) => Ok(Json(CommandRes {
            ok: true,
            stdout: "Opened in browser".to_string(),
            stderr: String::new(),
        })),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_branch_refs_tracking_gone_and_remote_head() {
        let s = '\u{1f}';
        let raw = [
            format!("refs/heads/main{s}origin/main{s}ahead 2, behind 1{s}*{s}1700000300{s}2 minutes ago{s}Fix: a{s}b"),
            format!("refs/heads/feat/x{s}origin/feat/x{s}gone{s} {s}1700000200{s}1 hour ago{s}Work"),
            format!("refs/heads/local-only{s}{s}{s} {s}1700000100{s}2 days ago{s}Local"),
            format!("refs/remotes/origin/HEAD{s}{s}{s} {s}1700000300{s}2 minutes ago{s}Fix"),
            format!("refs/remotes/origin{s}{s}{s} {s}1700000300{s}2 minutes ago{s}Fix"),
            format!("refs/remotes/origin/main{s}{s}{s} {s}1700000000{s}3 days ago{s}Old"),
        ]
        .join("\n");
        let b = parse_branch_refs(&raw);
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].name, "main");
        assert!(b[0].is_current);
        assert_eq!((b[0].ahead, b[0].behind), (2, 1));
        assert_eq!(b[0].subject, format!("Fix: a{s}b"));
        assert_eq!(b[1].name, "feat/x");
        assert!(b[1].upstream_gone);
        assert_eq!(b[2].upstream, "");
        assert_eq!(b[3].kind, "remote");
        assert_eq!(b[3].name, "origin/main");
    }

    #[test]
    fn test_parse_refs_handles_local_slash_remote_and_tags() {
        let remotes = vec!["origin".to_string()];
        let (refs, is_head) = parse_refs(
            "HEAD -> feat/diff-panel, origin/feat/diff-panel, origin/HEAD, tag: v1.0.0, other/local",
            &remotes,
        );
        assert!(is_head);
        assert_eq!(refs.len(), 4);
        assert_eq!(refs[0].name, "feat/diff-panel");
        assert_eq!(refs[0].kind, "local");
        assert!(refs[0].is_current);
        assert_eq!(refs[1].name, "origin/feat/diff-panel");
        assert_eq!(refs[1].kind, "remote");
        assert_eq!(refs[2].name, "v1.0.0");
        assert_eq!(refs[2].kind, "tag");
        assert_eq!(refs[3].name, "other/local");
        assert_eq!(refs[3].kind, "local");
    }

    #[test]
    fn test_parse_z_diff_files_handles_modify_rename_and_binary() {
        let name_status = b"M\0src/app.ts\0R100\0old/name.rs\0new/name.rs\0A\0assets/logo.png\0";
        let numstat = b"10\t2\tsrc/app.ts\03\t1\t\0old/name.rs\0new/name.rs\0-\t-\tassets/logo.png\0";
        let files = parse_z_diff_files(name_status, numstat);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "src/app.ts");
        assert_eq!(files[0].status, "M");
        assert_eq!((files[0].additions, files[0].deletions, files[0].binary), (10, 2, false));

        assert_eq!(files[1].path, "new/name.rs");
        assert_eq!(files[1].old_path.as_deref(), Some("old/name.rs"));
        assert_eq!(files[1].status, "R");
        assert_eq!((files[1].additions, files[1].deletions), (3, 1));

        assert_eq!(files[2].path, "assets/logo.png");
        assert_eq!(files[2].status, "A");
        assert!(files[2].binary);
    }
}
