use crate::state::{background_command, resolve_safe_path, AppState, Checkpoint};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

/// Resolved `.git` directories for a workspace (handles regular repos, linked worktrees,
/// submodules, and subdirectories inside a repository).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// Per-worktree git dir (`HEAD`, `index`, `MERGE_HEAD`, `FETCH_HEAD`, `rebase-*`).
    pub git_dir: PathBuf,
    /// Shared git dir (`refs/`, `packed-refs`, `config`, `logs/refs/stash`, `reftable/`).
    pub common_dir: PathBuf,
}

/// Pure-Rust git directory discovery (~0.05 ms, zero `git.exe` spawns).
pub fn resolve_git_dirs(ws: &Path) -> Option<GitDirs> {
    if ws.as_os_str().is_empty() {
        return None;
    }
    let mut cur: Option<&Path> = Some(ws);
    while let Some(dir) = cur {
        let dot_git = dir.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else if dot_git.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let first = content.lines().next()?.trim();
            let raw = first.strip_prefix("gitdir:")?.trim();
            let p = PathBuf::from(raw);
            if p.is_absolute() {
                p
            } else {
                dir.join(p)
            }
        } else {
            cur = dir.parent();
            continue;
        };

        let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|rel| {
                let p = PathBuf::from(&rel);
                if p.is_absolute() {
                    p
                } else {
                    git_dir.join(p)
                }
            })
            .unwrap_or_else(|| git_dir.clone());

        return Some(GitDirs { git_dir, common_dir });
    }
    None
}

fn hash_file_or_meta(h: &mut Sha256, label: &str, path: &Path, max_read: u64) {
    h.update(label.as_bytes());
    h.update([0u8]);
    match std::fs::metadata(path) {
        Ok(md) => {
            h.update([1u8]);
            h.update(md.len().to_le_bytes());
            if let Ok(mtime) = md.modified() {
                if let Ok(dur) = mtime.duration_since(UNIX_EPOCH) {
                    h.update(dur.as_secs().to_le_bytes());
                    h.update(dur.subsec_nanos().to_le_bytes());
                }
            }
            if md.is_file() && md.len() <= max_read {
                if let Ok(bytes) = std::fs::read(path) {
                    h.update(&bytes);
                }
            }
        }
        Err(_) => {
            h.update([0u8]);
        }
    }
}

fn hex_prefix8(digest: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for &b in digest.iter().take(8) {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Fast pure-Rust fingerprint of all git refs, HEAD, remotes/tracking config, fetch state,
/// stashes, and in-progress operations. Changes on every ref-mutating git command (`push`,
/// `pull`, `fetch`, `tag`, `merge`, `rebase`, `commit`, `branch`, `checkout`, `reset`,
/// `stash`, `remote`) and stays stable across ordinary working-tree edits and staging.
pub fn compute_refs_fingerprint(dirs: &GitDirs) -> String {
    let mut h = Sha256::new();
    hash_file_or_meta(&mut h, "HEAD", &dirs.git_dir.join("HEAD"), 512);
    hash_file_or_meta(&mut h, "packed-refs", &dirs.common_dir.join("packed-refs"), 64 * 1024);
    hash_file_or_meta(&mut h, "config", &dirs.common_dir.join("config"), 32 * 1024);
    hash_file_or_meta(&mut h, "FETCH_HEAD", &dirs.git_dir.join("FETCH_HEAD"), 4096);
    hash_file_or_meta(
        &mut h,
        "stash-log",
        &dirs.common_dir.join("logs").join("refs").join("stash"),
        16 * 1024,
    );
    hash_file_or_meta(
        &mut h,
        "reftable",
        &dirs.common_dir.join("reftable").join("tables.list"),
        16 * 1024,
    );
    for sentinel in ["CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply"] {
        hash_file_or_meta(&mut h, sentinel, &dirs.git_dir.join(sentinel), 0);
    }

    // Walk loose refs deterministically (skipping transient `*.lock` files).
    let refs_root = dirs.common_dir.join("refs");
    if refs_root.is_dir() {
        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        let mut stack: Vec<(String, PathBuf)> = vec![(String::new(), refs_root)];
        const MAX_LOOSE_REFS: usize = 2000;
        while let Some((prefix, dir)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".lock") {
                    continue;
                }
                let rel = if prefix.is_empty() {
                    name.into_owned()
                } else {
                    format!("{prefix}/{name}")
                };
                let p = entry.path();
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push((rel, p));
                } else {
                    entries.push((rel, p));
                    if entries.len() >= MAX_LOOSE_REFS {
                        break;
                    }
                }
            }
            if entries.len() >= MAX_LOOSE_REFS {
                break;
            }
        }
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for (rel, p) in entries {
            hash_file_or_meta(&mut h, &rel, &p, 256);
        }
    }

    hex_prefix8(&h.finalize())
}

fn config_stamp(dirs: &GitDirs) -> String {
    let mut h = Sha256::new();
    hash_file_or_meta(&mut h, "config", &dirs.common_dir.join("config"), 32 * 1024);
    hex_prefix8(&h.finalize())
}

fn stash_stamp(dirs: &GitDirs) -> Option<String> {
    let ref_p = dirs.common_dir.join("refs").join("stash");
    let log_p = dirs.common_dir.join("logs").join("refs").join("stash");
    let packed_p = dirs.common_dir.join("packed-refs");
    let has_loose = ref_p.exists() || log_p.exists();
    let has_packed = !has_loose
        && std::fs::read_to_string(&packed_p)
            .map(|s| s.lines().any(|l| l.ends_with(" refs/stash")))
            .unwrap_or(false);
    if !has_loose && !has_packed {
        return None;
    }
    let mut h = Sha256::new();
    hash_file_or_meta(&mut h, "refs/stash", &ref_p, 256);
    hash_file_or_meta(&mut h, "logs/refs/stash", &log_p, 16 * 1024);
    if has_packed {
        hash_file_or_meta(&mut h, "packed-refs", &packed_p, 0);
    }
    Some(hex_prefix8(&h.finalize()))
}

fn detect_git_state(dirs: &GitDirs) -> Option<String> {
    if dirs.git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some("cherry-pick".to_string())
    } else if dirs.git_dir.join("MERGE_HEAD").exists() {
        Some("merge".to_string())
    } else if dirs.git_dir.join("REVERT_HEAD").exists() {
        Some("revert".to_string())
    } else if dirs.git_dir.join("rebase-merge").exists() || dirs.git_dir.join("rebase-apply").exists() {
        Some("rebase".to_string())
    } else {
        None
    }
}

/// Resolves a ref (e.g. `refs/heads/main`) to its commit SHA in pure Rust via loose refs or `packed-refs`.
fn resolve_ref_sha(dirs: &GitDirs, ref_name: &str) -> Option<String> {
    let mut loose = dirs.common_dir.clone();
    for part in ref_name.split('/') {
        loose.push(part);
    }
    if let Ok(raw) = std::fs::read_to_string(&loose) {
        let t = raw.trim();
        if let Some(target) = t.strip_prefix("ref:") {
            return resolve_ref_sha(dirs, target.trim());
        }
        if !t.is_empty() && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(t.to_string());
        }
    }
    if let Ok(packed) = std::fs::read_to_string(dirs.common_dir.join("packed-refs")) {
        for line in packed.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            if let Some((sha, name)) = line.split_once(' ') {
                if name.trim() == ref_name {
                    return Some(sha.trim().to_string());
                }
            }
        }
    }
    None
}

/// Reads `(branch_name_or_HEAD, Option<full_head_sha>)` directly from `.git/HEAD` (~0.05 ms).
fn read_head_state(dirs: &GitDirs) -> Option<(String, Option<String>)> {
    let raw = std::fs::read_to_string(dirs.git_dir.join("HEAD")).ok()?;
    let trimmed = raw.trim();
    if let Some(target) = trimmed.strip_prefix("ref:") {
        let ref_name = target.trim();
        let branch = ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(ref_name)
            .to_string();
        let sha = resolve_ref_sha(dirs, ref_name);
        Some((branch, sha))
    } else if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(("HEAD".to_string(), Some(trimmed.to_string())))
    } else {
        None
    }
}

/// Reads `refs/remotes/origin/HEAD` directly in pure Rust (e.g. `ref: refs/remotes/origin/main` -> `"main"`).
fn read_default_branch_symref(dirs: &GitDirs) -> Option<String> {
    let p = dirs.common_dir.join("refs").join("remotes").join("origin").join("HEAD");
    let raw = std::fs::read_to_string(p).ok()?;
    let target = raw.trim().strip_prefix("ref:")?.trim();
    let short = target.strip_prefix("refs/remotes/origin/")?;
    (!short.is_empty()).then(|| short.to_string())
}

