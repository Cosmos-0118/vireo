//! Agent usage for the status bar, read from the session logs the CLIs already write
//! (`~/.claude/projects/<workspace>/*.jsonl`, `~/.codex/sessions/YYYY/MM/DD/*.jsonl`).
//! That covers agents run in the terminal as well as in the chat panel, with no extra wiring.
//!
//! Claude's real plan limits (5-hour session and weekly %) are not in the logs. When Claude Code
//! is signed in with a subscription, they are read from the same OAuth usage endpoint Claude
//! Code's own `/usage` screen uses, with the access token Claude Code stored. The token is only
//! read, never refreshed (refreshing would rotate it under Claude Code's feet); when it has
//! expired the bar falls back to log totals until Claude Code refreshes it on its next run.

use axum::{extract::State, Json};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::state::AppState;

const BLOCK_SECS: u64 = 5 * 3600;
const DAY_SECS: u64 = 24 * 3600;
const CACHE_TTL: Duration = Duration::from_secs(10);
/// Plan limits move slowly; poll the endpoint at most this often.
const QUOTA_TTL: Duration = Duration::from_secs(60);
/// After a failure (offline, 429, 5xx), wait longer before trying again.
const QUOTA_RETRY: Duration = Duration::from_secs(300);
const QUOTA_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Clone, Serialize, Default)]
pub struct ClaudeUsage {
    /// Tokens (input + output + cache writes) in the current 5-hour block, all projects.
    pub block_tokens: u64,
    /// When the current block ends (unix seconds); 0 when no block is active.
    pub block_resets_at: u64,
    /// Tokens used by this workspace in the last 24 hours.
    pub day_tokens: u64,
    /// Share of this workspace's input tokens (last 24h) served from the prompt cache, 0..1.
    pub cache_hit: f64,
    /// Tokens held in the latest conversation's context, and its window size.
    pub ctx_tokens: u64,
    pub ctx_window: u64,
    /// When that conversation last changed (unix seconds); 0 when there is none.
    pub ctx_updated_at: u64,
    pub model: String,
    /// Plan limits from the OAuth usage endpoint; None when unavailable (see `quota_status`).
    pub quota: Option<ClaudeQuota>,
    /// "ok", "no_credentials" (API key / not signed in), "expired" or "error".
    pub quota_status: &'static str,
    /// Subscription type Claude Code recorded at sign-in ("pro", "max", ...), if any.
    pub plan: String,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct QuotaWindow {
    /// 0..100
    pub used_percent: f64,
    /// Unix seconds; 0 when the window has not started.
    pub resets_at: u64,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct ClaudeQuota {
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
    pub seven_day_opus: Option<QuotaWindow>,
    pub seven_day_sonnet: Option<QuotaWindow>,
}

#[derive(Clone, Serialize, Default)]
pub struct RateWindow {
    pub used_percent: f64,
    pub window_minutes: u64,
    pub resets_at: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct CodexUsage {
    pub day_tokens: u64,
    pub ctx_tokens: u64,
    pub ctx_window: u64,
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
}

#[derive(Clone, Serialize, Default)]
pub struct UsageRes {
    pub claude: Option<ClaudeUsage>,
    pub codex: Option<CodexUsage>,
}

static CACHE: Mutex<Option<(Instant, PathBuf, UsageRes)>> = Mutex::new(None);

#[derive(Clone)]
struct QuotaRead {
    quota: Option<ClaudeQuota>,
    status: &'static str,
    plan: String,
}

/// Last endpoint reading and when to ask again.
static QUOTA: Mutex<Option<(Instant, QuotaRead)>> = Mutex::new(None);

pub async fn get_usage(State(state): State<AppState>) -> Json<UsageRes> {
    let ws = state.workspace_path().await;
    if let Some((at, p, res)) = CACHE.lock().ok().and_then(|g| g.clone()) {
        if p == ws && at.elapsed() < CACHE_TTL {
            return Json(res);
        }
    }
    let quota = claude_quota(state.http_client()).await;
    let ws2 = ws.clone();
    let mut res = tokio::task::spawn_blocking(move || collect(&ws2)).await.unwrap_or_default();
    if res.claude.is_none() && quota.quota.is_some() {
        // Signed in with fresh limits but no recent local logs: still worth showing.
        res.claude = Some(ClaudeUsage { ctx_window: 200_000, ..Default::default() });
    }
    if let Some(c) = res.claude.as_mut() {
        c.quota = quota.quota;
        c.quota_status = quota.status;
        c.plan = quota.plan;
    }
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((Instant::now(), ws, res.clone()));
    }
    Json(res)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// Claude Code's config dir (`CLAUDE_CONFIG_DIR` overrides `~/.claude`).
fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".claude")))
}

