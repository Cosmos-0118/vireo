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
    // Avoid rewriting `.git/index` or contending on `index.lock` during read-only probes.
    // Mutating git commands still take required locks regardless of this flag.
    c.env("GIT_OPTIONAL_LOCKS", "0");
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

/// True when `tag` already exists as a local git tag (even if unreachable from HEAD).
pub fn tag_exists(ws: &Path, tag: &str) -> bool {
    git(ws, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_ok_and(|g| g.ok)
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
    /// True when `kind` was inferred heuristically from a non-conventional commit subject.
    pub inferred: bool,
    /// Non-trailer commit body lines, used as agent context (not written into the release notes file).
    #[serde(skip)]
    pub body: String,
}

fn canonical_kind(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().as_str() {
        "feat" | "feature" | "features" => Some("feat"),
        "fix" | "bugfix" | "bug" | "hotfix" | "security" | "sec" => Some("fix"),
        "perf" | "performance" => Some("perf"),
        "refactor" | "refact" => Some("refactor"),
        "docs" | "doc" => Some("docs"),
        "build" => Some("build"),
        "ci" => Some("ci"),
        "chore" | "chores" | "maint" => Some("chore"),
        "deps" | "dep" | "dependencies" => Some("deps"),
        "test" | "tests" => Some("test"),
        "style" => Some("style"),
        "revert" => Some("revert"),
        _ => None,
    }
}

/// Conservative verb/keyword heuristic for non-conventional commit subjects.
fn infer_kind(subject: &str) -> Option<&'static str> {
    let s = subject.trim();
    // If written like `Area: fix crash`, inspect the part after the first `: ` as well.
    let tail = s
        .split_once(": ")
        .filter(|(p, _)| !p.is_empty() && p.len() <= 24 && !p.contains(' '))
        .map_or(s, |(_, r)| r.trim());
    let l = tail.to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| l.starts_with(p));

    if starts(&["test ", "tests ", "add test", "add unit test", "add integration test", "fix test", "update test"]) {
        return Some("test");
    }
    if starts(&["docs ", "doc ", "document ", "update readme", "update docs", "update changelog", "add docs", "fix typo", "fix doc"]) {
        return Some("docs");
    }
    if starts(&["bump ", "upgrade ", "update dependenc", "update lockfile", "pin "]) {
        return Some("deps");
    }
    if starts(&["ci ", "workflow ", "github actions"]) {
        return Some("ci");
    }
    if starts(&["fix ", "fixes ", "fixed ", "bugfix ", "hotfix ", "resolve ", "resolved ", "patch ", "prevent ", "avoid crash", "handle crash"]) {
        return Some("fix");
    }
    if starts(&["perf ", "optimiz", "speed up", "faster ", "reduce memory"]) {
        return Some("perf");
    }
    if starts(&["refactor ", "simplify ", "clean up ", "cleanup ", "extract ", "reorganize "]) {
        return Some("refactor");
    }
    if starts(&["add ", "added ", "implement ", "implemented ", "introduce ", "support ", "enable ", "allow ", "create ", "new "]) {
        return Some("feat");
    }
    None
}

fn classify_subject(subject: &str) -> (String, Option<String>, bool, String, bool) {
    let head = lazy_re!(r"^(\w+)(?:\(([^)]+)\))?(!)?:\s*(.+)$");
    if let Some(c) = head.captures(subject) {
        let raw_kind = &c[1];
        let scope = c.get(2).map(|m| m.as_str().to_string());
        let bang = c.get(3).is_some();
        if let Some(canon) = canonical_kind(raw_kind) {
            return (canon.to_string(), scope, bang, c[4].to_string(), false);
        }
        // Only treat an unknown prefix as a Conventional Commit when `(scope)` or `!` is present;
        // otherwise preserve prefixes like `Windows: fix path` in the subject text.
        if scope.is_some() || bang {
            return (raw_kind.to_ascii_lowercase(), scope, bang, c[4].to_string(), false);
        }
    }

    let bracket = lazy_re!(r"^\[([A-Za-z]+)\]\s*(.+)$");
    if let Some(c) = bracket.captures(subject) {
        if let Some(canon) = canonical_kind(&c[1]) {
            return (canon.to_string(), None, false, c[2].to_string(), false);
        }
    }

    if subject.starts_with("Revert \"") || subject.starts_with("revert \"") {
        return ("revert".to_string(), None, false, subject.to_string(), false);
    }

    const GITMOJI: &[(&str, &str, bool)] = &[
        ("💥", "feat", true),
        ("✨", "feat", false),
        ("🐛", "fix", false),
        ("🩹", "fix", false),
        ("🔒", "fix", false),
        ("⚡", "perf", false),
        ("♻️", "refactor", false),
        ("♻", "refactor", false),
        ("📝", "docs", false),
        ("✅", "test", false),
        ("🔧", "chore", false),
        ("⬆", "deps", false),
        ("⬇", "deps", false),
        ("👷", "ci", false),
    ];
    for &(emoji, kind, breaking) in GITMOJI {
        if let Some(rest) = subject.strip_prefix(emoji) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return (kind.to_string(), None, breaking, rest.to_string(), false);
            }
        }
    }

    if let Some(inferred) = infer_kind(subject) {
        return (inferred.to_string(), None, false, subject.to_string(), true);
    }
    ("other".to_string(), None, false, subject.to_string(), false)
}