/// Parses `[remote "<name>"]` sections and `origin`'s `url` directly from `.git/config` (~0.02 ms),
/// avoiding `git remote` and `git remote get-url origin` spawns. Returns `None` if `.git/config`
/// uses `[include` or `insteadOf` directives that require git's full config resolver.
fn read_git_config_remotes(dirs: &GitDirs) -> Option<(Vec<String>, Option<String>)> {
    let raw = std::fs::read_to_string(dirs.common_dir.join("config")).ok()?;
    let lower = raw.to_ascii_lowercase();
    if lower.contains("[include") || lower.contains("insteadof") {
        return None;
    }
    let mut remotes: Vec<String> = Vec::new();
    let mut origin_url: Option<String> = None;
    let mut cur_remote: Option<String> = None;
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            let inner = t[1..t.len() - 1].trim();
            cur_remote = inner
                .strip_prefix("remote ")
                .and_then(|r| r.trim().strip_prefix('"'))
                .and_then(|r| r.strip_suffix('"'))
                .map(str::to_string);
            if let Some(ref name) = cur_remote {
                if !remotes.contains(name) {
                    remotes.push(name.clone());
                }
            }
            continue;
        }
        if cur_remote.as_deref() == Some("origin") && origin_url.is_none() {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim().eq_ignore_ascii_case("url") {
                    let val = v.trim().trim_matches('"').to_string();
                    if !val.is_empty() {
                        origin_url = Some(val);
                    }
                }
            }
        }
    }
    Some((remotes, origin_url))
}

#[derive(Default)]
struct GitCacheInner {
    workspace: PathBuf,
    remote_url: Option<(String, String)>,
    stashes: Option<(String, Vec<StashItem>)>,
    branches: Option<(String, BranchesRes)>,
    logs_fp: String,
    logs: Vec<((usize, bool, Option<String>), CommitLogRes)>,
    last_dirty: Option<(bool, bool)>,
    recent_fp: Option<(Instant, String)>,
    prewarmed_status: Option<(Instant, GitStatusRes)>,
    prewarmed_diff: Option<(Instant, String, bool, GitDiffRes)>,
}

#[derive(Default)]
pub struct GitCache {
    inner: Mutex<GitCacheInner>,
    status_lock: tokio::sync::Mutex<()>,
}

impl GitCache {
    fn lock_ws(&self, ws: &Path) -> std::sync::MutexGuard<'_, GitCacheInner> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.workspace != ws {
            *g = GitCacheInner {
                workspace: ws.to_path_buf(),
                ..Default::default()
            };
        }
        g
    }

    pub fn invalidate_volatile(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.recent_fp = None;
        g.prewarmed_status = None;
        g.prewarmed_diff = None;
    }

    fn get_or_compute_refs_fingerprint(&self, ws: &Path, dirs: &GitDirs) -> String {
        {
            let g = self.lock_ws(ws);
            if let Some((at, ref fp)) = g.recent_fp {
                if at.elapsed() < Duration::from_millis(450) {
                    return fp.clone();
                }
            }
        }
        let fp = compute_refs_fingerprint(dirs);
        self.lock_ws(ws).recent_fp = Some((Instant::now(), fp.clone()));
        fp
    }

    fn set_prewarmed_status(&self, ws: &Path, status: GitStatusRes) {
        self.lock_ws(ws).prewarmed_status = Some((Instant::now(), status));
    }

    fn take_prewarmed_status(&self, ws: &Path) -> Option<GitStatusRes> {
        let mut g = self.lock_ws(ws);
        let (at, status) = g.prewarmed_status.take()?;
        (at.elapsed() < Duration::from_millis(2500)).then_some(status)
    }

    fn set_prewarmed_diff(&self, ws: &Path, path: String, staged: bool, diff: GitDiffRes) {
        self.lock_ws(ws).prewarmed_diff = Some((Instant::now(), path, staged, diff));
    }

    fn take_prewarmed_diff(&self, ws: &Path, path: &str, staged: bool) -> Option<GitDiffRes> {
        let mut g = self.lock_ws(ws);
        if g.prewarmed_diff
            .as_ref()
            .is_some_and(|(at, p, s, _)| at.elapsed() < Duration::from_millis(3000) && p == path && *s == staged)
        {
            return g.prewarmed_diff.take().map(|(_, _, _, d)| d);
        }
        None
    }

    fn get_remote_url(&self, ws: &Path, stamp: &str) -> Option<String> {
        let g = self.lock_ws(ws);
        g.remote_url
            .as_ref()
            .filter(|(s, _)| s == stamp)
            .map(|(_, v)| v.clone())
    }

    fn set_remote_url(&self, ws: &Path, stamp: String, url: String) {
        self.lock_ws(ws).remote_url = Some((stamp, url));
    }

    fn get_stashes(&self, ws: &Path, stamp: &str) -> Option<Vec<StashItem>> {
        let g = self.lock_ws(ws);
        g.stashes
            .as_ref()
            .filter(|(s, _)| s == stamp)
            .map(|(_, v)| v.clone())
    }

    fn set_stashes(&self, ws: &Path, stamp: String, items: Vec<StashItem>) {
        self.lock_ws(ws).stashes = Some((stamp, items));
    }

    fn get_branches(&self, ws: &Path, fp: &str) -> Option<BranchesRes> {
        let g = self.lock_ws(ws);
        g.branches
            .as_ref()
            .filter(|(f, _)| f == fp)
            .map(|(_, v)| v.clone())
    }

    fn set_branches(&self, ws: &Path, fp: String, res: BranchesRes) {
        self.lock_ws(ws).branches = Some((fp, res));
    }

    fn get_log(
        &self,
        ws: &Path,
        fp: &str,
        limit: usize,
        all: bool,
        branch: &Option<String>,
    ) -> Option<CommitLogRes> {
        let g = self.lock_ws(ws);
        if g.logs_fp != fp {
            return None;
        }
        g.logs
            .iter()
            .find(|((l, a, b), _)| *l == limit && *a == all && b == branch)
            .map(|(_, v)| v.clone())
    }

    fn set_log(
        &self,
        ws: &Path,
        fp: String,
        limit: usize,
        all: bool,
        branch: Option<String>,
        res: CommitLogRes,
    ) {
        let mut g = self.lock_ws(ws);
        if g.logs_fp != fp {
            g.logs.clear();
            g.logs_fp = fp;
        }
        let key = (limit, all, branch);
        if let Some(existing) = g.logs.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = res;
        } else {
            if g.logs.len() >= 6 {
                g.logs.remove(0);
            }
            g.logs.push((key, res));
        }
    }

    fn last_dirty(&self, ws: &Path) -> (bool, bool) {
        // On cold start (`None`), speculatively run `git diff --numstat` in Wave 1 alongside
        // `git status` so dirty repositories avoid a sequential second wave.
        self.lock_ws(ws).last_dirty.unwrap_or((true, false))
    }

    fn set_last_dirty(&self, ws: &Path, unstaged_tracked: bool, staged: bool) {
        self.lock_ws(ws).last_dirty = Some((unstaged_tracked, staged));
    }
}

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
    /// Fingerprint of all git refs, HEAD, config, fetch state, stashes, and merge/rebase state.
    pub refs_fingerprint: String,
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

#[derive(Clone, Serialize)]
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

#[derive(Clone, Serialize)]
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

fn git_exe() -> &'static str {
    static GIT: OnceLock<String> = OnceLock::new();
    GIT.get_or_init(|| {
        #[cfg(windows)]
        {
            let path = std::env::var_os("PATH").unwrap_or_default();
            for dir in std::env::split_paths(&path) {
                let cand = dir.join("git.exe");
                if cand.is_file() {
                    return cand.to_string_lossy().into_owned();
                }
            }
        }
        "git".to_string()
    })
}

