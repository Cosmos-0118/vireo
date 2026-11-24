//! Gated release flow: preflight checks, publish (bump → commit → tag → push / draft release),
//! and post-release verification.

use super::ci::{self, NotesWiring};
use super::manifest::{self, FileChange};
use super::notes::{self, git, git_ok, git_t};
use super::semver::Ver;
use super::Ctx;
use crate::code_intel::util::{command, run_capture};
use crate::code_intel::{check, check_failed, CheckOpts};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Clone)]
pub struct Check {
    pub id: &'static str,
    /// `ok` | `warn` | `fail` | `skip`
    pub status: &'static str,
    pub label: String,
    pub detail: String,
}

fn ck(id: &'static str, status: &'static str, label: &str, detail: impl Into<String>) -> Check {
    Check { id, status, label: label.to_string(), detail: detail.into() }
}

fn mark(status: &str) -> &'static str {
    match status {
        "ok" => "✔",
        "warn" => "⚠",
        "fail" => "✖",
        _ => "·",
    }
}

fn render_checks(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| format!("{} {}{}\n", mark(c.status), c.label, if c.detail.is_empty() { String::new() } else { format!(" — {}", c.detail) }))
        .collect()
}

#[derive(Serialize)]
pub struct Report {
    pub version: String,
    pub tag: String,
    pub ok: bool,
    pub checks: Vec<Check>,
    #[serde(skip)]
    verifying: bool,
}

impl Report {
    fn new(version: &Ver, tag: String, checks: Vec<Check>) -> Report {
        Report { version: version.to_string(), tag, ok: !checks.iter().any(|c| c.status == "fail"), checks, verifying: false }
    }

    pub fn render(&self) -> String {
        let head = format!("release {} ({})\n", self.version, self.tag);
        let tail = match (self.verifying, self.ok) {
            (false, true) => "ready to release\n",
            (false, false) => "✖ not ready: fix the failed checks above\n",
            (true, true) => "release looks healthy\n",
            (true, false) => "✖ release has problems (see above)\n",
        };
        format!("{head}{}{tail}", render_checks(&self.checks))
    }
}

fn gh_cmd(ws: &Path, args: &[&str], secs: u64) -> Result<(bool, String, String), String> {
    let mut c = command("gh");
    c.current_dir(ws).args(args);
    let r = run_capture(c, Duration::from_secs(secs)).map_err(|e| format!("gh unavailable: {e}"))?;
    if r.timed_out {
        return Err(format!("`gh {}` timed out after {secs}s", args.first().unwrap_or(&"")));
    }
    Ok((r.code == 0, r.stdout.trim().to_string(), r.stderr.trim().to_string()))
}

/// Notes for an existing release tag, as GitHub shows them: the release body from `gh` when
/// available, else the saved notes file. Returns `(source, markdown)`.
pub fn release_body(ctx: &Ctx, tag: &str) -> (&'static str, String) {
    if ctx.mode() != "local" {
        if let Ok((true, body, _)) = gh_cmd(&ctx.ws, &["release", "view", tag, "--json", "body", "--jq", ".body"], 20) {
            if !body.trim().is_empty() {
                return ("github", body);
            }
        }
    }
    let rel = ctx.notes_path(tag);
    match std::fs::read_to_string(ctx.ws.join(&rel)) {
        Ok(t) if !t.trim().is_empty() => ("file", t),
        _ => ("none", String::new()),
    }
}

fn has_origin(ws: &Path) -> bool {
    git(ws, &["remote", "get-url", "origin"]).is_ok_and(|g| g.ok)
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().filter(|l| !l.trim().is_empty()).take(n).collect::<Vec<_>>().join("; ")
}

