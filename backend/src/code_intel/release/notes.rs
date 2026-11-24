//! Git analysis: tags, Conventional Commits since the last release, suggested bump, notes.

use super::semver::Ver;
use crate::code_intel::util::{command, run_capture};
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

pub struct GitOut {
    pub ok: bool,
    pub out: String,
    pub err: String,
}

pub fn git_t(ws: &Path, args: &[&str], secs: u64) -> Result<GitOut, String> {
    let mut c = command("git");
    c.arg("-C").arg(ws).args(args);
    let r = run_capture(c, Duration::from_secs(secs))?;
    if r.timed_out {
        return Err(format!("`git {}` timed out after {secs}s", args.first().unwrap_or(&"")));
    }
    Ok(GitOut { ok: r.code == 0, out: r.stdout.trim().to_string(), err: r.stderr.trim().to_string() })
}

pub fn git(ws: &Path, args: &[&str]) -> Result<GitOut, String> {
    git_t(ws, args, 30)
}

/// stdout of a successful git command, else its error text.
pub fn git_ok(ws: &Path, args: &[&str]) -> Result<String, String> {
    let r = git(ws, args)?;
    if r.ok {
        Ok(r.out)
    } else {
        Err(format!("git {}: {}", args.join(" "), if r.err.is_empty() { r.out } else { r.err }))
    }
}

#[derive(Clone, Serialize)]
pub struct Tag {
    pub name: String,
    #[serde(skip)]
    pub ver: Ver,
}

/// Semver tags reachable from HEAD, highest first.
pub fn tags(ws: &Path) -> Vec<Tag> {
    let out = git(ws, &["tag", "--merged", "HEAD", "--list"]).map(|g| g.out).unwrap_or_default();
    let mut v: Vec<Tag> =
        out.lines().filter_map(|l| Some(Tag { name: l.trim().to_string(), ver: Ver::parse(l.trim())? })).collect();
    v.sort_by(|a, b| b.ver.cmp(&a.ver));
    v
}

pub fn tag_prefix(latest: Option<&Tag>) -> String {
    match latest {
        Some(t) if !t.name.starts_with(['v', 'V']) => String::new(),
        _ => "v".to_string(),
    }
}

/// The tag release notes should compare against: the last stable one, unless releasing a prerelease.
pub fn previous_for<'a>(tags: &'a [Tag], target: Option<&Ver>) -> Option<&'a Tag> {
    let stable_target = target.map_or(true, |t| !t.is_pre());
    tags.iter()
        .filter(|t| target.map_or(true, |x| t.ver < *x))
        .find(|t| !stable_target || !t.ver.is_pre())
}

#[derive(Clone, Serialize)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub kind: String,
    pub scope: Option<String>,
    pub subject: String,
    pub breaking: bool,
    pub pr: Option<u64>,
}

/// Commits in `prev..HEAD` (no merges), newest first. Release bookkeeping commits are dropped.
pub fn commits(ws: &Path, prev: Option<&str>) -> Vec<Commit> {
    let range = prev.map(|p| format!("{p}..HEAD")).unwrap_or_else(|| "HEAD".to_string());
    let fmt = "--format=%H%x1f%an%x1f%s%x1f%b%x1e";
    let Ok(out) = git(ws, &["log", "--no-merges", fmt, &range]) else { return Vec::new() };
    let head = lazy_re!(r"^(\w+)(?:\(([^)]+)\))?(!)?:\s*(.+)$");
    let pr_re = lazy_re!(r"\(#(\d+)\)\s*$");
    let break_re = lazy_re!(r"(?m)^BREAKING[ -]CHANGE:");
    out.out
        .split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let mut f = rec.split('\x1f');
            let (hash, author, subject, body) = (f.next()?.trim(), f.next()?, f.next()?.trim(), f.next().unwrap_or(""));
            if hash.is_empty() || subject.is_empty() {
                return None;
            }
            let pr = pr_re.captures(subject).and_then(|c| c[1].parse().ok());
            let (kind, scope, bang, text) = match head.captures(subject) {
                Some(c) => (
                    c[1].to_ascii_lowercase(),
                    c.get(2).map(|m| m.as_str().to_string()),
                    c.get(3).is_some(),
                    c[4].to_string(),
                ),
                None => ("other".to_string(), None, false, subject.to_string()),
            };
            if kind == "chore" && scope.as_deref() == Some("release") {
                return None;
            }
            let text = pr_re.replace(&text, "").trim().to_string();
            Some(Commit {
                hash: hash.chars().take(7).collect(),
                author: author.to_string(),
                kind,
                scope,
                subject: text,
                breaking: bang || break_re.is_match(body),
                pr,
            })
        })
        .collect()
}

#[derive(Serialize)]
pub struct Suggestion {
    pub bump: &'static str,
    pub version: Option<String>,
    pub reason: String,
}