/// A `git` command for `ws` with the request-derived `args`, hardened against argument abuse:
/// the `ext::` / `fd::` transports (which run arbitrary commands) are disabled even if a config
/// file enables them, and option
/// overrides that execute programs (`--upload-pack`, `--receive-pack`, `--exec`, `--config-env`)
/// are refused. Handlers still validate refs and put paths after `--`; this is the last line of
/// defense. The value after `-m` is a message and is never treated as an option.
fn git_command(ws: &Path, args: &[&str]) -> Result<tokio::process::Command, String> {
    let mut prev = "";
    for a in args {
        if a.contains('\0') {
            return Err("Invalid git argument".to_string());
        }
        let lower = a.to_ascii_lowercase();
        // Options that run a program or write an arbitrary file. (`--output-indicator-*` are
        // harmless diff options, so `--output` is matched exactly.)
        let runs_or_writes = ["--upload-pack", "--receive-pack", "--exec", "--config-env", "--open-files-in-pager"]
            .iter()
            .any(|bad| lower.starts_with(bad))
            || lower == "--output"
            || lower.starts_with("--output=")
            // `git grep -O<pager>` opens matches in a program (`-o` is --only-matching).
            || (args.first() == Some(&"grep") && a.starts_with("-O"));
        if prev != "-m" && runs_or_writes {
            return Err(format!("Refusing unsafe git argument: {a}"));
        }
        prev = a;
    }
    let mut cmd = background_command(git_exe());
    // Read-only commands (status, diff) must not refresh and rewrite `.git/index`: that write is
    // itself a watcher event, so every refresh would trigger another one, and it contends with
    // user-run git commands on `index.lock`. Writes that need the lock still take it.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.args(["-c", "protocol.ext.allow=never", "-c", "protocol.fd.allow=never"])
        .args(args)
        .current_dir(ws);
    // SSH key passphrases (fetch/push, SSH commit signing) and HTTPS credentials are asked in
    // an in-app dialog instead of failing for lack of a terminal.
    crate::askpass::apply_env(&mut cmd);
    Ok(cmd)
}

async fn run_git(ws: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = git_command(ws, args)?
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

async fn run_git_bytes(ws: &Path, args: &[&str]) -> Result<(bool, Vec<u8>, String), String> {
    let output = git_command(ws, args)?
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
    let mut child = git_command(ws, args)?
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
            let b = branch.trim();
            let name = b.strip_prefix("refs/heads/").unwrap_or(b);
            refs.push(GitRef {
                name: name.to_string(),
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
            let t = tag.trim();
            let name = t.strip_prefix("refs/tags/").unwrap_or(t);
            refs.push(GitRef {
                name: name.to_string(),
                kind: "tag".to_string(),
                is_current: false,
            });
        } else if tok.ends_with("/HEAD") || tok.contains(" -> ") || tok == "refs/stash" {
            // Skip symbolic remote HEAD pointers (origin/HEAD) and stash ref.
            continue;
        } else if let Some(local) = tok.strip_prefix("refs/heads/") {
            refs.push(GitRef {
                name: local.to_string(),
                kind: "local".to_string(),
                is_current: false,
            });
        } else if let Some(remote) = tok.strip_prefix("refs/remotes/") {
            refs.push(GitRef {
                name: remote.to_string(),
                kind: "remote".to_string(),
                is_current: false,
            });
        } else if let Some(tag) = tok.strip_prefix("refs/tags/") {
            refs.push(GitRef {
                name: tag.to_string(),
                kind: "tag".to_string(),
                is_current: false,
            });
        } else if tok.starts_with("refs/") {
            // Ignore non-branch/non-tag refs such as refs/notes/* or refs/replace/*.
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

/// Untracked files above this size are not read just to count their lines.
const MAX_UNTRACKED_COUNT_BYTES: u64 = 1024 * 1024;
/// At most this many untracked files are line-counted per status refresh...
const MAX_UNTRACKED_COUNT_FILES: usize = 500;
/// ...and at most this many bytes read in total.
const MAX_UNTRACKED_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Default)]
struct StatusV2 {
    branch: String,
    head_hash: String,
    ahead: usize,
    behind: usize,
    /// (index_status, worktree_status, path) with ' ' for "unchanged".
    entries: Vec<(char, char, String)>,
}

/// Parse `git status --porcelain=v2 --branch -z -uall` output.
fn parse_status_v2(bytes: &[u8]) -> StatusV2 {
    let mut out = StatusV2::default();
    let mut detached = false;
    let mut toks = bytes.split(|&b| b == 0).map(|t| String::from_utf8_lossy(t).into_owned());
    let xy = |s: &str| {
        let mut c = s.chars();
        let m = |ch: Option<char>| match ch {
            Some('.') | None => ' ',
            Some(c) => c,
        };
        (m(c.next()), m(c.next()))
    };
    while let Some(tok) = toks.next() {
        if tok.is_empty() {
            continue;
        }
        if let Some(rest) = tok.strip_prefix("# ") {
            if let Some(v) = rest.strip_prefix("branch.oid ") {
                if v != "(initial)" {
                    out.head_hash = v.trim().to_string();
                }
            } else if let Some(v) = rest.strip_prefix("branch.head ") {
                if v == "(detached)" {
                    detached = true;
                } else {
                    out.branch = v.trim().to_string();
                }
            } else if let Some(v) = rest.strip_prefix("branch.ab ") {
                let mut it = v.split_whitespace();
                out.ahead = it.next().and_then(|s| s.trim_start_matches('+').parse().ok()).unwrap_or(0);
                out.behind = it.next().and_then(|s| s.trim_start_matches('-').parse().ok()).unwrap_or(0);
            }
            continue;
        }
        match tok.as_bytes()[0] {
            b'1' => {
                let f: Vec<&str> = tok.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let (x, y) = xy(f[1]);
                    out.entries.push((x, y, f[8].to_string()));
                }
            }
            b'2' => {
                let f: Vec<&str> = tok.splitn(10, ' ').collect();
                let _orig = toks.next(); // original path follows as its own NUL-terminated token
                if f.len() == 10 {
                    let (x, y) = xy(f[1]);
                    out.entries.push((x, y, f[9].to_string()));
                }
            }
            b'u' => {
                let f: Vec<&str> = tok.splitn(11, ' ').collect();
                if f.len() == 11 {
                    let (x, y) = xy(f[1]);
                    out.entries.push((x, y, f[10].to_string()));
                }
            }
            b'?' => {
                if let Some(p) = tok.strip_prefix("? ") {
                    out.entries.push(('?', '?', p.to_string()));
                }
            }
            _ => {}
        }
    }
    if detached || out.branch.is_empty() {
        out.branch = "HEAD".to_string();
    }
    out
}

/// Line counts for untracked files (skips large / binary / non-UTF-8 files).
fn count_untracked_lines(ws: &Path, paths: Vec<String>) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    // Bound the work per status refresh: a tree of thousands of untracked files (build output,
    // datasets) must not make every refresh read hundreds of MB.
    let mut budget = MAX_UNTRACKED_TOTAL_BYTES;
    for p in paths.into_iter().take(MAX_UNTRACKED_COUNT_FILES) {
        let Ok(full) = resolve_safe_path(ws, &p) else { continue };
        let len = match std::fs::metadata(&full) {
            Ok(md) if md.is_file() && md.len() <= MAX_UNTRACKED_COUNT_BYTES => md.len(),
            _ => continue,
        };
        if len > budget {
            continue; // too big for what is left; smaller files after it may still fit
        }
        // Count newline bytes directly: no UTF-8 validation / String allocation. A NUL in the
        // first block marks a binary file (not counted).
        let Ok(bytes) = std::fs::read(&full) else { continue };
        budget -= len.min(budget);
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let nl = bytes.iter().filter(|&&b| b == b'\n').count();
        m.insert(p, nl + usize::from(bytes.last().is_some_and(|&b| b != b'\n')));
    }
    m
}

pub async fn get_status(
    State(state): State<AppState>,
) -> Result<Json<GitStatusRes>, (StatusCode, String)> {
    let _status_guard = state.git_cache.status_lock.lock().await;
    let ws = state.workspace_path().await;
    if let Some(mut prewarmed) = state.git_cache.take_prewarmed_status(&ws) {
        prewarmed.checkpoints = state.checkpoints.read().await.clone();
        return Ok(Json(prewarmed));
    }
    compute_status(&state, &ws).await.map(Json)
}

