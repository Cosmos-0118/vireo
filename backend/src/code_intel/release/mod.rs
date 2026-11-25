//! Polyglot release engine behind `vx release` and the Releases page.
//!
//! Stages: `status` (inspect & plan) → `preflight` (gates) → `notes` (Conventional-Commit
//! changelog) → `bump` (atomic version sync across stacks) → `publish` (commit, tag, push or
//! draft GitHub release) → `verify`. Every stage is a plain function over [`Ctx`]; [`run`] is the
//! single entry point and renders text for agents or JSON (`"json": true`) for the UI.

mod activity;
mod ci;
mod flow;
mod manifest;
mod notes;
mod semver;
#[cfg(test)]
mod tests;

use ci::{CiWorkflow, NotesWiring};
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

    /// Detailed analysis of workflow files that run on a pushed tag.
    pub fn ci_workflow_details(&self) -> Vec<CiWorkflow> {
        ci::scan_workflows(&self.ws)
    }

    /// Workflow files that run on a pushed tag.
    pub fn ci_workflows(&self) -> Vec<String> {
        self.ci_workflow_details().into_iter().map(|w| w.file).collect()
    }

    /// Repo-relative notes directory read by a tag-triggered CI workflow, if any.
    pub fn ci_notes_dir(&self) -> Option<String> {
        let wfs = self.ci_workflow_details();
        wfs.iter()
            .filter(|w| w.creates_release)
            .find_map(|w| w.notes_dir.clone())
            .or_else(|| wfs.into_iter().find_map(|w| w.notes_dir))
    }

    pub fn mode(&self) -> String {
        match self.cfg.mode.as_deref() {
            Some(m @ ("ci" | "gh" | "local")) => m.to_string(),
            _ if !self.ci_workflows().is_empty() => "ci".into(),
            _ if notes::github_repo(&self.ws).is_some() => "gh".into(),
            _ => "local".into(),
        }
    }

    /// Where the release notes for `tag` live: follows the CI workflow's notes dir when present,
    /// else next to hand-written ones in `.github/release-notes/`, else `.vireo/release-notes/`.
    pub fn notes_path(&self, tag: &str) -> String {
        if let Some(dir) = self.ci_notes_dir() {
            return format!("{dir}/{tag}.md");
        }
        if self.ws.join(".github/release-notes").is_dir() {
            format!(".github/release-notes/{tag}.md")
        } else {
            format!(".vireo/release-notes/{tag}.md")
        }
    }

    /// True when `publish` should stage and commit `notes_rel` alongside the version bump.
    pub fn should_stage_notes(&self, notes_rel: &str) -> bool {
        let rel = notes_rel.replace('\\', "/");
        let rel = rel.trim_start_matches("./");
        if rel.starts_with(".github/") || rel.starts_with("docs/") {
            return true;
        }
        self.ci_notes_dir().is_some_and(|d| {
            let prefix = format!("{}/", d.trim_end_matches('/'));
            rel.starts_with(&prefix)
        })
    }

    pub fn changelog_has(&self, tag: &str, ver: &Ver) -> bool {
        let Ok(old) = std::fs::read_to_string(self.ws.join("CHANGELOG.md")) else { return false };
        old.lines().any(|l| l.starts_with("## ") && (l.contains(tag) || l.contains(&format!("[{ver}]"))))
    }

    /// Warning when in `ci` mode and the tag-triggered workflow won't pick up `notes_rel`.
    pub fn ci_notes_warning(&self, notes_rel: &str, tag: Option<&str>, ver: Option<&Ver>) -> Option<String> {
        if self.mode() != "ci" {
            return None;
        }
        let wfs = self.ci_workflow_details();
        match ci::evaluate_notes_wiring(&wfs, notes_rel) {
            NotesWiring::NoCi | NotesWiring::Aligned { .. } => None,
            NotesWiring::Mismatch { workflow, expected, actual } => Some(format!(
                "CI workflow `{workflow}` reads `{expected}`, not `{actual}` — write notes to `{expected}` or update the workflow"
            )),
            NotesWiring::Changelog { workflow } => match (tag, ver) {
                (Some(t), Some(v)) if !self.changelog_has(t, v) => Some(format!(
                    "CI workflow `{workflow}` reads `CHANGELOG.md`, which has no entry for `{t}` yet (run `vx release notes --changelog`)"
                )),
                _ => None,
            },
            NotesWiring::Missing { workflows, auto_notes } => Some(format!(
                "CI workflow ({}) does not appear to read `{notes_rel}` or `CHANGELOG.md`{}; notes written by `vx` won't reach GitHub unless the workflow reads them",
                workflows.join(", "),
                if auto_notes { " (uses GitHub auto-generated notes)" } else { "" }
            )),
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
    unconventional: usize,
    suggested: notes::Suggestion,
    /// `patch|minor|major|prerelease` -> the version each would produce.
    bumps: BTreeMap<&'static str, String>,
    ci_workflows: Vec<String>,
    ci_reads_notes: bool,
    ci_notes_source: Option<String>,
    ci_notes_warning: Option<String>,
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
    let explicit = |k: &str| cs.iter().filter(|c| !c.inferred && c.kind == k).count();
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
    let suggested_ver = suggested.version.as_deref().and_then(Ver::parse);
    let tag = suggested_ver.as_ref().map(|v| ctx.tag_for(v)).unwrap_or_default();
    let notes_path = ctx.notes_path(if tag.is_empty() { "<tag>" } else { &tag });
    let notes_exists = !tag.is_empty() && ws.join(&notes_path).is_file();
    let dirty_out = git_ok(ws, &["diff", "--name-only", "HEAD"]).unwrap_or_default();
    let dirty: Vec<&str> = dirty_out.lines().filter(|l| !l.trim().is_empty()).collect();
    let mode = ctx.mode();
    let wfs = ctx.ci_workflow_details();
    let ci_workflows: Vec<String> = wfs.iter().map(|w| w.file.clone()).collect();
    let wiring = ci::evaluate_notes_wiring(&wfs, &notes_path);
    let (ci_reads_notes, ci_notes_source) = match &wiring {
        NotesWiring::Aligned { source, .. } => (true, Some(source.clone())),
        NotesWiring::Changelog { .. } => (true, Some("CHANGELOG.md".to_string())),
        NotesWiring::Mismatch { expected, .. } => (true, Some(expected.clone())),
        NotesWiring::Missing { .. } | NotesWiring::NoCi => (false, None),
    };
    let ci_notes_warning = ctx.ci_notes_warning(
        &notes_path,
        (!tag.is_empty()).then_some(tag.as_str()),
        suggested_ver.as_ref(),
    );
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
        feats: explicit("feat"),
        fixes: explicit("fix") + explicit("perf"),
        unconventional: notes::unconventional_count(&cs),
        bumps: ["patch", "minor", "major", "prerelease"]
            .into_iter()
            .filter_map(|b| Some((b, ctx.base().resolve(b).ok()?.to_string())))
            .collect(),
        suggested,
        ci_workflows,
        ci_reads_notes,
        ci_notes_source,
        ci_notes_warning,
        mode,
        repo: notes::github_repo(ws).map(|(o, r)| format!("{o}/{r}")),
        notes_exists,
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
    if s.commits_since > 0 && s.unconventional == s.commits_since && s.breaking == 0 {
        o.push_str(&format!("commits{since}: {} (non-conventional; review changes to choose bump)\n", s.commits_since));
    } else if s.unconventional > 0 {
        o.push_str(&format!(
            "commits{since}: {} ({} breaking, {} feat, {} fix/perf, {} non-conventional)\n",
            s.commits_since, s.breaking, s.feats, s.fixes, s.unconventional
        ));
    } else {
        o.push_str(&format!("commits{since}: {} ({} breaking, {} feat, {} fix/perf)\n", s.commits_since, s.breaking, s.feats, s.fixes));
    }
    o.push_str(&format!(
        "suggested: {} → {} ({})\n",
        s.suggested.bump,
        s.suggested.version.as_deref().unwrap_or("-"),
        s.suggested.reason
    ));
    let ci_detail = if s.ci_workflows.is_empty() {
        String::new()
    } else if let Some(src) = &s.ci_notes_source {
        format!(" (tag-triggered: {} · reads {src})", s.ci_workflows.join(", "))
    } else {
        format!(" (tag-triggered: {})", s.ci_workflows.join(", "))
    };
    o.push_str(&format!(
        "release mode: {}{}{}\n",
        s.mode,
        ci_detail,
        s.repo.as_ref().map(|r| format!(" · github.com/{r}")).unwrap_or_default()
    ));
    if let Some(w) = &s.ci_notes_warning {
        o.push_str(&format!("⚠ {w}\n"));
    }
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
            let prev = ctx.prev_tag(Some(&target));
            let tag_exists = notes::tag_exists(ws, &tag);
            let upper = if tag_exists { tag.as_str() } else { "HEAD" };
            let range = prev.map(|t| format!("{}..{upper}", t.name)).unwrap_or_else(|| upper.to_string());
            let cs = notes::commits_between(ws, prev.map(|t| t.name.as_str()), upper);
            let n = notes::build_notes(ws, &tag, prev, &cs);
            let unconv = notes::unconventional_count(&cs);
            let diff_stat = if cs.is_empty() { None } else { notes::diff_stat(ws, prev.map(|t| t.name.as_str()), upper, 15) };

            let target_notes_rel = s(a, "notes_file").map(str::to_string).unwrap_or_else(|| ctx.notes_path(&tag));
            let mut warnings: Vec<String> = Vec::new();
            if tag_exists {
                let unreleased = ctx.commits_since(None);
                let next = ctx
                    .suggestion(&unreleased)
                    .version
                    .or_else(|| ctx.base().resolve("patch").ok().map(|v| v.to_string()));
                if !unreleased.is_empty() {
                    let hint = next.as_deref().map(|v| format!("; for the next release use `{v}`")).unwrap_or_default();
                    warnings.push(format!(
                        "tag `{tag}` already exists — showing commits in `{range}`, not the {} unreleased commit(s) after it{hint}",
                        unreleased.len()
                    ));
                } else {
                    warnings.push(format!(
                        "tag `{tag}` already exists — showing commits in `{range}` (pass `patch|minor|major` or a newer version for a new release)"
                    ));
                }
            } else if let Some(latest) = ctx.tags.first().filter(|latest| target <= latest.ver) {
                warnings.push(format!(
                    "version `{target}` is older than latest tag `{}` on this branch — range `{range}` includes commits from newer releases",
                    latest.name
                ));
            }
            if let Some(w) = ctx.ci_notes_warning(&target_notes_rel, Some(&tag), Some(&target)) {
                warnings.push(w);
            }
            if unconv > 0 {
                let inferred_n = cs.iter().filter(|c| c.inferred).count();
                let how = if inferred_n > 0 {
                    format!(" ({inferred_n} categorized heuristically from subjects)")
                } else {
                    String::new()
                };
                warnings.push(format!(
                    "{unconv}/{} commit(s) do not use Conventional Commits{how} — review against the change summary before publishing",
                    cs.len()
                ));
            }

            let mut msgs: Vec<String> = Vec::new();
            let mut path = None;
            if flag(a, "write") || s(a, "notes_file").is_some() {
                let full = ws.join(&target_notes_rel);
                if tag_exists && full.is_file() {
                    let next = ctx
                        .suggestion(&ctx.commits_since(None))
                        .version
                        .or_else(|| ctx.base().resolve("patch").ok().map(|v| v.to_string()));
                    let hint = next.as_deref().map(|v| format!(" or pass `{v}` for the next release")).unwrap_or_default();
                    return Err(format!(
                        "tag `{tag}` already exists and `{target_notes_rel}` is already written; refusing to overwrite released notes (edit `{target_notes_rel}` directly{hint})"
                    ));
                }
                if let Some(dir) = full.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&full, &n.markdown).map_err(|e| format!("{target_notes_rel}: {e}"))?;
                msgs.push(format!("wrote {target_notes_rel}"));
                path = Some(target_notes_rel.clone());
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
                    "tag_exists": tag_exists, "range": range, "unconventional": unconv,
                    "warnings": warnings, "diff_stat": diff_stat,
                }));
            }
            let mut out = String::new();
            for w in &warnings {
                out.push_str(&format!("⚠ {w}\n"));
            }
            if !warnings.is_empty() {
                out.push('\n');
            }
            out.push_str(&n.markdown);
            for m in msgs {
                out.push_str(&format!("\n— {m}"));
            }
            let bodies: Vec<String> = cs
                .iter()
                .filter(|c| !c.body.is_empty())
                .take(8)
                .map(|c| {
                    let snippet: String = c.body.chars().take(180).collect();
                    let ell = if c.body.chars().count() > 180 { "…" } else { "" };
                    format!("- `{}` {}: {snippet}{ell}", c.hash, c.subject)
                })
                .collect();
            if diff_stat.is_some() || !bodies.is_empty() {
                out.push_str("\n\n---\nChange context (not written to the notes file):\n");
                if let Some(stat) = &diff_stat {
                    out.push_str(stat);
                    out.push('\n');
                }
                if !bodies.is_empty() {
                    out.push_str("\nCommit details:\n");
                    for b in bodies {
                        out.push_str(&b);
                        out.push('\n');
                    }
                }
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
        "draft" => {
            let target = ctx.target(version).ok();
            let (want_ver, want_tag) = (target.as_ref().map(|v| v.to_string()), target.as_ref().map(|v| ctx.tag_for(v)));
            let dry = flag(a, "dry_run");
            match flow::draft(&ctx, version, s(a, "notes_file"), dry) {
                Ok(text) => {
                    if !dry {
                        log("draft", json!({ "ok": true, "version": want_ver, "tag": want_tag, "summary": "draft release staged" }));
                    }
                    Ok(text)
                }
                Err(e) => {
                    log("draft", json!({ "ok": false, "dry_run": dry, "version": want_ver, "tag": want_tag, "summary": e }));
                    Err(e)
                }
            }
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
        other => Err(format!("unknown release action `{other}` (status|preflight|notes|bump|draft|publish|verify|activity|body)")),
    }
}
