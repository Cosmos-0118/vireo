//! Polyglot release engine behind `vx release` and the Releases page.
//!
//! Stages: `status` (inspect & plan) → `preflight` (gates) → `notes` (Conventional-Commit
//! changelog) → `bump` (atomic version sync across stacks) → `publish` (commit, tag, push or
//! draft GitHub release) → `verify`. Every stage is a plain function over [`Ctx`]; [`run`] is the
//! single entry point and renders text for agents or JSON (`"json": true`) for the UI.

mod activity;
mod flow;
mod manifest;
mod notes;
mod semver;
#[cfg(test)]
mod tests;

use manifest::{FileChange, Kind, Manifest};
use notes::{git_ok, Commit, Tag};
use semver::Ver;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_PATH: &str = ".vireo/release.json";

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Extra version locations: `{ "path": "CMakeLists.txt", "pattern": "VERSION ([0-9.]+)" }`
    /// (capture group 1 is replaced).
    pub files: Vec<CustomFile>,
    /// Path prefixes the scanner must ignore (e.g. vendored packages with their own versions).
    pub exclude: Vec<String>,
    /// Globs of build outputs to attach to a GitHub release (`gh` mode).
    pub assets: Vec<String>,
    /// Commands to run for the build/test gate instead of auto-detection.
    pub checks: Vec<String>,
    /// Tag prefix, `v` by default (or whatever the latest tag uses).
    pub tag_prefix: Option<String>,
    /// `ci` | `gh` | `local`. Detected from the workflows when absent.
    pub mode: Option<String>,
}

#[derive(Deserialize)]
pub struct CustomFile {
    pub path: String,
    pub pattern: String,
}

impl Config {
    fn load(ws: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(ws.join(DEFAULT_CONFIG_PATH)) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{DEFAULT_CONFIG_PATH}: {e}")),
            Err(_) => Ok(Config::default()),
        }
    }

    fn is_excluded(&self, rel: &str) -> bool {
        self.exclude.iter().any(|p| {
            let p = p.replace('\\', "/");
            let p = p.trim_matches('/');
            rel == p || rel.starts_with(&format!("{p}/"))
        })
    }
}

pub struct Ctx {
    pub ws: PathBuf,
    pub cfg: Config,
    pub manifests: Vec<Manifest>,
    pub tags: Vec<Tag>,
}

impl Ctx {
    pub fn load(ws: &Path) -> Result<Ctx, String> {
        let cfg = Config::load(ws)?;
        let manifests = manifest::scan(ws, &cfg);
        let tags = notes::tags(ws);
        Ok(Ctx { ws: ws.to_path_buf(), cfg, manifests, tags })
    }

    /// Most common manifest version (ties go to the highest); the latest tag when there are none.
    pub fn base(&self) -> Ver {
        let mut votes: BTreeMap<Ver, usize> = BTreeMap::new();
        for m in self.manifests.iter().filter(|m| !matches!(m.kind, Kind::CargoLock | Kind::NpmLock | Kind::Nbgv)) {
            if let Some(v) = m.version.as_deref().and_then(Ver::parse) {
                *votes.entry(v).or_default() += 1;
            }
        }
        votes
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
            .map(|(v, _)| v)
            .or_else(|| self.tags.first().map(|t| t.ver.clone()))
            .unwrap_or(Ver { major: 0, minor: 0, patch: 0, pre: None })
    }

    pub fn prefix(&self) -> String {
        self.cfg.tag_prefix.clone().unwrap_or_else(|| notes::tag_prefix(self.tags.first()))
    }

    pub fn tag_for(&self, v: &Ver) -> String {
        format!("{}{v}", self.prefix())
    }

    pub fn prev_tag(&self, target: Option<&Ver>) -> Option<&Tag> {
        notes::previous_for(&self.tags, target)
    }

    pub fn commits_since(&self, target: Option<&Ver>) -> Vec<Commit> {
        notes::commits(&self.ws, self.prev_tag(target).map(|t| t.name.as_str()))
    }