pub fn suggest(base: Option<&Ver>, cs: &[Commit]) -> Suggestion {
    let count = |k: &str| cs.iter().filter(|c| c.kind == k).count();
    let (breaking, feats, fixes) = (cs.iter().filter(|c| c.breaking).count(), count("feat"), count("fix") + count("perf"));
    let bump = match base {
        _ if cs.is_empty() => "none",
        // Before 1.0 a breaking change bumps the minor, as Cargo and npm expect.
        Some(v) if v.major == 0 && (breaking > 0 || feats > 0) => "minor",
        _ if breaking > 0 => "major",
        _ if feats > 0 => "minor",
        _ => "patch",
    };
    let reason = if cs.is_empty() {
        "no commits since the last release".to_string()
    } else {
        format!("{} commits: {breaking} breaking, {feats} feat, {fixes} fix/perf", cs.len())
    };
    let version = match (bump, base) {
        ("none", _) => None,
        (b, Some(v)) => v.resolve(b).ok().map(|n| n.to_string()),
        (_, None) => Some("0.1.0".to_string()),
    };
    Suggestion { bump, version, reason }
}

/// `(owner, repo)` of a GitHub `origin`.
pub fn github_repo(ws: &Path) -> Option<(String, String)> {
    let url = git(ws, &["remote", "get-url", "origin"]).ok().filter(|g| g.ok)?.out;
    let c = lazy_re!(r"github\.com[:/]([^/\s]+)/([^/\s]+?)(?:\.git)?/?$").captures(&url)?;
    Some((c[1].to_string(), c[2].to_string()))
}

fn authors_before(ws: &Path, prev: Option<&str>) -> HashSet<String> {
    let Some(p) = prev else { return HashSet::new() };
    git(ws, &["log", "--format=%an", p]).map(|g| g.out.lines().map(str::to_string).collect()).unwrap_or_default()
}

const SECTIONS: &[(&str, &[&str])] = &[
    ("Features", &["feat"]),
    ("Bug Fixes", &["fix"]),
    ("Performance & Refactoring", &["perf", "refactor"]),
    ("Documentation", &["docs"]),
    ("Dependencies & Maintenance", &["build", "ci", "chore", "deps", "test", "style", "revert"]),
    ("Other Changes", &["other"]),
];

pub struct Notes {
    pub markdown: String,
    pub commits: usize,
    pub compare_url: Option<String>,
}

pub fn build_notes(ws: &Path, tag: &str, prev: Option<&Tag>, cs: &[Commit]) -> Notes {
    let line = |c: &Commit, repo: &Option<(String, String)>| {
        let scope = c.scope.as_ref().map(|s| format!("**{s}:** ")).unwrap_or_default();
        let pr = match (c.pr, repo) {
            (Some(n), Some((o, r))) => format!(" ([#{n}](https://github.com/{o}/{r}/pull/{n}))"),
            (Some(n), None) => format!(" (#{n})"),
            _ => String::new(),
        };
        let mut s = c.subject.clone();
        if let Some(f) = s.get(..1) {
            s = format!("{}{}", f.to_uppercase(), &s[1..]);
        }
        format!("- {scope}{s}{pr} (`{}`)\n", c.hash)
    };
    let repo = github_repo(ws);
    let mut md = format!("## {tag}\n\n");
    let breaking: Vec<&Commit> = cs.iter().filter(|c| c.breaking).collect();
    if !breaking.is_empty() {
        md.push_str("### Breaking Changes\n\n");
        breaking.iter().for_each(|c| md.push_str(&line(c, &repo)));
        md.push('\n');
    }
    for (title, kinds) in SECTIONS {
        let items: Vec<&Commit> = cs.iter().filter(|c| !c.breaking && (kinds.contains(&c.kind.as_str()) || (*title == "Other Changes" && !SECTIONS.iter().any(|(_, k)| k.contains(&c.kind.as_str()))))).collect();
        if items.is_empty() {
            continue;
        }
        md.push_str(&format!("### {title}\n\n"));
        items.iter().for_each(|c| md.push_str(&line(c, &repo)));
        md.push('\n');
    }
    if cs.is_empty() {
        md.push_str("_No user-facing changes recorded._\n\n");
    }
    let known = authors_before(ws, prev.map(|p| p.name.as_str()));
    let mut fresh: Vec<&str> = Vec::new();
    for c in cs {
        if !known.contains(&c.author) && !fresh.contains(&c.author.as_str()) {
            fresh.push(&c.author);
        }
    }
    if prev.is_some() && !fresh.is_empty() {
        md.push_str("### New Contributors\n\n");
        fresh.iter().for_each(|a| md.push_str(&format!("- {a}\n")));
        md.push('\n');
    }
    let compare_url = match (&repo, prev) {
        (Some((o, r)), Some(p)) => Some(format!("https://github.com/{o}/{r}/compare/{}...{tag}", p.name)),
        _ => None,
    };
    if let Some(u) = &compare_url {
        md.push_str(&format!("**Full Changelog**: {u}\n"));
    }
    Notes { markdown: md.trim_end().to_string() + "\n", commits: cs.len(), compare_url }
}