async fn compute_status(
    state: &AppState,
    ws: &Path,
) -> Result<GitStatusRes, (StatusCode, String)> {
    let checkpoints = state.checkpoints.read().await.clone();

    let ws_meta = ws.to_path_buf();
    let cached_remote_check = state.git_cache.clone();
    let meta = tokio::task::spawn_blocking(move || {
        let dirs = resolve_git_dirs(&ws_meta)?;
        let cfg_st = config_stamp(&dirs);
        let st_st = stash_stamp(&dirs);
        let direct_origin = if cached_remote_check.get_remote_url(&ws_meta, &cfg_st).is_none() {
            read_git_config_remotes(&dirs).map(|(_, orig)| orig.unwrap_or_default())
        } else {
            None
        };
        Some((dirs, cfg_st, st_st, direct_origin))
    })
    .await
    .unwrap_or(None);

    let Some((dirs, cfg_stamp, stash_st, direct_origin)) = meta else {
        return Ok(GitStatusRes {
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
            refs_fingerprint: String::new(),
        });
    };

    if let Some(url) = direct_origin {
        state.git_cache.set_remote_url(ws, cfg_stamp.clone(), url);
    }
    let cached_remote = state.git_cache.get_remote_url(ws, &cfg_stamp);
    let cached_stashes = match &stash_st {
        None => Some(Vec::new()),
        Some(st) => state.git_cache.get_stashes(ws, st),
    };
    let (hint_unstaged, hint_staged) = state.git_cache.last_dirty(ws);

    // Run `git status` alongside only the queries that are uncached or hinted dirty.
    let (status, mut unstaged_stat, mut staged_stat, remote_res, stash_res) = tokio::join!(
        run_git_bytes(ws, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]),
        async {
            if hint_unstaged {
                Some(run_git(ws, &["diff", "--numstat"]).await)
            } else {
                None
            }
        },
        async {
            if hint_staged {
                Some(run_git(ws, &["diff", "--cached", "--numstat"]).await)
            } else {
                None
            }
        },
        async {
            if cached_remote.is_none() {
                Some(run_git(ws, &["remote", "get-url", "origin"]).await)
            } else {
                None
            }
        },
        async {
            if cached_stashes.is_none() {
                Some(run_git(ws, &["stash", "list", "--format=%gd%x1f%gs%x1f%ar"]).await)
            } else {
                None
            }
        },
    );

    // If `.git` exists on disk, a non-zero `git status` is usually a transient lock/merge race:
    // retry once before returning 503 so the UI retries instead of flashing "No Git Repository".
    let status_bytes = match status {
        Ok((true, out, _)) => out,
        _ => {
            tokio::time::sleep(std::time::Duration::from_millis(90)).await;
            match run_git_bytes(ws, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).await {
                Ok((true, out, _)) => out,
                _ => {
                    if resolve_git_dirs(ws).is_none() {
                        return Ok(GitStatusRes {
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
                            refs_fingerprint: String::new(),
                        });
                    }
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Git status temporarily busy".to_string(),
                    ));
                }
            }
        }
    };
    let parsed = parse_status_v2(&status_bytes);

    let has_unstaged_tracked = parsed.entries.iter().any(|(x, y, _)| !(*x == '?' && *y == '?') && *y != ' ');
    let has_staged = parsed.entries.iter().any(|(x, y, _)| !(*x == '?' && *y == '?') && *x != ' ');
    state.git_cache.set_last_dirty(ws, has_unstaged_tracked, has_staged);

    let untracked_paths: Vec<String> = parsed
        .entries
        .iter()
        .filter(|(x, y, _)| *x == '?' && *y == '?')
        .map(|(_, _, p)| p.clone())
        .collect();

    // Compute any missing numstats + untracked line counts + post-status refs_fingerprint in parallel.
    let need_unstaged = has_unstaged_tracked && unstaged_stat.is_none();
    let need_staged = has_staged && staged_stat.is_none();
    let ws2 = ws.to_path_buf();
    let dirs_post = dirs.clone();
    let git_cache_fp = state.git_cache.clone();
    let (extra_unstaged, extra_staged, (untracked_lines, git_state, refs_fingerprint)) = tokio::join!(
        async {
            if need_unstaged {
                Some(run_git(ws, &["diff", "--numstat"]).await)
            } else {
                None
            }
        },
        async {
            if need_staged {
                Some(run_git(ws, &["diff", "--cached", "--numstat"]).await)
            } else {
                None
            }
        },
        async {
            tokio::task::spawn_blocking(move || {
                let ul = if untracked_paths.is_empty() {
                    HashMap::new()
                } else {
                    count_untracked_lines(&ws2, untracked_paths)
                };
                let gs = detect_git_state(&dirs_post);
                let fp = git_cache_fp.get_or_compute_refs_fingerprint(&ws2, &dirs_post);
                (ul, gs, fp)
            })
            .await
            .unwrap_or_default()
        },
    );
    if extra_unstaged.is_some() {
        unstaged_stat = extra_unstaged;
    }
    if extra_staged.is_some() {
        staged_stat = extra_staged;
    }

    let remote_url = if let Some(u) = cached_remote {
        u
    } else {
        let u = remote_res
            .and_then(|r| r.ok())
            .filter(|(ok, _, _)| *ok)
            .map(|(_, o, _)| o.trim().to_string())
            .unwrap_or_default();
        state.git_cache.set_remote_url(ws, cfg_stamp, u.clone());
        u
    };

    let stashes = if let Some(s) = cached_stashes {
        s
    } else {
        let mut list = Vec::new();
        if let Some(Ok((true, st_out, _))) = stash_res {
            for line in st_out.lines() {
                let p: Vec<&str> = line.split('\x1f').collect();
                if p.len() >= 3 {
                    list.push(StashItem {
                        index: p[0].to_string(),
                        message: p[1].to_string(),
                        date: p[2].to_string(),
                    });
                }
            }
        }
        if let Some(st) = stash_st {
            state.git_cache.set_stashes(ws, st, list.clone());
        }
        list
    };

    let mut stats = if has_unstaged_tracked {
        parse_numstat(&unstaged_stat.and_then(|r| r.ok()).map(|r| r.1).unwrap_or_default())
    } else {
        HashMap::new()
    };
    if has_staged {
        for (k, (a, d)) in parse_numstat(&staged_stat.and_then(|r| r.ok()).map(|r| r.1).unwrap_or_default()) {
            let e = stats.entry(k).or_insert((0, 0));
            e.0 += a;
            e.1 += d;
        }
    }

    let mut files = Vec::with_capacity(parsed.entries.len());
    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;
    for (x, y, path) in parsed.entries {
        let raw_path = path.replace('\\', "/");
        let untracked = x == '?' && y == '?';
        let staged = !untracked && x != ' ';
        let unstaged = !untracked && y != ' ';
        let (mut adds, dels) = stats.get(&raw_path).copied().unwrap_or((0, 0));
        if untracked {
            adds = untracked_lines.get(&path).copied().unwrap_or(0);
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

    Ok(GitStatusRes {
        is_repo: true,
        branch: parsed.branch,
        head_hash: parsed.head_hash,
        ahead: parsed.ahead,
        behind: parsed.behind,
        remote_url,
        state: git_state,
        files,
        total_additions,
        total_deletions,
        checkpoints,
        stashes,
        refs_fingerprint,
    })
}

pub async fn get_diff(
    State(state): State<AppState>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<GitDiffRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let rel = q.path.replace('\\', "/");
    let staged = q.staged.unwrap_or(false);
    if let Some(prewarmed) = state.git_cache.take_prewarmed_diff(&ws, &rel, staged) {
        return Ok(Json(prewarmed));
    }
    compute_diff(&ws, rel, staged).await.map(Json)
}

async fn compute_diff(
    ws: &Path,
    rel: String,
    staged: bool,
) -> Result<GitDiffRes, (StatusCode, String)> {
    let (original, modified) = if staged {
        let head_spec = format!("HEAD:{}", rel);
        let idx_spec = format!(":{}", rel);
        let head_args = ["show", head_spec.as_str()];
        let idx_args = ["show", idx_spec.as_str()];
        let (orig_res, mod_res) = tokio::join!(
            run_git_bytes(ws, &head_args),
            run_git_bytes(ws, &idx_args),
        );
        let (ok_orig, orig_bytes, _) = orig_res.unwrap_or((false, Vec::new(), String::new()));
        let (ok_mod, mod_bytes, _) = mod_res.unwrap_or((false, Vec::new(), String::new()));
        if (ok_orig && is_binary_bytes(&orig_bytes)) || (ok_mod && is_binary_bytes(&mod_bytes)) {
            return Ok(GitDiffRes {
                path: rel,
                staged,
                original: String::new(),
                modified: String::new(),
                patch: String::new(),
                binary: true,
            });
        }
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
        )
    } else {
        // Compare index (if file is staged too, VS Code compares index vs working tree) or HEAD vs working tree.
        let idx_spec = format!(":{}", rel);
        let idx_args = ["show", idx_spec.as_str()];
        let safe_full = resolve_safe_path(ws, &rel).ok();
        let (idx_res, wt_bytes) = tokio::join!(
            run_git_bytes(ws, &idx_args),
            async {
                match safe_full {
                    Some(full) => tokio::fs::read(&full).await.ok(),
                    None => None,
                }
            },
        );
        let (ok_idx, idx_bytes, _) = idx_res.unwrap_or((false, Vec::new(), String::new()));
        let (orig_ok, orig_bytes) = if ok_idx {
            (true, idx_bytes)
        } else {
            let head_spec = format!("HEAD:{}", rel);
            let (ok_head, head_bytes, _) = run_git_bytes(ws, &["show", &head_spec])
                .await
                .unwrap_or((false, Vec::new(), String::new()));
            (ok_head, head_bytes)
        };
        if (orig_ok && is_binary_bytes(&orig_bytes))
            || wt_bytes.as_deref().map(is_binary_bytes).unwrap_or(false)
        {
            return Ok(GitDiffRes {
                path: rel,
                staged,
                original: String::new(),
                modified: String::new(),
                patch: String::new(),
                binary: true,
            });
        }
        let original_text = if orig_ok {
            String::from_utf8_lossy(&orig_bytes).to_string()
        } else {
            String::new()
        };
        let modified_text = wt_bytes
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        (original_text, modified_text)
    };

    Ok(GitDiffRes {
        path: rel,
        staged,
        original,
        modified,
        patch: String::new(),
        binary: false,
    })
}