    /// Workflow files that run on a pushed tag.
    pub fn ci_workflows(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.ws.join(".github/workflows")) else { return Vec::new() };
        let re = lazy_re!(r"(?m)^\s*push:\s*\n(?:[ \t]+.*\n|\s*\n)*?[ \t]+tags:");
        let mut out: Vec<String> = rd
            .flatten()
            .filter(|e| matches!(e.path().extension().and_then(|x| x.to_str()), Some("yml" | "yaml")))
            .filter(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| re.is_match(&t.replace("\r\n", "\n") )))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        out.sort();
        out
    }

    pub fn mode(&self) -> String {
        match self.cfg.mode.as_deref() {
            Some(m @ ("ci" | "gh" | "local")) => m.to_string(),
            _ if !self.ci_workflows().is_empty() => "ci".into(),
            _ if notes::github_repo(&self.ws).is_some() => "gh".into(),
            _ => "local".into(),
        }
    }

    /// Where the release notes for `tag` live: next to hand-written ones when the repo keeps them.
    pub fn notes_path(&self, tag: &str) -> String {
        if self.ws.join(".github/release-notes").is_dir() {
            format!(".github/release-notes/{tag}.md")
        } else {
            format!(".vireo/release-notes/{tag}.md")
        }
    }

    pub fn target(&self, arg: Option<&str>) -> Result<Ver, String> {
        let base = self.base();
        match arg.map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => base.resolve(a),
            None => {
                let cs = self.commits_since(None);
                self.suggestion(&cs)
                    .version
                    .as_deref()
                    .and_then(Ver::parse)
                    .ok_or_else(|| "nothing to release since the last tag; pass a version (patch|minor|major|X.Y.Z)".to_string())
            }
        }
    }

    pub fn suggestion(&self, cs: &[Commit]) -> notes::Suggestion {
        let base = self.base();
        let latest = self.tags.first();
        let mut s = notes::suggest(Some(&base), cs);
        match latest {
            None => {
                s.bump = "first";
                s.version = Some(base.to_string());
                s.reason = "no release tag yet: tag the current version".into();
            }
            // Manifests were already moved past the last tag (an earlier bump that was never tagged).
            Some(t) if base > t.ver => {
                s.bump = "as-is";
                s.version = Some(base.to_string());
                s.reason = format!("files already at {base}, newer than {}: release it as-is", t.name);
            }
            _ => {}
        }
        s
    }
}

#[derive(Serialize)]
struct StatusOut {
    version: String,
    latest_tag: Option<String>,
    latest_stable: Option<String>,
    tag_prefix: String,
    branch: Option<String>,
    dirty: usize,
    stacks: Vec<&'static str>,
    manifests: Vec<Manifest>,
    drift: Vec<String>,
    commits_since: usize,
    breaking: usize,
    feats: usize,
    fixes: usize,
    suggested: notes::Suggestion,
    /// `patch|minor|major|prerelease` -> the version each would produce.
    bumps: BTreeMap<&'static str, String>,
    ci_workflows: Vec<String>,
    mode: String,
    repo: Option<String>,
    notes_path: String,
    notes_exists: bool,
    config: bool,
    /// Uncommitted tracked files (first 20), so the UI can say exactly what blocks a release.
    dirty_files: Vec<String>,
    /// Most recent release tags, newest first.
    releases: Vec<PastRelease>,
}

#[derive(Serialize)]
struct PastRelease {
    tag: String,
    version: String,
    prerelease: bool,
    date: Option<String>,
    commit: Option<String>,
}

fn past_releases(ctx: &Ctx, n: usize) -> Vec<PastRelease> {
    let tags: Vec<&Tag> = ctx.tags.iter().take(n).collect();
    if tags.is_empty() {
        return Vec::new();
    }
    let refs: Vec<String> = tags.iter().map(|t| format!("refs/tags/{}", t.name)).collect();
    let mut args = vec![
        "for-each-ref",
        "--format=%(refname:short)%09%(creatordate:iso-strict)%09%(if)%(*objectname)%(then)%(*objectname:short)%(else)%(objectname:short)%(end)",
    ];
    args.extend(refs.iter().map(String::as_str));
    let meta = git_ok(&ctx.ws, &args).unwrap_or_default();
    let info = |name: &str| {
        meta.lines().find_map(|l| {
            let mut p = l.split('\t');
            (p.next()? == name).then(|| (p.next().map(str::to_string), p.next().map(str::to_string)))
        })
    };
    tags.iter()
        .map(|t| {
            let (date, commit) = info(&t.name).unwrap_or((None, None));
            PastRelease { tag: t.name.clone(), version: t.ver.to_string(), prerelease: t.ver.is_pre(), date, commit }
        })
        .collect()
}

