//! Append-only log of release actions, whoever ran them (the Releases page, `vx release` in a
//! terminal, or an agent over MCP). The Releases page polls it so people can watch an agent
//! prepare and ship a release. It lives in the git dir: never tracked, never dirties the tree.

use super::notes::git_ok;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "vireo-release-activity.jsonl";
/// Entries kept after a trim; the file is rewritten only once it grows `SLACK` past this.
const KEEP: usize = 100;
const SLACK: usize = 25;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn log_path(ws: &Path) -> Option<PathBuf> {
    let dir = git_ok(ws, &["rev-parse", "--git-dir"]).ok()?;
    let dir = dir.trim();
    if dir.is_empty() {
        return None;
    }
    Some(ws.join(dir).join(FILE))
}

/// Append one entry. `id` groups a `running` entry with the entry that finishes it.
/// Logging is best effort: a failure never breaks the release action itself.
pub fn record(ws: &Path, id: u64, source: &str, action: &str, mut entry: Value) {
    let Some(path) = log_path(ws) else { return };
    if let Some(o) = entry.as_object_mut() {
        o.insert("id".into(), json!(id.to_string()));
        o.insert("ts".into(), json!(now_ms()));
        o.insert("source".into(), json!(source));
        o.insert("action".into(), json!(action));
        o.entry("phase").or_insert(json!("done"));
    }
    let Ok(line) = serde_json::to_string(&entry) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
    trim(&path);
}

fn trim(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= KEEP + SLACK {
        return;
    }
    let mut out = lines[lines.len() - KEEP..].join("\n");
    out.push('\n');
    let _ = std::fs::write(path, out);
}

/// Newest first, with each `running` entry replaced by the entry that finished it (same `id`).
pub fn read(ws: &Path, limit: usize) -> Vec<Value> {
    let Some(path) = log_path(ws) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut out: Vec<Value> = Vec::new();
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if out.iter().any(|e| e["id"] == v["id"]) {
            continue; // an older line of an entry already seen (its `running` start)
        }
        out.push(v);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Text form for `vx release activity`.
pub fn render(items: &[Value]) -> String {
    if items.is_empty() {
        return "no release activity yet".into();
    }
    let mut o = String::new();
    for e in items {
        let ok = match (e["phase"].as_str(), e["ok"].as_bool()) {
            (Some("running"), _) => "…",
            (_, Some(true)) => "✔",
            (_, Some(false)) => "✖",
            _ => "·",
        };
        o.push_str(&format!(
            "{ok} {} {} [{}] {}\n",
            e["action"].as_str().unwrap_or("?"),
            e["tag"].as_str().unwrap_or(""),
            e["source"].as_str().unwrap_or("?"),
            e["summary"].as_str().unwrap_or("")
        ));
    }
    o
}