/// Pre-warms git status, branches, commit log, and the first modified file's diff in parallel
/// with WebView2 startup so the UI's initial requests hit warm memory instead of waiting on `git.exe`.
pub fn prewarm_workspace(state: AppState) {
    tokio::spawn(async move {
        let ws = state.workspace_path().await;
        if ws.as_os_str().is_empty() {
            return;
        }
        let status_fut = async {
            let _guard = state.git_cache.status_lock.lock().await;
            if let Ok(status) = compute_status(&state, &ws).await {
                let first = status
                    .files
                    .first()
                    .map(|f| (f.path.clone(), f.staged && !f.unstaged));
                state.git_cache.set_prewarmed_status(&ws, status);
                if let Some((path, staged)) = first {
                    if let Ok(diff) = compute_diff(&ws, path.clone(), staged).await {
                        state.git_cache.set_prewarmed_diff(&ws, path, staged, diff);
                    }
                }
            }
        };
        let branches_fut = get_branches(State(state.clone()));
        let log_fut = get_log(
            State(state.clone()),
            Query(LogQuery {
                limit: Some(60),
                skip: Some(0),
                all: Some(true),
                search: None,
                branch: None,
            }),
        );
        let _ = tokio::join!(status_fut, branches_fut, log_fut);
    });
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

static AUTO_FETCH_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct AutoFetchGuard;
impl Drop for AutoFetchGuard {
    fn drop(&mut self) {
        AUTO_FETCH_BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Serialize)]
pub struct AutoFetchRes {
    pub ok: bool,
    /// Remote branch heads differed from our remote-tracking refs (so a fetch ran).
    pub changed: bool,
    pub stderr: String,
}

/// Runs `git args` non-interactively with a hard timeout (process is killed on expiry).
async fn run_git_bounded(ws: &Path, args: &[&str], secs: u64) -> Result<(bool, String, String), String> {
    let mut cmd = git_command(ws, args)?;
    // Background calls (auto-fetch) must never pop up a credential dialog.
    crate::askpass::disable_env(&mut cmd);
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(std::time::Duration::from_secs(secs), cmd.output()).await {
        Ok(Ok(o)) => Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )),
        Ok(Err(e)) => Err(format!("Failed to run git: {e}")),
        Err(_) => Err(format!("git {} timed out", args.first().copied().unwrap_or(""))),
    }
}

/// Quiet background sync check driven by the UI so remote updates show up without a manual
/// Fetch. Cheap first: `git ls-remote --heads` per remote is compared with the local
/// `refs/remotes/<remote>/*`, and a full fetch only runs for remotes whose heads differ.
/// Single-flight (overlapping calls are skipped), never prompts for credentials, never runs
/// `gc`, every git call is time-bounded. Offline/auth failures return `ok: false`. The file
/// watcher sees the resulting ref changes, so the UI refreshes on its own.
pub async fn auto_fetch(State(state): State<AppState>) -> Json<AutoFetchRes> {
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering;
    let res = |ok: bool, changed: bool, stderr: &str| {
        Json(AutoFetchRes { ok, changed, stderr: stderr.to_string() })
    };
    if AUTO_FETCH_BUSY.swap(true, Ordering::AcqRel) {
        return res(true, false, "busy");
    }
    let _guard = AutoFetchGuard;
    let ws = state.workspace_path().await;
    let remotes: Vec<String> =
        get_remotes(&ws).await.into_iter().filter(|r| !r.starts_with('-')).collect();
    if remotes.is_empty() {
        return res(true, false, "no remotes");
    }

    let mut stale: Vec<String> = Vec::new();
    for remote in &remotes {
        let (ok, out, err) = match run_git_bounded(&ws, &["ls-remote", "--heads", remote], 20).await {
            Ok(r) => r,
            Err(e) => return res(false, false, &e),
        };
        if !ok {
            return res(false, false, &err);
        }
        let remote_heads: BTreeMap<String, String> = out
            .lines()
            .filter_map(|l| {
                let (sha, name) = l.split_once('\t')?;
                Some((name.strip_prefix("refs/heads/")?.to_string(), sha.to_string()))
            })
            .collect();
        let prefix = format!("refs/remotes/{remote}/");
        let local = run_git_bounded(
            &ws,
            &["for-each-ref", "--format=%(objectname)%09%(refname)", &prefix],
            10,
        )
        .await
        .map(|(_, o, _)| o)
        .unwrap_or_default();
        let local_heads: BTreeMap<String, String> = local
            .lines()
            .filter_map(|l| {
                let (sha, name) = l.split_once('\t')?;
                let name = name.strip_prefix(&prefix)?;
                (name != "HEAD").then(|| (name.to_string(), sha.to_string()))
            })
            .collect();
        if remote_heads != local_heads {
            stale.push(remote.clone());
        }
    }
    if stale.is_empty() {
        return res(true, false, "");
    }

    let mut errors = String::new();
    let mut all_ok = true;
    for remote in &stale {
        match run_git_bounded(
            &ws,
            &["-c", "gc.auto=0", "-c", "maintenance.auto=false", "fetch", "--prune", "--quiet", remote],
            60,
        )
        .await
        {
            Ok((ok, _, err)) => {
                if !ok {
                    all_ok = false;
                    errors.push_str(&err);
                }
            }
            Err(e) => {
                all_ok = false;
                errors.push_str(&e);
            }
        }
    }
    res(all_ok, true, &errors)
}