fn status(ctx: &Ctx) -> StatusOut {
    let ws = &ctx.ws;
    let cs = ctx.commits_since(None);
    let count = |k: &str| cs.iter().filter(|c| c.kind == k).count();
    let mut stacks: Vec<&'static str> = ctx.manifests.iter().map(|m| m.stack).collect();
    if ws.join("go.mod").is_file() {
        stacks.push("Go");
    }
    stacks.sort();
    stacks.dedup();
    let mut seen: Vec<String> = Vec::new();
    for m in ctx.manifests.iter().filter(|m| !matches!(m.kind, Kind::Nbgv)) {
        if let Some(v) = &m.version {
            if !seen.contains(v) {
                seen.push(v.clone());
            }
        }
    }
    let suggested = ctx.suggestion(&cs);
    let tag = suggested.version.as_deref().and_then(Ver::parse).map(|v| ctx.tag_for(&v)).unwrap_or_default();
    let notes_path = ctx.notes_path(&tag);
    let dirty_out = git_ok(ws, &["diff", "--name-only", "HEAD"]).unwrap_or_default();
    let dirty: Vec<&str> = dirty_out.lines().filter(|l| !l.trim().is_empty()).collect();
    StatusOut {
        version: ctx.base().to_string(),
        latest_tag: ctx.tags.first().map(|t| t.name.clone()),
        latest_stable: ctx.tags.iter().find(|t| !t.ver.is_pre()).map(|t| t.name.clone()),
        tag_prefix: ctx.prefix(),
        branch: git_ok(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).ok(),
        dirty: dirty.len(),
        dirty_files: dirty.iter().take(20).map(|s| s.to_string()).collect(),
        releases: past_releases(ctx, 8),
        stacks,
        drift: if seen.len() > 1 { seen } else { Vec::new() },
        commits_since: cs.len(),
        breaking: cs.iter().filter(|c| c.breaking).count(),
        feats: count("feat"),
        fixes: count("fix") + count("perf"),
        bumps: ["patch", "minor", "major", "prerelease"]
            .into_iter()
            .filter_map(|b| Some((b, ctx.base().resolve(b).ok()?.to_string())))
            .collect(),
        suggested,
        ci_workflows: ctx.ci_workflows(),
        mode: ctx.mode(),
        repo: notes::github_repo(ws).map(|(o, r)| format!("{o}/{r}")),
        notes_exists: ws.join(&notes_path).is_file(),
        notes_path,
        config: ws.join(DEFAULT_CONFIG_PATH).is_file(),
        manifests: ctx.manifests.clone(),
    }
}

fn render_status(s: &StatusOut) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "version {} · latest tag {} · branch {} · {}\n",
        s.version,
        s.latest_tag.as_deref().unwrap_or("none"),
        s.branch.as_deref().unwrap_or("?"),
        if s.dirty == 0 { "clean".to_string() } else { format!("{} uncommitted", s.dirty) }
    ));
    o.push_str(&format!("stacks: {}\n", if s.stacks.is_empty() { "none detected".into() } else { s.stacks.join(", ") }));
    o.push_str(&format!("version files ({}):\n", s.manifests.len()));
    for m in &s.manifests {
        let flag = if !s.drift.is_empty() && m.version.as_deref().is_some_and(|v| v != s.version) { " ≠" } else { "" };
        o.push_str(&format!(
            "  {} {}{}{}\n",
            m.path,
            m.version.as_deref().unwrap_or("-"),
            flag,
            m.note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()
        ));
    }
    if !s.drift.is_empty() {
        o.push_str(&format!("⚠ version drift: {} (a bump aligns them)\n", s.drift.join(", ")));
    }
    let since = s.latest_stable.as_deref().map(|t| format!(" since {t}")).unwrap_or_default();
    o.push_str(&format!("commits{since}: {} ({} breaking, {} feat, {} fix/perf)\n", s.commits_since, s.breaking, s.feats, s.fixes));
    o.push_str(&format!(
        "suggested: {} → {} ({})\n",
        s.suggested.bump,
        s.suggested.version.as_deref().unwrap_or("-"),
        s.suggested.reason
    ));
    o.push_str(&format!(
        "release mode: {}{}{}\n",
        s.mode,
        if s.ci_workflows.is_empty() { String::new() } else { format!(" (tag-triggered: {})", s.ci_workflows.join(", ")) },
        s.repo.as_ref().map(|r| format!(" · github.com/{r}")).unwrap_or_default()
    ));
    if let Some(v) = &s.suggested.version {
        o.push_str(&format!("next: vx release preflight {v} · vx release notes {v} --write · vx release publish {v} --dry-run\n"));
    }
    o
}