async fn claude_quota(client: &reqwest::Client) -> QuotaRead {
    if let Some((next, read)) = QUOTA.lock().ok().and_then(|g| g.clone()) {
        if Instant::now() < next {
            return read;
        }
    }
    let read = fetch_quota(client).await;
    let ttl = if read.status == "error" { QUOTA_RETRY } else { QUOTA_TTL };
    if let Ok(mut g) = QUOTA.lock() {
        *g = Some((Instant::now() + ttl, read.clone()));
    }
    read
}

async fn fetch_quota(client: &reqwest::Client) -> QuotaRead {
    let none = |status| QuotaRead { quota: None, status, plan: String::new() };
    // macOS keeps these in the Keychain instead; reading it would prompt, so only the file is used.
    let Some(path) = claude_config_dir().map(|d| d.join(".credentials.json")) else {
        return none("no_credentials");
    };
    let Ok(text) = tokio::fs::read_to_string(&path).await else { return none("no_credentials") };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return none("no_credentials") };
    let Some(oauth) = v.get("claudeAiOauth") else { return none("no_credentials") };
    let plan = oauth.get("subscriptionType").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(token) = oauth.get("accessToken").and_then(Value::as_str).filter(|t| !t.is_empty()) else {
        return none("no_credentials");
    };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let expires_ms = oauth.get("expiresAt").and_then(Value::as_u64).unwrap_or(u64::MAX);
    if expires_ms <= now_ms {
        return QuotaRead { quota: None, status: "expired", plan };
    }
    let resp = client
        .get(QUOTA_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(reqwest::header::USER_AGENT, concat!("vireo/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(8))
        .send()
        .await;
    let body = match resp {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            return QuotaRead { quota: None, status: "expired", plan };
        }
        _ => None,
    };
    match body.as_ref().and_then(parse_quota) {
        Some(q) => QuotaRead { quota: Some(q), status: "ok", plan },
        None => QuotaRead { quota: None, status: "error", plan },
    }
}

/// `{ five_hour: { utilization, resets_at }, seven_day: {...}, seven_day_opus, seven_day_sonnet }`
fn parse_quota(v: &Value) -> Option<ClaudeQuota> {
    let win = |k: &str| {
        let w = v.get(k).filter(|w| w.is_object())?;
        let used_percent = w.get("utilization").and_then(Value::as_f64)?;
        let resets_at = match w.get("resets_at") {
            Some(Value::String(s)) => parse_iso(s).unwrap_or(0),
            Some(n) => n.as_u64().unwrap_or(0),
            None => 0,
        };
        Some(QuotaWindow { used_percent: used_percent.clamp(0.0, 100.0), resets_at })
    };
    let q = ClaudeQuota {
        five_hour: win("five_hour"),
        seven_day: win("seven_day"),
        seven_day_opus: win("seven_day_opus"),
        seven_day_sonnet: win("seven_day_sonnet"),
    };
    (q.five_hour.is_some() || q.seven_day.is_some()).then_some(q)
}

fn collect(ws: &Path) -> UsageRes {
    let Some(home) = home_dir() else {
        return UsageRes::default();
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let claude_root = claude_config_dir().unwrap_or_else(|| home.join(".claude")).join("projects");
    UsageRes {
        claude: claude_usage(&claude_root, ws, now),
        codex: codex_usage(&home.join(".codex").join("sessions"), ws, now),
    }
}

/// Files under `dir` (one level of subfolders when `nested`) modified within `max_age` seconds.
fn recent_jsonl(dir: &Path, now: u64, max_age: u64) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if depth < 4 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                let m = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if now.saturating_sub(m) <= max_age {
                    out.push((p, m));
                }
            }
        }
    }
    out
}