pub async fn get_branches(State(state): State<AppState>) -> Json<BranchesRes> {
    let ws = state.workspace_path().await;
    let ws_meta = ws.clone();
    let git_cache_fp = state.git_cache.clone();
    let meta = tokio::task::spawn_blocking(move || {
        let dirs = resolve_git_dirs(&ws_meta)?;
        let fp = git_cache_fp.get_or_compute_refs_fingerprint(&ws_meta, &dirs);
        let head = read_head_state(&dirs);
        let default_sym = read_default_branch_symref(&dirs);
        let direct_remotes = read_git_config_remotes(&dirs).map(|(r, _)| r);
        let last_fetched = std::fs::metadata(dirs.git_dir.join("FETCH_HEAD"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Some((fp, head, default_sym, direct_remotes, last_fetched))
    })
    .await
    .unwrap_or(None);

    let Some((refs_fp, head_state, default_sym, direct_remotes, last_fetched)) = meta else {
        return Json(BranchesRes {
            current: String::new(),
            branches: Vec::new(),
            remote_branches: Vec::new(),
            details: Vec::new(),
            upstream: String::new(),
            upstream_gone: false,
            detached: false,
            head_short: String::new(),
            default_branch: String::new(),
            remotes: Vec::new(),
            last_fetched: None,
        });
    };

    if let Some(cached) = state.git_cache.get_branches(&ws, &refs_fp) {
        return Json(cached);
    }

    // `for-each-ref` provides all local+remote branch details; remotes come directly from `.git/config`
    // (falling back to `git remote` only when config uses `[include` or `insteadOf`).
    let none = || (false, String::new(), String::new());
    let (refs_out, remotes_out) = tokio::join!(
        run_git(
            &ws,
            &["for-each-ref", "--sort=-committerdate", BRANCH_REF_FORMAT, "refs/heads", "refs/remotes"],
        ),
        async {
            if direct_remotes.is_none() {
                Some(run_git(&ws, &["remote"]).await)
            } else {
                None
            }
        },
    );

    let (_, refs_out, _) = refs_out.unwrap_or_else(|_| none());
    let details = parse_branch_refs(&refs_out);

    let mut branches: Vec<String> = details
        .iter()
        .filter(|b| b.kind == "local")
        .map(|b| b.name.clone())
        .collect();
    branches.sort();

    let mut remote_branches: Vec<String> = details
        .iter()
        .filter(|b| b.kind == "remote")
        .map(|b| b.name.clone())
        .collect();
    remote_branches.sort();

    let (current, head_sha) = match head_state {
        Some((cur, sha)) => (cur, sha),
        None => (current_branch(&ws).await, None),
    };
    let detached = current == "HEAD";
    let head_short = match head_sha {
        Some(sha) => sha.chars().take(7).collect(),
        None if !details.is_empty() => run_git(&ws, &["rev-parse", "--short", "HEAD"])
            .await
            .ok()
            .filter(|(ok, _, _)| *ok)
            .map(|(_, o, _)| o.trim().to_string())
            .unwrap_or_default(),
        None => String::new(),
    };

    let (upstream, upstream_gone) = details
        .iter()
        .find(|b| b.kind == "local" && b.is_current)
        .map(|b| (b.upstream.clone(), b.upstream_gone))
        .unwrap_or_default();

    let remotes: Vec<String> = match direct_remotes {
        Some(r) => r,
        None => {
            let (_, remotes_out, _) = remotes_out.and_then(|r| r.ok()).unwrap_or_else(none);
            remotes_out
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }
    };

    let default_branch = if let Some(sym) = default_sym {
        sym
    } else if details.iter().any(|b| b.kind == "local" && b.name == "main") {
        "main".to_string()
    } else if details.iter().any(|b| b.kind == "local" && b.name == "master") {
        "master".to_string()
    } else {
        String::new()
    };

    let res = BranchesRes {
        current,
        branches,
        remote_branches,
        details,
        upstream,
        upstream_gone,
        detached,
        head_short,
        default_branch,
        remotes,
        last_fetched,
    };
    state.git_cache.set_branches(&ws, refs_fp, res.clone());
    Json(res)
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

/// `git remote prune` for every remote: drops remote-tracking refs whose branch
/// no longer exists on the remote. Local branches are never touched.
pub async fn prune_remote_branches(
    State(state): State<AppState>,
) -> Result<Json<CommandRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let (ok, remotes_out, stderr) = run_git(&ws, &["remote"])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !ok {
        return Ok(Json(CommandRes { ok, stdout: String::new(), stderr }));
    }
    let mut all_ok = true;
    let mut stdout = String::new();
    let mut errors = String::new();
    for remote in remotes_out
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty() && !r.starts_with('-'))
    {
        let (ok, out, err) = run_git(&ws, &["remote", "prune", remote])
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        all_ok &= ok;
        stdout.push_str(&out);
        errors.push_str(&err);
    }
    state.notify_fs_changed(vec!["git:branch-delete".to_string()]);
    Ok(Json(CommandRes { ok: all_ok, stdout, stderr: errors }))
}

pub async fn get_log(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> Json<CommitLogRes> {
    let ws = state.workspace_path().await;
    let limit = q.limit.unwrap_or(80).clamp(1, 500);
    let skip = q.skip.unwrap_or(0);
    let all = q.all.unwrap_or(true);
    let branch_key = q
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('-'))
        .map(str::to_string);
    let search_lower = q
        .search
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    let ws_meta = ws.clone();
    let git_cache_fp = state.git_cache.clone();
    let meta = tokio::task::spawn_blocking(move || {
        let dirs = resolve_git_dirs(&ws_meta)?;
        let fp = git_cache_fp.get_or_compute_refs_fingerprint(&ws_meta, &dirs);
        let head_sha = read_head_state(&dirs).and_then(|(_, sha)| sha);
        Some((fp, head_sha))
    })
    .await
    .unwrap_or(None);

    let Some((refs_fp, head_sha)) = meta else {
        return Json(CommitLogRes {
            commits: Vec::new(),
            has_more: false,
            head_hash: String::new(),
        });
    };

    let cacheable = skip == 0 && search_lower.is_none();
    if cacheable {
        if let Some(cached) = state.git_cache.get_log(&ws, &refs_fp, limit, all, &branch_key) {
            return Json(cached);
        }
    }

    let mut head_hash = head_sha.unwrap_or_default();

    // Fetch limit + 1 to accurately compute `has_more`
    let fetch_n = (limit + 1).to_string();
    let skip_str = skip.to_string();

    // `--decorate=full` tags refs unambiguously with `refs/heads/`, `refs/remotes/`, `refs/tags/`,
    // eliminating the extra `git remote` and `rev-parse HEAD` spawns (1 git.exe instead of 3).
    let mut args: Vec<String> = vec![
        "log".to_string(),
        "--topo-order".to_string(),
        "--decorate=full".to_string(),
        format!("-n{}", fetch_n),
        format!("--skip={}", skip_str),
        "--format=%H%x1f%h%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ae%x1f%ar%x1f%aI%x1f%at".to_string(),
    ];

    if let Some(ref b) = branch_key {
        args.push(b.clone());
    } else if all {
        // Stashes are not history; keep them out of the graph.
        args.push("--exclude=refs/stash".to_string());
        args.push("--all".to_string());
    }

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
            let (refs, ref_is_head) = parse_refs(p[3], &[]);
            if ref_is_head && head_hash.is_empty() {
                head_hash = hash.clone();
            }
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

    let res = CommitLogRes {
        commits: items,
        has_more,
        head_hash,
    };
    if cacheable {
        state
            .git_cache
            .set_log(&ws, refs_fp, limit, all, branch_key, res.clone());
    }
    Json(res)
}

pub async fn get_commit_detail(
    State(state): State<AppState>,
    Query(q): Query<CommitQuery>,
) -> Result<Json<CommitDetailRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let hash = validate_git_ref_arg(&q.hash)?;

    // Run metadata + name-status + numstat concurrently (1 wave instead of 4 sequential spawns).
    let meta_args = [
        "show",
        "-s",
        "--decorate=full",
        "--format=%H%x1f%h%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ae%x1f%ar%x1f%aI%x1f%b",
        hash,
    ];
    let st_args = [
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "-m",
        "--first-parent",
        "-M",
        "-z",
        "--name-status",
        hash,
    ];
    let ns_args = [
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "-m",
        "--first-parent",
        "-M",
        "-z",
        "--numstat",
        hash,
    ];
    let (meta_res, st_res, ns_res) = tokio::join!(
        run_git(&ws, &meta_args),
        run_git_bytes(&ws, &st_args),
        run_git_bytes(&ws, &ns_args),
    );

    let (ok, meta_out, meta_err) = meta_res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    let (refs, _) = parse_refs(parts[3], &[]);
    let subject = parts[4].to_string();
    let author = parts[5].to_string();
    let author_email = parts[6].to_string();
    let date = parts[7].to_string();
    let iso_date = parts[8].to_string();
    let body = parts[9].trim().to_string();

    let (_, st_bytes, _) = st_res.unwrap_or((false, Vec::new(), String::new()));
    let (_, ns_bytes, _) = ns_res.unwrap_or((false, Vec::new(), String::new()));
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
    let orig_spec = format!("{}:{}", parent_ref, old_rel);
    let mod_spec = format!("{}:{}", hash, rel);
    let orig_args = ["show", orig_spec.as_str()];
    let mod_args = ["show", mod_spec.as_str()];

    // Run parent blob and commit blob concurrently.
    let (orig_res, mod_res) = tokio::join!(
        run_git_bytes(&ws, &orig_args),
        run_git_bytes(&ws, &mod_args),
    );

    let (ok_orig, orig_bytes, _) = orig_res.unwrap_or((false, Vec::new(), String::new()));
    let (ok_mod, mod_bytes, _) = mod_res.unwrap_or((false, Vec::new(), String::new()));

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
        patch: String::new(),
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

    let orig_spec = format!("{}:{}", base, old_rel);
    let safe_full = resolve_safe_path(&ws, &rel).ok();
    let (orig_res, wt_bytes) = tokio::join!(
        run_git_bytes(&ws, &["show", &orig_spec]),
        async {
            match safe_full {
                Some(full) => tokio::fs::read(&full).await.ok(),
                None => None,
            }
        },
    );
    let (ok_orig, orig_bytes, _) = orig_res.unwrap_or((false, Vec::new(), String::new()));
    if (ok_orig && is_binary_bytes(&orig_bytes))
        || wt_bytes.as_deref().map(is_binary_bytes).unwrap_or(false)
    {
        return Ok(Json(GitDiffRes {
            path: rel,
            staged: false,
            original: String::new(),
            modified: String::new(),
            patch: String::new(),
            binary: true,
        }));
    }

    let modified_text = wt_bytes
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();

    Ok(Json(GitDiffRes {
        path: rel,
        staged: false,
        original: if ok_orig {
            String::from_utf8_lossy(&orig_bytes).to_string()
        } else {
            String::new()
        },
        modified: modified_text,
        patch: String::new(),
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
    // Re-serialize through the URL parser: only http(s) with a host, and the string handed to the
    // OS is the normalized, percent-encoded form (no quotes, spaces or shell metacharacters).
    let raw = req.url.trim();
    let parsed = url::Url::parse(raw)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()))
        .filter(|_| !raw.chars().any(|c| c.is_control() || matches!(c, '"' | '\'' | '`' | ';')))
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid external URL".to_string()))?;
    let url = parsed.as_str();

    #[cfg(target_os = "windows")]
    let res = {
        // The user's browser must outlive Vireo, so it leaves the kill-on-close job.
        let mut c = background_command("rundll32");
        c.args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x0800_0000 | crate::jobobject::CREATE_BREAKAWAY_FROM_JOB);
        c.spawn()
    };
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
    fn git_command_refuses_program_overrides() {
        let ws = Path::new(".");
        for bad in [
            vec!["fetch", "--upload-pack=calc.exe", "origin"],
            vec!["push", "--receive-pack", "x", "origin"],
            vec!["log", "--config-env=core.pager=X"],
            vec!["show", "HEAD\0--exec"],
            vec!["diff", "--output=/tmp/owned", "HEAD"],
            vec!["log", "--output", "/tmp/owned"],
            vec!["grep", "--open-files-in-pager=calc", "x"],
            vec!["grep", "-Ocalc", "x"],
        ] {
            assert!(git_command(ws, &bad).is_err(), "{bad:?}");
        }
        // A commit/tag/stash message is data, even when it looks like an option.
        assert!(git_command(ws, &["commit", "-m", "--upload-pack is mentioned\nsecond line"]).is_ok());
        assert!(git_command(ws, &["checkout", "--", "src/--exec.rs"]).is_ok());
        // Look-alikes that are harmless stay allowed.
        assert!(git_command(ws, &["diff", "--output-indicator-new=+", "HEAD"]).is_ok());
        assert!(git_command(ws, &["grep", "-o", "x"]).is_ok());
        assert!(git_command(ws, &["log", "-O", "order.txt"]).is_ok());
    }

    #[test]
    fn test_clean_commit_message() {
        assert_eq!(
            clean_commit_message("```\nfeat(ui): add commit generator.\n\n- one\n```"),
            "feat(ui): add commit generator\n\n- one"
        );
        assert_eq!(
            clean_commit_message("Commit message: \"fix: handle empty diff\""),
            "fix: handle empty diff"
        );
    }

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

    #[test]
    fn test_parse_refs_full_decoration_without_remotes() {
        let (refs, is_head) = parse_refs(
            "HEAD -> refs/heads/feat/fast, refs/remotes/origin/feat/fast, refs/remotes/origin/HEAD, tag: refs/tags/v1.2.3, refs/heads/origin/local-branch, refs/stash, refs/notes/commits",
            &[],
        );
        assert!(is_head);
        assert_eq!(refs.len(), 4);
        assert_eq!((refs[0].name.as_str(), refs[0].kind.as_str(), refs[0].is_current), ("feat/fast", "local", true));
        assert_eq!((refs[1].name.as_str(), refs[1].kind.as_str(), refs[1].is_current), ("origin/feat/fast", "remote", false));
        assert_eq!((refs[2].name.as_str(), refs[2].kind.as_str(), refs[2].is_current), ("v1.2.3", "tag", false));
        // Even a local branch prefixed with `origin/` is unambiguously classified as local!
        assert_eq!((refs[3].name.as_str(), refs[3].kind.as_str(), refs[3].is_current), ("origin/local-branch", "local", false));
    }

    #[test]
    fn test_refs_fingerprint_reacts_to_git_ops_and_ignores_worktree_edits() {
        let base = std::env::temp_dir().join(format!("vireo-git-fp-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let git_ok = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&base)
                .output()
                .expect("git should be installed");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        git_ok(&["init", "-b", "main"]);
        git_ok(&["config", "user.email", "test@vireo.dev"]);
        git_ok(&["config", "user.name", "Vireo Test"]);
        std::fs::write(base.join("a.txt"), "hello\n").unwrap();
        git_ok(&["add", "a.txt"]);
        git_ok(&["commit", "-m", "init"]);

        let dirs = resolve_git_dirs(&base).expect("resolve_git_dirs should find .git");
        let (br, sha) = read_head_state(&dirs).expect("read_head_state");
        assert_eq!(br, "main");
        assert!(sha.as_deref().map(|s| s.len() >= 40).unwrap_or(false));

        // Subdirectory resolution works too.
        let sub = base.join("nested").join("dir");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(resolve_git_dirs(&sub), Some(dirs.clone()));

        let fp0 = compute_refs_fingerprint(&dirs);

        // Ordinary working-tree edit + staging MUST NOT change refs_fingerprint.
        std::fs::write(base.join("a.txt"), "hello world\n").unwrap();
        git_ok(&["add", "a.txt"]);
        assert_eq!(compute_refs_fingerprint(&dirs), fp0, "worktree/index edits must keep refs_fingerprint stable");

        // Tag creation MUST change refs_fingerprint.
        git_ok(&["tag", "v0.1.0"]);
        let fp_tag = compute_refs_fingerprint(&dirs);
        assert_ne!(fp_tag, fp0, "git tag must change refs_fingerprint");

        // Tag deletion MUST change refs_fingerprint.
        git_ok(&["tag", "-d", "v0.1.0"]);
        let fp_untag = compute_refs_fingerprint(&dirs);
        assert_ne!(fp_untag, fp_tag, "git tag -d must change refs_fingerprint");

        // Simulating a push updating a remote-tracking ref MUST change refs_fingerprint.
        let head_sha = sha.unwrap();
        git_ok(&["update-ref", "refs/remotes/origin/main", &head_sha]);
        let fp_push = compute_refs_fingerprint(&dirs);
        assert_ne!(fp_push, fp_untag, "remote ref update (push) must change refs_fingerprint");

        // Creating a branch MUST change refs_fingerprint.
        git_ok(&["branch", "feature/x"]);
        let fp_branch = compute_refs_fingerprint(&dirs);
        assert_ne!(fp_branch, fp_push, "branch creation must change refs_fingerprint");

        // Adding a remote (updating .git/config) MUST change refs_fingerprint.
        git_ok(&["remote", "add", "origin", "https://example.com/org/repo.git"]);
        let fp_remote = compute_refs_fingerprint(&dirs);
        assert_ne!(fp_remote, fp_branch, "remote add must change refs_fingerprint");

        // Worktree `.git` file resolution (gitdir + commondir).
        let wt = std::env::temp_dir().join(format!("vireo-git-wt-test-{}", uuid::Uuid::new_v4()));
        let wt_str = wt.to_string_lossy().to_string();
        git_ok(&["worktree", "add", &wt_str, "feature/x"]);
        let wt_dirs = resolve_git_dirs(&wt).expect("worktree resolve_git_dirs");
        assert_ne!(wt_dirs.git_dir, wt_dirs.common_dir);
        assert_eq!(
            wt_dirs.common_dir.canonicalize().unwrap(),
            dirs.common_dir.canonicalize().unwrap()
        );

        let _ = std::fs::remove_dir_all(&wt);
        let _ = std::fs::remove_dir_all(&base);
    }
}

// ---------------------------------------------------------------------------
// AI commit-message generation (GitHub Copilot CLI)
// ---------------------------------------------------------------------------

const COMMIT_DIFF_BUDGET: usize = 14_000;
const COMMIT_SYSTEM_PROMPT: &str = "You write git commit messages. Reply with ONLY the commit message, no quotes, no code fences, no preamble.\n\
Rules:\n\
- Subject line: imperative mood, max 60 characters, no trailing period, says WHAT changed and the main WHY/area.\n\
- Follow the style of the recent commit subjects given (Conventional Commits prefix like feat/fix/refactor(scope): if they use it, same casing otherwise). If there is no clear style, use Conventional Commits.\n\
- Add a body ONLY when the change has several distinct parts: blank line, then at most 3 short '- ' bullets. Otherwise subject only.\n\
- Be specific (name the feature/function/file area). Never write vague subjects like 'update code' or 'fix stuff'.\n\
- Describe only what the diff shows. Do not invent details.";

#[derive(Deserialize, Default)]
pub struct GenerateMessageReq {
    pub amend: Option<bool>,
}

#[derive(Serialize)]
pub struct GenerateMessageRes {
    pub ok: bool,
    pub message: String,
    /// "copilot_missing" | "copilot_unauthenticated" | "no_changes" | "failed" when !ok.
    pub code: String,
    pub error: String,
}

fn gen_fail(code: &str, error: &str) -> Json<GenerateMessageRes> {
    Json(GenerateMessageRes {
        ok: false,
        message: String::new(),
        code: code.to_string(),
        error: error.to_string(),
    })
}

/// One-shot, tool-less GitHub Copilot CLI call (`copilot -p ... --output-format json`).
/// Returns the assistant's text, or a user-facing error string ("MISSING" if not installed).
async fn run_copilot_oneshot(ws: &Path, prompt: &str) -> Result<String, String> {
    let Some(exe) = tokio::task::spawn_blocking(|| crate::pty::resolve_cli_tool_any(&["copilot"]))
        .await
        .ok()
        .flatten()
    else {
        return Err("MISSING".to_string());
    };
    let child = background_command(&exe.to_string_lossy())
        .args([
            "-p",
            prompt,
            "--output-format",
            "json",
            // Pure text task: no tools, no MCP servers, no repo instructions.
            "--available-tools",
            "",
            "--disable-builtin-mcps",
            "--no-custom-instructions",
            "--no-ask-user",
            "--no-auto-update",
            "--no-color",
        ])
        .current_dir(ws)
        .env("PATH", crate::pty::augmented_path_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Failed to run GitHub Copilot CLI: {}", e))?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "GitHub Copilot CLI timed out after 90s.".to_string())?
    .map_err(|e| format!("Failed to wait for GitHub Copilot CLI: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut text = String::new();
    let mut error = String::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v["type"].as_str() {
            Some("assistant.message") => {
                if let Some(c) = v["data"]["content"].as_str().filter(|c| !c.trim().is_empty()) {
                    text = c.to_string();
                }
            }
            Some("session.error") => {
                if let Some(m) = v["data"]["message"].as_str() {
                    error = m.to_string();
                }
            }
            _ => {}
        }
    }
    if !text.trim().is_empty() {
        return Ok(text);
    }
    if error.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let t = stderr.trim();
        error = if t.is_empty() {
            "GitHub Copilot CLI returned no message.".to_string()
        } else {
            truncate_chars(t, 300).to_string()
        };
    }
    Err(error)
}

fn truncate_chars(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Tidy model output: strip fences/quotes/"Commit message:" labels and trailing whitespace.
fn clean_commit_message(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.starts_with("```") {
        s = s
            .trim_start_matches("```")
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim_start()
            .to_string();
        if let Some(i) = s.rfind("```") {
            s.truncate(i);
        }
        s = s.trim().to_string();
    }
    let lower = s.to_lowercase();
    for label in ["commit message:", "message:"] {
        if lower.starts_with(label) {
            s = s[label.len()..].trim_start().to_string();
            break;
        }
    }
    let s = s.trim().trim_matches(|c| c == '"' || c == '`').trim();
    let mut lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    if let Some(first) = lines.first_mut() {
        *first = first.trim_end_matches('.');
    }
    lines.join("\n").trim().to_string()
}

pub async fn generate_commit_message(
    State(state): State<AppState>,
    body: Option<Json<GenerateMessageReq>>,
) -> Result<Json<GenerateMessageRes>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    let amend = body.and_then(|Json(r)| r.amend).unwrap_or(false);

    // Mirror what the Commit button will commit: staged changes, or everything when nothing is staged.
    let has_head = run_git(&ws, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .map(|r| r.0)
        .unwrap_or(false);
    let staged_any = !run_git(&ws, &["diff", "--cached", "--quiet"])
        .await
        .map(|r| r.0)
        .unwrap_or(true);
    let excludes = [
        ":(exclude)*.lock",
        ":(exclude)package-lock.json",
        ":(exclude)*.min.js",
        ":(exclude)*.map",
    ];
    let base: Vec<&str> = if amend && has_head {
        let has_parent = run_git(&ws, &["rev-parse", "--verify", "-q", "HEAD~1"])
            .await
            .map(|r| r.0)
            .unwrap_or(false);
        if has_parent {
            vec!["diff", "--cached", "HEAD~1"]
        } else {
            vec!["diff", "--cached"]
        }
    } else if staged_any {
        vec!["diff", "--cached"]
    } else if has_head {
        vec!["diff", "HEAD"]
    } else {
        vec!["diff", "--cached"]
    };

    let mut stat_args = base.clone();
    stat_args.extend(["--stat", "--no-color"]);
    let stat = run_git(&ws, &stat_args)
        .await
        .map(|r| r.1)
        .unwrap_or_default();
    let mut patch_args = base.clone();
    patch_args.extend(["--no-color", "-U2", "--no-ext-diff", "--", "."]);
    patch_args.extend(excludes);
    let patch = run_git(&ws, &patch_args)
        .await
        .map(|r| r.1)
        .unwrap_or_default();

    let untracked = if staged_any || amend {
        String::new()
    } else {
        run_git(&ws, &["ls-files", "--others", "--exclude-standard"])
            .await
            .map(|r| r.1)
            .unwrap_or_default()
    };

    if stat.trim().is_empty() && patch.trim().is_empty() && untracked.trim().is_empty() {
        return Ok(gen_fail("no_changes", "No changes to describe."));
    }

    let recent = if has_head {
        run_git(&ws, &["log", "-8", "--no-merges", "--pretty=format:%s"])
            .await
            .map(|r| r.1)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let branch = current_branch(&ws).await;

    let mut prompt = String::new();
    prompt.push_str(&format!("Branch: {}\n", branch));
    if !recent.trim().is_empty() {
        prompt.push_str(&format!("Recent commit subjects:\n{}\n\n", recent.trim()));
    }
    prompt.push_str(&format!(
        "Changed files:\n{}\n",
        truncate_chars(stat.trim(), 3_000)
    ));
    if !untracked.trim().is_empty() {
        let list: Vec<&str> = untracked.lines().take(30).collect();
        prompt.push_str(&format!("New untracked files:\n{}\n", list.join("\n")));
    }
    let budget = COMMIT_DIFF_BUDGET.saturating_sub(prompt.len().min(6_000));
    let clipped = truncate_chars(&patch, budget);
    prompt.push_str("\nDiff");
    if clipped.len() < patch.len() {
        prompt.push_str(" (truncated)");
    }
    prompt.push_str(":\n");
    prompt.push_str(clipped);

    let full_prompt = format!("{}

---
{}", COMMIT_SYSTEM_PROMPT, prompt);
    let content = match run_copilot_oneshot(&ws, &full_prompt).await {
        Ok(c) => c,
        Err(e) if e == "MISSING" => {
            return Ok(gen_fail(
                "copilot_missing",
                "GitHub Copilot CLI is not installed. Install it with `winget install GitHub.Copilot` (or `npm install -g @github/copilot`), then run `copilot` once to sign in.",
            ));
        }
        Err(e) => {
            let lower = e.to_lowercase();
            let code = if lower.contains("log in")
                || lower.contains("login")
                || lower.contains("auth")
                || lower.contains("sign in")
            {
                "copilot_unauthenticated"
            } else {
                "failed"
            };
            return Ok(gen_fail(code, &format!("GitHub Copilot CLI failed: {}", e)));
        }
    };
    let message = clean_commit_message(&content);
    if message.is_empty() {
        return Ok(gen_fail("failed", "The model returned an empty message."));
    }
    Ok(Json(GenerateMessageRes {
        ok: true,
        message,
        code: String::new(),
        error: String::new(),
    }))
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn bench_count_untracked() {
        let ws = std::env::temp_dir().join(format!("vireo-bench-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let body: String = (0..5000).map(|i| format!("log line {i} some output text here\n")).collect();
        let mut paths = Vec::new();
        for i in 0..1500 {
            std::fs::write(ws.join(format!("f{i}.log")), &body).unwrap();
            paths.push(format!("f{i}.log"));
        }
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            std::hint::black_box(count_untracked_lines(&ws, paths.clone()));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("BENCH count_untracked_lines(1500 files x 175KB) {best:.2} ms");
        let _ = std::fs::remove_dir_all(&ws);
    }
}