fn render_changes(changes: &[FileChange], dry: bool) -> String {
    if changes.is_empty() {
        return "all version files already match; nothing to change\n".into();
    }
    let mut o = format!("{} {} file(s):\n", if dry { "would update" } else { "updated" }, changes.len());
    for c in changes {
        o.push_str(&format!("  {} {} → {}\n", c.path, c.from.as_deref().unwrap_or("?"), c.to));
        for ch in &c.changes {
            o.push_str(&format!("    {}: {} → {}\n", ch.line, ch.before, ch.after));
        }
    }
    o
}

/// Prepend `section` to a Keep-a-Changelog style file (before the newest release heading).
fn prepend_changelog(ws: &Path, section: &str, tag: &str, ver: &Ver) -> Result<String, String> {
    let path = ws.join("CHANGELOG.md");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let crlf = old.contains("\r\n");
    let old = old.replace("\r\n", "\n");
    if old.lines().any(|l| l.starts_with("## ") && (l.contains(tag) || l.contains(&format!("[{ver}]")))) {
        return Ok(format!("CHANGELOG.md already has an entry for {tag}; left unchanged"));
    }
    let mut lines: Vec<&str> = old.lines().collect();
    if lines.is_empty() {
        lines.extend(["# Changelog", ""]);
    }
    let at = lines
        .iter()
        .position(|l| l.starts_with("## ") && !l.to_ascii_lowercase().contains("unreleased"))
        .unwrap_or(lines.len());
    let mut block: Vec<&str> = section.trim_end().lines().collect();
    block.push("");
    lines.splice(at..at, block);
    let mut text = lines.join("\n") + "\n";
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    std::fs::write(&path, text).map_err(|e| format!("CHANGELOG.md: {e}"))?;
    Ok("CHANGELOG.md updated".into())
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

fn strs(a: &Value, k: &str) -> Vec<String> {
    match a.get(k) {
        Some(Value::Array(v)) => v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(x)) if !x.trim().is_empty() => vec![x.trim().to_string()],
        _ => Vec::new(),
    }
}

fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// `vx release <action>` entry point. Text for agents, or JSON (`json: true`) for the UI.
/// Every state-changing action (and each gate run) is written to the activity log with its
/// `source` (`ui`, `agent` or `cli`) so the Releases page can show who did what.
pub fn run(ws: &Path, a: &Value) -> Result<String, String> {
    let action = s(a, "action").unwrap_or("status");
    let as_json = flag(a, "json");
    let source = s(a, "source").unwrap_or("cli");
    let pretty = |v: Value| serde_json::to_string(&v).map_err(|e| e.to_string());
    if action == "activity" {
        let items = activity::read(ws, 60);
        return if as_json { pretty(json!({ "items": items })) } else { Ok(activity::render(&items)) };
    }
    let ctx = Ctx::load(ws)?;
    let version = s(a, "version");
    let id = activity::now_ms();
    let log = |action: &str, entry: Value| activity::record(ws, id, source, action, entry);
    match action {
        "status" => {
            let st = status(&ctx);
            if as_json {
                return pretty(serde_json::to_value(&st).map_err(|e| e.to_string())?);
            }
            Ok(render_status(&st))
        }
        "preflight" => {
            let target = ctx.target(version)?;
            let report = flow::preflight(&ctx, &target, !flag(a, "skip_tests"));
            let failed = report.checks.iter().filter(|c| c.status == "fail").count();
            let warned = report.checks.iter().filter(|c| c.status == "warn").count();
            log("preflight", json!({
                "ok": report.ok, "version": report.version, "tag": report.tag,
                "summary": if failed > 0 { format!("{failed} check(s) failed") } else if warned > 0 { format!("passed with {warned} warning(s)") } else { "all checks passed".into() },
                "detail": serde_json::to_value(&report).unwrap_or(Value::Null),
            }));
            if as_json {
                return pretty(serde_json::to_value(&report).map_err(|e| e.to_string())?);
            }
            Ok(report.render())
        }
        "notes" => {
            let target = ctx.target(version)?;
            let tag = ctx.tag_for(&target);
            let cs = ctx.commits_since(Some(&target));
            let n = notes::build_notes(ws, &tag, ctx.prev_tag(Some(&target)), &cs);
            let mut msgs: Vec<String> = Vec::new();
            let mut path = None;
            if flag(a, "write") || s(a, "notes_file").is_some() {
                let rel = s(a, "notes_file").map(str::to_string).unwrap_or_else(|| ctx.notes_path(&tag));
                let full = ws.join(&rel);
                if let Some(dir) = full.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&full, &n.markdown).map_err(|e| format!("{rel}: {e}"))?;
                msgs.push(format!("wrote {rel}"));
                path = Some(rel);
            }
            if flag(a, "changelog") {
                msgs.push(prepend_changelog(ws, &n.markdown, &tag, &target)?);
            }
            if !msgs.is_empty() {
                log("notes", json!({
                    "ok": true, "version": target.to_string(), "tag": tag, "path": path,
                    "summary": format!("{} commit(s) · {}", n.commits, msgs.join(" · ")),
                }));
            }
            if as_json {
                return pretty(json!({
                    "version": target.to_string(), "tag": tag, "markdown": n.markdown, "commits": n.commits,
                    "compare_url": n.compare_url, "written": path, "messages": msgs,
                }));
            }
            let mut out = n.markdown;
            for m in msgs {
                out.push_str(&format!("\n— {m}"));
            }
            Ok(out)
        }
        "bump" => {
            let Some(arg) = version else { return Err("usage: vx release bump <patch|minor|major|prerelease|X.Y.Z> [--dry-run] [--only FILE]".into()) };
            let target = ctx.base().resolve(arg)?;
            let changes = manifest::plan(&ctx.manifests, &strs(a, "only"), &target);
            let dry = flag(a, "dry_run");
            if !dry {
                manifest::write_all(ws, &changes)?;
                log("bump", json!({
                    "ok": true, "version": target.to_string(), "tag": ctx.tag_for(&target),
                    "summary": format!("{} → {target} in {} file(s)", ctx.base(), changes.len()),
                }));
            }
            if as_json {
                return pretty(json!({ "version": target.to_string(), "dry_run": dry, "files": changes }));
            }
            Ok(format!("{} → {target}\n{}", ctx.base(), render_changes(&changes, dry)))
        }
        "publish" => {
            let opts = flow::PublishOpts {
                version: version.map(str::to_string),
                dry_run: flag(a, "dry_run"),
                skip_tests: flag(a, "skip_tests"),
                no_push: flag(a, "no_push"),
                publish: flag(a, "publish"),
                notes_file: s(a, "notes_file").map(str::to_string),
                assets: strs(a, "assets"),
                only: strs(a, "only"),
            };
            let want = ctx.target(version).ok();
            let (want_ver, want_tag) = (want.as_ref().map(|v| v.to_string()), want.as_ref().map(|v| ctx.tag_for(v)));
            if !opts.dry_run {
                log("publish", json!({
                    "phase": "running", "dry_run": false, "version": want_ver, "tag": want_tag,
                    "summary": "releasing…",
                }));
            }
            let out = match flow::publish(&ctx, &opts) {
                Ok(o) => o,
                Err(e) => {
                    log("publish", json!({
                        "ok": false, "dry_run": opts.dry_run, "version": want_ver, "tag": want_tag, "summary": e,
                    }));
                    return Err(e);
                }
            };
            let summary = if out.ok {
                if out.dry_run { "plan ready, nothing changed".to_string() } else { format!("released {}", out.tag) }
            } else {
                let failed = out.preflight.iter().chain(&out.steps).filter(|c| c.status == "fail").count();
                if out.dry_run { format!("blocked: {failed} check(s) failed") } else { format!("stopped: {failed} step(s) failed") }
            };
            log("publish", json!({
                "ok": out.ok, "dry_run": out.dry_run, "version": out.version, "tag": out.tag, "url": out.url,
                "summary": summary, "detail": serde_json::to_value(&out).unwrap_or(Value::Null),
            }));
            if as_json {
                return pretty(serde_json::to_value(&out).map_err(|e| e.to_string())?);
            }
            Ok(out.render())
        }
        "verify" => {
            let target = match version {
                Some(v) => ctx.base().resolve(v)?,
                None => ctx.tags.first().map(|t| t.ver.clone()).ok_or("no release tag to verify; pass a version")?,
            };
            let report = flow::verify(&ctx, &target);
            log("verify", json!({
                "ok": report.ok, "version": report.version, "tag": report.tag,
                "summary": if report.ok { "release verified".to_string() } else { "verification found problems".to_string() },
                "detail": serde_json::to_value(&report).unwrap_or(Value::Null),
            }));
            if as_json {
                return pretty(serde_json::to_value(&report).map_err(|e| e.to_string())?);
            }
            Ok(report.render())
        }
        "body" => {
            let tag = s(a, "tag").ok_or("`body` needs a tag")?;
            let (source, markdown) = flow::release_body(&ctx, tag);
            if as_json {
                return pretty(json!({ "tag": tag, "source": source, "markdown": markdown }));
            }
            Ok(markdown)
        }
        other => Err(format!("unknown release action `{other}` (status|preflight|notes|bump|publish|verify|activity|body)")),
    }
}