fn clean_body(raw: &str) -> String {
    let trailer = lazy_re!(r"^(?i)(?:co-authored-by|signed-off-by|reviewed-by|acked-by|tested-by|helped-by|reported-by|change-id):\s");
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !trailer.is_match(l))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Commits in `prev..upper` (no merges), newest first. Release bookkeeping commits are dropped.
pub fn commits_between(ws: &Path, prev: Option<&str>, upper: &str) -> Vec<Commit> {
    let range = prev.map(|p| format!("{p}..{upper}")).unwrap_or_else(|| upper.to_string());
    let fmt = "--format=%H%x1f%an%x1f%s%x1f%b%x1e";
    let Ok(out) = git(ws, &["log", "--no-merges", fmt, &range]) else { return Vec::new() };
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
            let (kind, scope, bang, text, inferred) = classify_subject(subject);
            if !inferred && kind == "chore" && scope.as_deref() == Some("release") {
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
                inferred,
                body: clean_body(body),
            })
        })
        .collect()
}

/// Commits in `prev..HEAD` (no merges), newest first. Release bookkeeping commits are dropped.
pub fn commits(ws: &Path, prev: Option<&str>) -> Vec<Commit> {
    commits_between(ws, prev, "HEAD")
}

/// Compact `git diff --stat` summary for `prev..upper`, capped to `max_files` lines plus totals.
pub fn diff_stat(ws: &Path, prev: Option<&str>, upper: &str, max_files: usize) -> Option<String> {
    // Git's well-known empty tree object hash lets `diff --stat` work even before the first tag.
    let base = prev.unwrap_or("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    let range = format!("{base}..{upper}");
    let out = git(ws, &["diff", "--stat=80", &range]).ok().filter(|g| g.ok)?.out;
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).map(|l| l.strip_prefix(' ').unwrap_or(l)).collect();
    if lines.is_empty() {
        return None;
    }
    if lines.len() <= max_files + 1 {
        return Some(lines.join("\n"));
    }
    let summary = *lines.last().unwrap();
    let files = &lines[..lines.len() - 1];
    let mut kept: Vec<String> = files.iter().take(max_files).map(|l| (*l).to_string()).collect();
    kept.push(format!("... (+{} more files)", files.len() - max_files));
    kept.push(summary.to_string());
    Some(kept.join("\n"))
}

pub fn unconventional_count(cs: &[Commit]) -> usize {
    cs.iter().filter(|c| c.inferred || c.kind == "other").count()
}

#[derive(Serialize)]
pub struct Suggestion {
    pub bump: &'static str,
    pub version: Option<String>,
    pub reason: String,
}

pub fn suggest(base: Option<&Ver>, cs: &[Commit]) -> Suggestion {
    let explicit = |k: &str| cs.iter().filter(|c| !c.inferred && c.kind == k).count();
    let breaking = cs.iter().filter(|c| c.breaking).count();
    let feats = explicit("feat");
    let fixes = explicit("fix") + explicit("perf");
    let unconv = unconventional_count(cs);
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
    } else if unconv == cs.len() && breaking == 0 {
        format!("{} non-conventional commit(s); review changes to confirm patch/minor/major", cs.len())
    } else if unconv > 0 {
        format!("{} commits: {breaking} breaking, {feats} feat, {fixes} fix/perf, {unconv} non-conventional", cs.len())
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
        let mut chars = c.subject.chars();
        let s = match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        };
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