/// Claude Code stores a workspace under its path with every non-alphanumeric char as `-`.
fn claude_dir_name(ws: &Path) -> String {
    ws.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

struct Msg {
    ts: u64,
    /// input + output + cache writes
    billed: u64,
    /// In this workspace.
    mine: bool,
    /// All prompt tokens (fresh + cache reads + cache writes).
    input: u64,
    cache_read: u64,
}
fn claude_usage(root: &Path, ws: &Path, now: u64) -> Option<ClaudeUsage> {
    if !root.is_dir() {
        return None;
    }
    let mine = claude_dir_name(ws).to_lowercase();
    // One entry per distinct assistant message.
    let mut msgs: Vec<Msg> = Vec::new();
    let mut latest_ctx: Option<(u64, u64, String)> = None; // (mtime, ctx tokens, model)
    let Ok(projects) = std::fs::read_dir(root) else { return None };
    for proj in projects.flatten() {
        let is_mine = proj.file_name().to_string_lossy().to_lowercase() == mine;
        for (file, mtime) in recent_jsonl(&proj.path(), now, DAY_SECS) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let mut by_id: HashMap<String, Msg> = HashMap::new();
            let mut last_ctx: Option<(u64, String)> = None;
            for line in text.lines() {
                if !line.contains("\"usage\"") {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                let Some(msg) = v.get("message") else { continue };
                let Some(u) = msg.get("usage") else { continue };
                let ts = v.get("timestamp").and_then(Value::as_str).and_then(parse_iso).unwrap_or(0);
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                let billed = n("input_tokens") + n("output_tokens") + n("cache_creation_input_tokens");
                let ctx = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                let m = Msg { ts, billed, mine: is_mine, input: ctx, cache_read: n("cache_read_input_tokens") };
                let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let model = msg.get("model").and_then(Value::as_str).unwrap_or("").to_string();
                if model == "<synthetic>" {
                    continue;
                }
                if id.is_empty() {
                    msgs.push(m);
                } else {
                    // Streamed turns log the same message repeatedly: keep the last (final) usage.
                    by_id.insert(id, m);
                }
                if v.get("isSidechain").and_then(Value::as_bool) != Some(true) {
                    last_ctx = Some((ctx, model));
                }
            }
            msgs.extend(by_id.into_values());
            if is_mine {
                if let Some((ctx, model)) = last_ctx {
                    if latest_ctx.as_ref().map_or(true, |(m, _, _)| mtime > *m) {
                        latest_ctx = Some((mtime, ctx, model));
                    }
                }
            }
        }
    }
    if msgs.is_empty() {
        return None;
    }
    msgs.sort_by_key(|m| m.ts);
    // 5-hour blocks: a block opens at the hour of its first message and lasts five hours.
    let mut block_start = 0u64;
    let mut block_tokens = 0u64;
    for m in &msgs {
        if block_start == 0 || m.ts >= block_start + BLOCK_SECS {
            block_start = m.ts - m.ts % 3600;
            block_tokens = 0;
        }
        block_tokens += m.billed;
    }
    let active = block_start != 0 && now < block_start + BLOCK_SECS;
    let today = || msgs.iter().filter(|m| m.mine && now.saturating_sub(m.ts) <= DAY_SECS);
    let day_tokens = today().map(|m| m.billed).sum();
    let (input, cached) = today().fold((0u64, 0u64), |(i, c), m| (i + m.input, c + m.cache_read));
    let cache_hit = if input > 0 { cached as f64 / input as f64 } else { 0.0 };
    let (ctx_updated_at, ctx_tokens, model) = latest_ctx.unwrap_or_default();
    let ctx_window = if model.contains("[1m]") || ctx_tokens > 200_000 { 1_000_000 } else { 200_000 };
    Some(ClaudeUsage {
        block_tokens: if active { block_tokens } else { 0 },
        block_resets_at: if active { block_start + BLOCK_SECS } else { 0 },
        day_tokens,
        cache_hit,
        ctx_tokens,
        ctx_window,
        ctx_updated_at,
        model,
        ..Default::default()
    })
}

fn same_path(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.trim_start_matches(r"\\?\").replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}

fn codex_usage(root: &Path, ws: &Path, now: u64) -> Option<CodexUsage> {
    if !root.is_dir() {
        return None;
    }
    let mut out = CodexUsage::default();
    let mut seen = false;
    let mut newest_limits = 0u64;
    let mut newest_ctx = 0u64;
    for (file, mtime) in recent_jsonl(root, now, DAY_SECS) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let mut mine = false;
        let mut total = 0u64;
        let mut ctx: Option<(u64, u64)> = None;
        let mut limits: Option<(Option<RateWindow>, Option<RateWindow>)> = None;
        for line in text.lines() {
            if line.contains("\"session_meta\"") {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    mine = v
                        .pointer("/payload/cwd")
                        .and_then(Value::as_str)
                        .is_some_and(|c| same_path(c, ws));
                }
                continue;
            }
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(p) = v.get("payload") else { continue };
            if let Some(info) = p.get("info").filter(|i| i.is_object()) {
                total = info.pointer("/total_token_usage/total_tokens").and_then(Value::as_u64).unwrap_or(total);
                let window = info.get("model_context_window").and_then(Value::as_u64).unwrap_or(0);
                if let Some(last) = info.pointer("/last_token_usage/total_tokens").and_then(Value::as_u64) {
                    ctx = Some((last, window));
                }
            }
            if let Some(r) = p.get("rate_limits").filter(|r| r.is_object()) {
                let win = |k: &str| {
                    r.get(k).filter(|w| w.is_object()).map(|w| RateWindow {
                        used_percent: w.get("used_percent").and_then(Value::as_f64).unwrap_or(0.0),
                        window_minutes: w.get("window_minutes").and_then(Value::as_u64).unwrap_or(0),
                        resets_at: w.get("resets_at").and_then(Value::as_u64).unwrap_or(0),
                    })
                };
                limits = Some((win("primary"), win("secondary")));
            }
        }
        if mine {
            seen = true;
            out.day_tokens += total;
            if let Some((c, w)) = ctx {
                if mtime >= newest_ctx {
                    newest_ctx = mtime;
                    out.ctx_tokens = c;
                    out.ctx_window = w;
                }
            }
        }
        // Rate limits belong to the account, so any session's latest reading will do.
        if let Some((p, s)) = limits {
            seen = true;
            if mtime >= newest_limits {
                newest_limits = mtime;
                out.primary = p.filter(|w| w.resets_at == 0 || w.resets_at > now);
                out.secondary = s.filter(|w| w.resets_at == 0 || w.resets_at > now);
            }
        }
    }
    seen.then_some(out)
}

/// `2026-11-28T12:54:00.123Z` / `2026-11-28T12:54:00.123456+05:30` -> unix seconds.
fn parse_iso(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Optional fraction, then `Z`, `+HH:MM` / `-HH:MM`, or nothing (UTC).
    let rest = s.get(19..).unwrap_or("").trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        Some(&sign @ (b'+' | b'-')) => {
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
            let o = oh * 3600 + om * 60;
            if sign == b'+' { o } else { -o }
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec - offset).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(parse_iso("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso("2026-11-28T12:54:00.123Z"), Some(1_795_870_440));
        assert_eq!(parse_iso("2026-11-28T12:54:00.123456+00:00"), Some(1_795_870_440));
        assert_eq!(parse_iso("2026-11-28T18:24:00+05:30"), Some(1_795_870_440));
        assert_eq!(parse_iso("2026-11-28T07:54:00-05:00"), Some(1_795_870_440));
    }

    #[test]
    fn parses_oauth_quota() {
        let v: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":37.0,"resets_at":"2026-11-28T12:54:00.5+00:00"},
                "seven_day":{"utilization":26,"resets_at":null},
                "seven_day_opus":null,
                "seven_day_sonnet":{"utilization":24.0,"resets_at":"2026-11-28T12:54:00Z"}}"#,
        )
        .unwrap();
        let q = parse_quota(&v).unwrap();
        assert_eq!(q.five_hour, Some(QuotaWindow { used_percent: 37.0, resets_at: 1_795_870_440 }));
        assert_eq!(q.seven_day, Some(QuotaWindow { used_percent: 26.0, resets_at: 0 }));
        assert_eq!(q.seven_day_opus, None);
        assert_eq!(q.seven_day_sonnet.map(|w| w.used_percent), Some(24.0));
        assert!(parse_quota(&serde_json::json!({ "error": "x" })).is_none());
    }

    #[test]
    fn encodes_claude_project_dir() {
        assert_eq!(
            claude_dir_name(Path::new(r"C:\Users\user\Documents\Developer\vireo")),
            "C--Users-user-Documents-Developer-vireo"
        );
    }
}