pub fn preflight(ctx: &Ctx, target: &Ver, tests: bool) -> Report {
    let ws = &ctx.ws;
    let tag = ctx.tag_for(target);
    let mut v: Vec<Check> = Vec::new();
    let mode = ctx.mode();

    if !git(ws, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|g| g.ok && g.out == "true") {
        v.push(ck("git", "fail", "Git repository", "this folder is not a git repository"));
        return Report::new(target, tag, v);
    }

    // Working tree.
    let dirty = git_ok(ws, &["diff", "--name-only", "HEAD"]).unwrap_or_default();
    let files: Vec<&str> = dirty.lines().collect();
    v.push(if files.is_empty() {
        ck("clean", "ok", "Working tree clean", "")
    } else {
        let more = if files.len() > 4 { format!(" +{} more", files.len() - 4) } else { String::new() };
        ck("clean", "fail", "Working tree clean", format!("{} uncommitted: {}{more}", files.len(), files[..files.len().min(4)].join(", ")))
    });

    // Branch.
    let branch = git_ok(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let default = git_ok(ws, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .map(|b| b.trim_start_matches("origin/").to_string());
    v.push(if branch == "HEAD" || branch.is_empty() {
        ck("branch", "fail", "On a branch", "HEAD is detached")
    } else if default.as_deref().map_or(!matches!(branch.as_str(), "main" | "master"), |d| d != branch) {
        ck("branch", "warn", "Release branch", format!("on `{branch}`, default is `{}`", default.as_deref().unwrap_or("main")))
    } else {
        ck("branch", "ok", "Release branch", format!("`{branch}`"))
    });

    // Remote sync.
    let origin = has_origin(ws);
    if origin {
        let _ = git_t(ws, &["fetch", "--quiet", "origin"], 30);
        match git_ok(ws, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
            Ok(up) => {
                let counts = git_ok(ws, &["rev-list", "--left-right", "--count", &format!("{up}...HEAD")]).unwrap_or_default();
                let mut it = counts.split_whitespace().filter_map(|n| n.parse::<u32>().ok());
                let (behind, ahead) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
                v.push(if behind > 0 {
                    ck("sync", "fail", "Up to date with remote", format!("{behind} commit(s) behind {up}: pull first"))
                } else if ahead > 0 {
                    ck("sync", "warn", "Up to date with remote", format!("{ahead} unpushed commit(s) will be pushed with the release"))
                } else {
                    ck("sync", "ok", "Up to date with remote", format!("in sync with {up}"))
                });
            }
            Err(_) => v.push(ck("sync", "warn", "Up to date with remote", "branch has no upstream; publish pushes it to origin")),
        }
    } else {
        v.push(ck("sync", "warn", "Remote", "no `origin` remote: the tag will stay local"));
    }

    // Tag availability.
    let local = git(ws, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_ok_and(|g| g.ok);
    let remote = if origin { git_t(ws, &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")], 30) } else { Ok(notes::GitOut { ok: true, out: String::new(), err: String::new() }) };
    v.push(if local {
        ck("tag", "fail", "Tag available", format!("`{tag}` already exists locally"))
    } else {
        match remote {
            Ok(g) if g.ok && !g.out.is_empty() => ck("tag", "fail", "Tag available", format!("`{tag}` already exists on origin")),
            Ok(g) if g.ok => ck("tag", "ok", "Tag available", format!("`{tag}`")),
            _ => ck("tag", "warn", "Tag available", format!("`{tag}` is free locally; could not reach origin to check")),
        }
    });

    // Version ordering.
    // Re-read the tags: the fetch above may have brought in releases cut elsewhere.
    let tags = if origin { notes::tags(ws) } else { ctx.tags.clone() };
    v.push(match tags.first() {
        Some(t) if t.ver >= *target => ck("version", "fail", "Version increases", format!("{target} is not newer than {}", t.name)),
        Some(t) => ck("version", "ok", "Version increases", format!("{} → {target}", t.name)),
        None => ck("version", "ok", "Version increases", format!("first release {target}")),
    });

    // Manifests.
    let mut seen: Vec<&str> = Vec::new();
    for m in ctx.manifests.iter().filter(|m| m.kind != manifest::Kind::Nbgv) {
        if let Some(x) = m.version.as_deref() {
            if !seen.contains(&x) {
                seen.push(x);
            }
        }
    }
    v.push(if ctx.manifests.is_empty() {
        ck("manifests", "warn", "Version files", "none found: only the tag is created (add `.vireo/release.json` for custom files)")
    } else if seen.len() > 1 {
        ck("manifests", "warn", "Version files", format!("{} files disagree ({}); the bump aligns them", ctx.manifests.len(), seen.join(", ")))
    } else {
        ck("manifests", "ok", "Version files", format!("{} file(s) at {}", ctx.manifests.len(), seen.first().copied().unwrap_or("?")))
    });

    // Forge auth.
    if notes::github_repo(ws).is_some() {
        let need = mode == "gh";
        v.push(match gh_cmd(ws, &["auth", "status"], 20) {
            Ok((true, out, err)) => {
                let who = lazy_re!(r"account (\S+)").captures(&format!("{out}\n{err}")).map(|c| format!("as {}", &c[1])).unwrap_or_default();
                ck("gh", "ok", "GitHub CLI authenticated", who)
            }
            Ok((false, ..)) => ck("gh", if need { "fail" } else { "warn" }, "GitHub CLI authenticated", "run `gh auth login`"),
            Err(e) => ck("gh", if need { "fail" } else { "warn" }, "GitHub CLI authenticated", e),
        });
    }

    // Release mode & CI workflow wiring.
    let wfs = ctx.ci_workflow_details();
    let wf_names: Vec<String> = wfs.iter().map(|w| w.file.clone()).collect();
    v.push(match mode.as_str() {
        "ci" => {
            if !wfs.is_empty() && !wfs.iter().any(|w| ci::tag_matches_patterns(&tag, &w.tag_patterns)) {
                ck(
                    "mode",
                    "warn",
                    "Release mode",
                    format!("`{tag}` does not match `push.tags` filters in {}; CI may not trigger", wf_names.join(", ")),
                )
            } else {
                ck(
                    "mode",
                    "ok",
                    "Release mode",
                    format!("pushing `{tag}` triggers {}", if wf_names.is_empty() { "CI".to_string() } else { wf_names.join(", ") }),
                )
            }
        }
        "gh" => ck("mode", "ok", "Release mode", "draft GitHub release created with `gh`"),
        _ => ck("mode", "ok", "Release mode", "local: commit and tag only"),
    });
    if mode == "ci" && !wfs.is_empty() {
        let notes_rel = ctx.notes_path(&tag);
        v.push(match ci::evaluate_notes_wiring(&wfs, &notes_rel) {
            NotesWiring::Aligned { workflow, source } => {
                ck("ci-notes", "ok", "CI release notes", format!("`{workflow}` reads `{source}`"))
            }
            NotesWiring::Mismatch { workflow, expected, actual } => {
                ck("ci-notes", "warn", "CI release notes", format!("`{workflow}` reads `{expected}`, not `{actual}`"))
            }
            NotesWiring::Changelog { workflow } => {
                if ctx.changelog_has(&tag, target) {
                    ck("ci-notes", "ok", "CI release notes", format!("`{workflow}` reads `CHANGELOG.md` (has `{tag}`)"))
                } else {
                    ck("ci-notes", "warn", "CI release notes", format!("`{workflow}` reads `CHANGELOG.md`, which has no entry for `{tag}` yet"))
                }
            }
            NotesWiring::Missing { workflows, auto_notes } => ck(
                "ci-notes",
                "warn",
                "CI release notes",
                format!(
                    "{} does not read `{notes_rel}` or `CHANGELOG.md`{}; written notes won't reach GitHub",
                    workflows.join(", "),
                    if auto_notes { " (uses GitHub auto-generated notes)" } else { "" }
                ),
            ),
            NotesWiring::NoCi => unreachable!(),
        });
    }

    // Build / test gate.
    v.push(if !tests {
        ck("build", "skip", "Build & tests", "skipped")
    } else {
        let cmds: Vec<Option<&str>> = if ctx.cfg.checks.is_empty() { vec![None] } else { ctx.cfg.checks.iter().map(|c| Some(c.as_str())).collect() };
        let mut failure: Option<String> = None;
        let mut summary = String::new();
        for cmd in cmds {
            let opts = CheckOpts { raw: false, warnings: false, dir: None, cwd: Some(ws.clone()), wait: None, job: None };
            match check(ws, cmd, 900, opts) {
                Ok(r) if check_failed(&r) => {
                    failure = Some(first_lines(&r.lines().filter(|l| l.starts_with('✖')).collect::<Vec<_>>().join("\n"), 3));
                    break;
                }
                Ok(r) => summary = first_lines(&r, 1),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        match failure {
            Some(f) => ck("build", "fail", "Build & tests", f),
            None => ck("build", "ok", "Build & tests", summary),
        }
    });
    Report::new(target, tag, v)
}

pub struct PublishOpts {
    pub version: Option<String>,
    pub dry_run: bool,
    pub skip_tests: bool,
    pub no_push: bool,
    /// `gh` mode: promote the draft to a public release right away.
    pub publish: bool,
    pub notes_file: Option<String>,
    pub assets: Vec<String>,
    pub only: Vec<String>,
}

#[derive(Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub dry_run: bool,
    pub version: String,
    pub tag: String,
    pub mode: String,
    pub preflight: Vec<Check>,
    pub steps: Vec<Check>,
    pub files: Vec<FileChange>,
    pub notes_path: Option<String>,
    pub url: Option<String>,
}

impl Outcome {
    pub fn render(&self) -> String {
        let mut o = format!("release {} ({}) · mode {}{}\n", self.version, self.tag, self.mode, if self.dry_run { " · DRY RUN" } else { "" });
        // Passing checks are noise here: show only what needs attention.
        let notable: Vec<Check> = self.preflight.iter().filter(|c| matches!(c.status, "warn" | "fail")).cloned().collect();
        o.push_str(&render_checks(&notable));
        if !self.ok && self.steps.is_empty() {
            o.push_str("✖ not ready: fix the failed checks above (nothing was changed)\n");
        }
        o.push_str(&render_checks(&self.steps));
        if let Some(u) = &self.url {
            o.push_str(&format!("→ {u}\n"));
        }
        o
    }
}

fn expand_assets(ws: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for p in patterns {
        let direct = ws.join(p);
        if direct.is_file() {
            found.push(direct);
            continue;
        }
        let mut ov = ignore::overrides::OverrideBuilder::new(ws);
        if ov.add(p).is_err() {
            continue;
        }
        let Ok(ov) = ov.build() else { continue };
        let walk = ignore::WalkBuilder::new(ws)
            .max_depth(Some(8))
            .standard_filters(false)
            .overrides(ov)
            .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | "node_modules")))
            .build();
        found.extend(walk.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).map(|e| e.into_path()));
    }
    found.sort();
    found.dedup();
    found
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn publish(ctx: &Ctx, o: &PublishOpts) -> Result<Outcome, String> {
    let ws = &ctx.ws;
    let target = ctx.target(o.version.as_deref())?;
    let tag = ctx.tag_for(&target);
    let mode = ctx.mode();
    let report = preflight(ctx, &target, !o.skip_tests && !o.dry_run);
    let mut out = Outcome {
        ok: report.ok,
        dry_run: o.dry_run,
        version: target.to_string(),
        tag: tag.clone(),
        mode: mode.clone(),
        preflight: report.checks,
        steps: Vec::new(),
        files: Vec::new(),
        notes_path: None,
        url: None,
    };
    // A dry run only reports what a real run would hit; it never fails on preflight.
    if !out.ok && !o.dry_run {
        return Ok(out);
    }
    let step = |out: &mut Outcome, id: &'static str, status: &'static str, label: &str, detail: String| out.steps.push(ck(id, status, label, detail));

    // Plan: version files.
    let changes = manifest::plan(&ctx.manifests, &o.only, &target);
    let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    step(&mut out, "bump", if changes.is_empty() { "warn" } else { "ok" }, "Bump version files", if changes.is_empty() { "nothing to change".into() } else { format!("{} file(s) → {target}: {}", changes.len(), paths.join(", ")) });

    // Plan: notes. Hand-written / agent-polished notes win over generated ones.
    let default_notes = ctx.notes_path(&tag);
    let notes_rel = o.notes_file.clone().unwrap_or_else(|| default_notes.clone());
    let have_notes = ws.join(&notes_rel).is_file();
    if o.notes_file.is_some() && !have_notes {
        return Err(format!("notes file `{notes_rel}` does not exist"));
    }
    let cs = ctx.commits_since(Some(&target));
    let generated = notes::build_notes(ws, &tag, ctx.prev_tag(Some(&target)), &cs).markdown;
    let stage_notes = ctx.should_stage_notes(&notes_rel);
    let base_notes = if have_notes {
        format!("using {notes_rel}")
    } else {
        format!("generated from {} commit(s) → {notes_rel}", cs.len())
    };
    if mode == "ci" {
        let wfs = ctx.ci_workflow_details();
        match ci::evaluate_notes_wiring(&wfs, &notes_rel) {
            NotesWiring::Aligned { workflow, .. } => step(
                &mut out,
                "notes",
                "ok",
                "Release notes",
                format!(
                    "{base_notes} ({}read by `{workflow}`)",
                    if stage_notes { "committed with release; " } else { "" }
                ),
            ),
            NotesWiring::Mismatch { workflow, expected, .. } => step(
                &mut out,
                "notes",
                "warn",
                "Release notes",
                format!("{base_notes} — `{workflow}` reads `{expected}`, so these notes won't reach GitHub"),
            ),
            NotesWiring::Changelog { workflow } => step(
                &mut out,
                "notes",
                if ctx.changelog_has(&tag, &target) { "ok" } else { "warn" },
                "Release notes",
                format!("{base_notes} (`{workflow}` reads `CHANGELOG.md`)"),
            ),
            NotesWiring::Missing { workflows, .. } => step(
                &mut out,
                "notes",
                "warn",
                "Release notes",
                format!(
                    "{base_notes} — not read by {}{}",
                    workflows.join(", "),
                    if stage_notes { "" } else { " and not committed; won't reach GitHub" }
                ),
            ),
            NotesWiring::NoCi => step(&mut out, "notes", "ok", "Release notes", base_notes),
        }
    } else {
        step(&mut out, "notes", "ok", "Release notes", base_notes);
    }
    out.notes_path = Some(notes_rel.clone());

    let assets = expand_assets(ws, &[o.assets.clone(), ctx.cfg.assets.clone()].concat());
    if mode == "gh" && !assets.is_empty() {
        step(&mut out, "assets", "ok", "Assets", format!("{} file(s) + SHA256SUMS.txt", assets.len()));
    }
    let branch = git_ok(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "HEAD".into());
    step(&mut out, "commit", "ok", "Commit & tag", format!("`chore(release): {tag}` + annotated tag `{tag}`"));
    let can_push = !o.no_push && has_origin(ws);
    step(&mut out, "push", if can_push { "ok" } else { "skip" }, "Push", if can_push { format!("atomic push of `{branch}` + `{tag}` to origin") } else if o.no_push { "--no-push: stays local".into() } else { "no origin remote: stays local".into() });
    match (mode.as_str(), can_push) {
        ("ci", true) => step(&mut out, "release", "ok", "Release", format!("CI builds and publishes after the tag push ({})", ctx.ci_workflows().join(", "))),
        ("gh", true) => step(&mut out, "release", "ok", "GitHub release", if o.publish { "created as draft, then published".into() } else { format!("created as DRAFT: review, then promote on GitHub or run `gh release edit {tag} --draft=false`") }),
        _ => {}
    }
    out.files = changes;
    if o.dry_run {
        return Ok(out);
    }

    // ---- Execute ----
    out.steps.clear();
    if let Err(e) = manifest::write_all(ws, &out.files) {
        out.ok = false;
        step(&mut out, "bump", "fail", "Bump version files", e);
        return Ok(out);
    }
    let n = out.files.len();
    step(&mut out, "bump", "ok", "Bump version files", format!("{n} file(s) → {target}"));

    let full_notes = ws.join(&notes_rel);
    if !have_notes {
        if let Some(d) = full_notes.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&full_notes, &generated) {
            manifest::restore(ws, &out.files);
            return Err(format!("{notes_rel}: {e}. Version files were restored."));
        }
    }
    let rollback = |out: &mut Outcome, why: String| {
        manifest::restore(ws, &out.files);
        if !have_notes {
            let _ = std::fs::remove_file(&full_notes);
        }
        let _ = git(ws, &["reset", "-q"]);
        out.ok = false;
        out.steps.push(ck("commit", "fail", "Commit", format!("{why}. Version files were restored.")));
    };
    let mut add: Vec<String> = paths;
    if stage_notes {
        add.push(notes_rel.clone());
    }
    let mut committed = false;
    if !add.is_empty() {
        let mut args = vec!["add", "--"];
        args.extend(add.iter().map(String::as_str));
        if let Err(e) = git_ok(ws, &args) {
            rollback(&mut out, e);
            return Ok(out);
        }
        // Only commit when staging actually introduced changes (e.g. an as-is release whose
        // notes file was already committed in an earlier commit has nothing to commit).
        let has_staged = !git(ws, &["diff", "--cached", "--quiet"]).is_ok_and(|g| g.ok);
        if has_staged {
            if let Err(e) = git_ok(ws, &["commit", "-q", "-m", &format!("chore(release): {tag}")]) {
                rollback(&mut out, e);
                return Ok(out);
            }
            committed = true;
        }
    }
    if let Err(e) = git_ok(ws, &["tag", "-a", &tag, "-m", &tag]) {
        out.ok = false;
        let extra = if committed { " The release commit exists locally;" } else { "" };
        step(&mut out, "tag", "fail", "Tag", format!("{e}.{extra} fix and run `git tag -a {tag} -m {tag}`"));
        return Ok(out);
    }
    let commit_detail = if committed {
        format!("`chore(release): {tag}` + `{tag}`")
    } else {
        format!("tagged HEAD as `{tag}` (no version/notes diff to commit)")
    };
    step(&mut out, "commit", "ok", "Commit & tag", commit_detail);

    if !can_push {
        step(&mut out, "push", "skip", "Push", format!("stayed local. Push with `git push --atomic origin {branch} {tag}`"));
        return Ok(out);
    }
    match git_t(ws, &["push", "--atomic", "origin", &branch, &format!("refs/tags/{tag}")], 180) {
        Ok(g) if g.ok => step(&mut out, "push", "ok", "Push", format!("`{branch}` + `{tag}` → origin")),
        Ok(g) => {
            out.ok = false;
            step(&mut out, "push", "fail", "Push", format!("{}. Commit and tag exist locally; retry `git push --atomic origin {branch} {tag}`", first_lines(&g.err, 2)));
            return Ok(out);
        }
        Err(e) => {
            out.ok = false;
            step(&mut out, "push", "fail", "Push", e);
            return Ok(out);
        }
    }
    if mode == "ci" {
        step(&mut out, "release", "ok", "Release", format!("CI is building `{tag}`: `vx release verify {target}` to follow it"));
        return Ok(out);
    }
    if mode != "gh" {
        return Ok(out);
    }

    // gh mode: draft release with notes and checksummed assets.
    let mut files: Vec<String> = assets.iter().map(|p| p.to_string_lossy().to_string()).collect();
    if !assets.is_empty() {
        let sums: Result<String, String> = assets
            .iter()
            .map(|p| Ok(format!("{}  {}\n", sha256(p)?, p.file_name().and_then(|n| n.to_str()).unwrap_or("asset"))))
            .collect();
        let sums_path = ws.join(".vireo/release-assets").join(&tag).join("SHA256SUMS.txt");
        match sums.and_then(|s| {
            std::fs::create_dir_all(sums_path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&sums_path, s).map_err(|e| e.to_string())
        }) {
            Ok(()) => files.push(sums_path.to_string_lossy().to_string()),
            Err(e) => step(&mut out, "assets", "warn", "Checksums", e),
        }
    }
    let notes_arg = full_notes.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["release", "create", &tag, "--draft", "--verify-tag", "--title", &tag, "--notes-file", &notes_arg];
    if target.is_pre() {
        args.push("--prerelease");
    }
    args.extend(files.iter().map(String::as_str));
    match gh_cmd(ws, &args, 600) {
        Ok((true, url, _)) => {
            out.url = url.lines().last().map(str::to_string);
            step(&mut out, "release", "ok", "GitHub release", "draft created".into());
            if o.publish {
                match gh_cmd(ws, &["release", "edit", &tag, "--draft=false"], 60) {
                    Ok((true, ..)) => step(&mut out, "publish", "ok", "Published", format!("`{tag}` is public")),
                    Ok((false, _, e)) | Err(e) => {
                        out.ok = false;
                        step(&mut out, "publish", "fail", "Published", first_lines(&e, 2));
                    }
                }
            }
        }
        Ok((false, _, e)) | Err(e) => {
            out.ok = false;
            step(&mut out, "release", "fail", "GitHub release", format!("{}. Tag is pushed; retry `gh release create {tag} --draft --verify-tag`", first_lines(&e, 2)));
        }
    }
    Ok(out)
}

pub fn verify(ctx: &Ctx, target: &Ver) -> Report {
    let ws = &ctx.ws;
    let tag = ctx.tag_for(target);
    let mut v: Vec<Check> = Vec::new();
    v.push(if git(ws, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_ok_and(|g| g.ok) {
        ck("tag-local", "ok", "Local tag", format!("`{tag}`"))
    } else {
        ck("tag-local", "fail", "Local tag", format!("`{tag}` not found"))
    });
    if has_origin(ws) {
        v.push(match git_t(ws, &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")], 30) {
            Ok(g) if g.ok && !g.out.is_empty() => ck("tag-remote", "ok", "Remote tag", "present on origin"),
            Ok(g) if g.ok => ck("tag-remote", "fail", "Remote tag", "not pushed to origin"),
            _ => ck("tag-remote", "warn", "Remote tag", "could not reach origin"),
        });
    }
    if notes::github_repo(ws).is_some() {
        v.push(match gh_cmd(ws, &["release", "view", &tag, "--json", "isDraft,isPrerelease,url,assets"], 30) {
            Ok((true, out, _)) => {
                let j: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
                let n = j["assets"].as_array().map_or(0, Vec::len);
                let sums = j["assets"].as_array().is_some_and(|a| a.iter().any(|x| x["name"].as_str().is_some_and(|s| s.to_ascii_lowercase().contains("sha256"))));
                let state = if j["isDraft"].as_bool() == Some(true) { "draft" } else if j["isPrerelease"].as_bool() == Some(true) { "prerelease" } else { "published" };
                ck("release", if state == "draft" { "warn" } else { "ok" }, "GitHub release", format!("{state}, {n} asset(s){}, {}", if sums { " incl. checksums" } else { "" }, j["url"].as_str().unwrap_or("")))
            }
            Ok((false, ..)) => ck("release", "warn", "GitHub release", "not found (yet)"),
            Err(e) => ck("release", "warn", "GitHub release", e),
        });
        v.push(match gh_cmd(ws, &["run", "list", "--branch", &tag, "--limit", "3", "--json", "name,status,conclusion,url"], 30) {
            Ok((true, out, _)) => match serde_json::from_str::<Value>(&out).ok().and_then(|j| j.as_array().cloned()) {
                Some(runs) if !runs.is_empty() => {
                    let r = &runs[0];
                    let (st, con) = (r["status"].as_str().unwrap_or(""), r["conclusion"].as_str().unwrap_or(""));
                    let status = match (st, con) {
                        ("completed", "success") => "ok",
                        ("completed", _) => "fail",
                        _ => "warn",
                    };
                    ck("ci", status, "CI run", format!("{} {}{} {}", r["name"].as_str().unwrap_or("workflow"), st, if con.is_empty() { String::new() } else { format!("/{con}") }, r["url"].as_str().unwrap_or("")))
                }
                _ => ck("ci", "skip", "CI run", "no workflow run for this tag"),
            },
            _ => ck("ci", "skip", "CI run", "unavailable"),
        });
    }
    Report { verifying: true, ..Report::new(target, tag, v) }
}
