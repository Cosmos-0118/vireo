//! Flexible, anchor-producing code search that never drifts to raw `rg`.
//!
//! Features:
//! - Accepts positional/flag `path` scopes AND multiple `--glob` / `-g` filters.
//! - Auto-recovers when a glob or path omits a monorepo prefix (e.g. `--glob "src/**"` or
//!   `--glob "src/components/BranchBar.tsx"` from repo root -> resolves to `**/src/**` or
//!   `frontend/src/components/BranchBar.tsx` with a 1-line note instead of `No files were searched`).
//! - Supports `-F`/`--literal`, `-i`/`--ignore-case`, `-C N`/`--context N` (with anchors!),
//!   `-l`/`--files` (file hit counts), and `--limit N`.

use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

use super::doc::line_hash;
use super::util::{
    command, find_path_candidates, ignored_dir, norm_slashes, rel_of, resolve_with_cwd,
    run_capture, top_level_dirs,
};

#[derive(Clone, Debug)]
pub struct SearchOpts {
    pub globs: Vec<String>,
    pub path: Option<String>,
    pub cwd: Option<PathBuf>,
    pub literal: bool,
    pub ignore_case: bool,
    pub context: usize,
    pub files_only: bool,
    pub limit: usize,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            globs: Vec::new(),
            path: None,
            cwd: None,
            literal: false,
            ignore_case: false,
            context: 0,
            files_only: false,
            limit: 80,
        }
    }
}

pub(crate) type Hit = (String, usize, String);

fn has_glob_meta(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[') || s.contains('{')
}

fn clean_filter(raw: &str) -> String {
    let mut s = raw.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

/// Resolved search scope: workspace-relative target paths (empty = whole workspace), active globs,
/// and human-readable notes describing any smart auto-resolution that occurred.
pub(crate) struct ResolvedScope {
    pub targets: Vec<String>,
    pub globs: Vec<String>,
    pub notes: Vec<String>,
}

/// Resolve user-supplied `path` and `globs` into concrete workspace targets + globs.
pub(crate) fn resolve_scope(
    ws: &Path,
    path: Option<&str>,
    globs: &[String],
    cwd: Option<&Path>,
) -> Result<ResolvedScope, String> {
    let mut targets: Vec<String> = Vec::new();
    let mut out_globs: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Helper: resolve a path-like string (exact cwd -> exact ws -> suffix match in repo).
    let resolve_target = |raw: &str, allow_missing: bool, notes: &mut Vec<String>| -> Result<Vec<String>, String> {
        let cleaned = clean_filter(raw);
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }
        if let Ok(full) = resolve_with_cwd(ws, cwd, &cleaned) {
            if full.exists() {
                let rel = rel_of(ws, &full);
                return Ok(vec![if rel == "." { ".".to_string() } else { rel }]);
            }
        }
        let cands = find_path_candidates(ws, &cleaned, true, true);
        match cands.len() {
            0 => {
                if allow_missing {
                    Ok(Vec::new())
                } else {
                    let tops = top_level_dirs(ws);
                    let tip = if tops.is_empty() {
                        String::new()
                    } else {
                        format!(" (workspace root dirs: {})", tops.join(", "))
                    };
                    Err(format!("path `{raw}` not found{tip}"))
                }
            }
            1 => {
                notes.push(format!("(resolved `{cleaned}` -> `{}`)", cands[0]));
                Ok(cands)
            }
            2..=8 => {
                notes.push(format!("(matched `{cleaned}` across: {})", cands.join(", ")));
                Ok(cands)
            }
            _ => {
                let shown: Vec<&str> = cands.iter().take(6).map(String::as_str).collect();
                Err(format!(
                    "path `{raw}` matches {} places ({}, ...); pass a more specific prefix",
                    cands.len(),
                    shown.join(", ")
                ))
            }
        }
    };

    if let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) {
        targets.extend(resolve_target(p, false, &mut notes)?);
    }

    let cwd_rel: Option<String> = cwd.and_then(|c| {
        let w = norm_slashes(ws);
        let cs = norm_slashes(c);
        if cs.len() > w.len() && cs[..w.len()].eq_ignore_ascii_case(&w) && cs.as_bytes()[w.len()] == b'/' {
            Some(cs[w.len() + 1..].to_string())
        } else {
            None
        }
    });

    for raw_g in globs {
        let g = clean_filter(raw_g);
        if g.is_empty() {
            continue;
        }
        if g.starts_with('!') {
            out_globs.push(g);
            continue;
        }
        // A glob with no wildcard characters (e.g. `--glob src/components/BranchBar.tsx` or
        // `--glob frontend/src`) is really a file/dir path; resolve it smartly!
        if !has_glob_meta(&g) {
            let found = resolve_target(&g, true, &mut notes)?;
            if !found.is_empty() {
                if targets.is_empty() {
                    targets.extend(found);
                } else {
                    // Already have a path target: keep as glob matching each resolved path.
                    for f in found {
                        if ws.join(&f).is_dir() {
                            out_globs.push(format!("{f}/**"));
                        } else {
                            out_globs.push(f);
                        }
                    }
                }
                continue;
            }
        }
        // If running from a subdir (e.g. `frontend/`) and the agent wrote `--glob src/**`,
        // check if the subdir prefix exists before falling back to workspace root.
        if g.contains('/') && !g.starts_with("**/") {
            if let Some(ref cr) = cwd_rel {
                let prefix = g.split('/').next().unwrap_or("");
                if !prefix.is_empty() && !has_glob_meta(prefix) && ws.join(cr).join(prefix).exists() {
                    out_globs.push(format!("{cr}/{g}"));
                    continue;
                }
            }
        }
        out_globs.push(g);
    }

    targets.sort();
    targets.dedup();
    Ok(ResolvedScope { targets, globs: out_globs, notes })
}

enum RgOutcome {
    Unavailable,
    NoFilesSearched,
    Hits(Vec<Hit>),
}

fn rg_search(
    ws: &Path,
    query: &str,
    targets: &[String],
    globs: &[String],
    opts: &SearchOpts,
) -> Result<RgOutcome, String> {
    let mut cmd = command("rg");
    let max_per_file = if opts.files_only { "1000" } else if opts.limit > 80 { "50" } else { "20" };
    cmd.args([
        "-n",
        "--no-heading",
        "--with-filename",
        "--max-count",
        max_per_file,
        "--glob",
        "!.git",
        "--glob",
        "!node_modules",
        "--glob",
        "!target",
        "--glob",
        "!dist",
    ]);
    if opts.ignore_case {
        cmd.arg("-i");
    } else {
        cmd.arg("--smart-case");
    }
    if opts.literal {
        cmd.arg("-F");
    }
    for g in globs {
        cmd.args(["--glob", g]);
    }
    cmd.args(["--", query]);
    for t in targets {
        cmd.arg(t);
    }
    cmd.current_dir(ws);
    let Ok(out) = run_capture(cmd, Duration::from_secs(20)) else {
        return Ok(RgOutcome::Unavailable);
    };
    if out.code == 2 && out.stdout.trim().is_empty() {
        let low = out.stderr.to_lowercase();
        if low.contains("no files were searched") {
            return Ok(RgOutcome::NoFilesSearched);
        }
        let msg: String = out.stderr.trim().lines().take(4).collect::<Vec<_>>().join("\n");
        return Err(format!("search failed: {msg}"));
    }
    let re = lazy_re!(r"^(.+?):(\d+):(.*)$");
    let mut hits = Vec::new();
    for l in out.stdout.lines() {
        let Some(c) = re.captures(l) else { continue };
        let file = c[1].replace('\\', "/");
        hits.push((
            file.trim_start_matches("./").to_string(),
            c[2].parse().unwrap_or(0),
            c[3].to_string(),
        ));
    }
    Ok(RgOutcome::Hits(hits))
}

/// Translate an rg-style glob (`*`, `**`, `?`, `{a,b}`) into an anchored regex.
pub(crate) fn glob_to_regex(g: &str) -> Result<Regex, String> {
    let g = clean_filter(g);
    let cs: Vec<char> = g.chars().collect();
    let mut re = String::from("^");
    let (mut i, mut in_brace) = (0usize, false);
    while i < cs.len() {
        match cs[i] {
            '*' if cs.get(i + 1) == Some(&'*') => {
                i += 1;
                if cs.get(i + 1) == Some(&'/') {
                    i += 1;
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '{' => {
                in_brace = true;
                re.push_str("(?:");
            }
            '}' if in_brace => {
                in_brace = false;
                re.push(')');
            }
            ',' if in_brace => re.push('|'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&re).map_err(|e| format!("invalid glob `{g}`: {e}"))
}

pub(crate) struct GlobMatcher {
    pos: Vec<(Regex, bool)>, // (regex, has_slash)
    neg: Vec<(Regex, bool)>,
}

impl GlobMatcher {
    pub(crate) fn compile(globs: &[String]) -> Result<Self, String> {
        let mut pos = Vec::new();
        let mut neg = Vec::new();
        for g in globs {
            let g = clean_filter(g);
            if g.is_empty() {
                continue;
            }
            if let Some(rest) = g.strip_prefix('!') {
                neg.push((glob_to_regex(rest)?, rest.contains('/')));
            } else {
                pos.push((glob_to_regex(&g)?, g.contains('/')));
            }
        }
        Ok(Self { pos, neg })
    }

    pub(crate) fn matches(&self, rel: &str, fname: &str) -> bool {
        let hit = |(re, has_slash): &(Regex, bool)| -> bool {
            if *has_slash {
                re.is_match(rel)
            } else {
                re.is_match(rel) || re.is_match(fname)
            }
        };
        if !self.pos.is_empty() && !self.pos.iter().any(hit) {
            return false;
        }
        if self.neg.iter().any(hit) {
            return false;
        }
        true
    }
}

/// Collect workspace files matching `targets` + `globs` (used by `builtin_search` and `replace_text`).
pub(crate) fn collect_files(
    ws: &Path,
    targets: &[String],
    globs: &[String],
) -> Result<Vec<(PathBuf, String)>, String> {
    let matcher = GlobMatcher::compile(globs)?;
    let roots: Vec<PathBuf> = if targets.is_empty() {
        vec![ws.to_path_buf()]
    } else {
        targets.iter().map(|t| if t == "." { ws.to_path_buf() } else { ws.join(t) }).collect()
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if root.is_file() {
            let rel = rel_of(ws, &root);
            let fname = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if matcher.matches(&rel, &fname) && seen.insert(rel.clone()) {
                out.push((root, rel));
            }
            continue;
        }
        let walk = WalkDir::new(&root).sort_by_file_name().into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name.starts_with('.') || (e.file_type().is_dir() && ignored_dir(&name)))
        });
        for e in walk.flatten() {
            if !e.file_type().is_file() {
                continue;
            }
            let rel = rel_of(ws, e.path());
            let fname = e.file_name().to_string_lossy();
            if !matcher.matches(&rel, &fname) {
                continue;
            }
            if seen.insert(rel.clone()) {
                out.push((e.path().to_path_buf(), rel));
            }
        }
    }
    Ok(out)
}

/// Dependency-free fallback for machines without ripgrep. Returns `(hits, files_searched)`.
pub(crate) fn builtin_search(
    ws: &Path,
    query: &str,
    targets: &[String],
    globs: &[String],
    opts: &SearchOpts,
) -> Result<(Vec<Hit>, usize), String> {
    let pat = if opts.literal { regex::escape(query) } else { query.to_string() };
    let smart = opts.ignore_case || !query.chars().any(|c| c.is_uppercase());
    let re = regex::RegexBuilder::new(&pat)
        .case_insensitive(smart)
        .build()
        .map_err(|e| format!("search failed: invalid regex: {e}"))?;
    let files = collect_files(ws, targets, globs)?;
    let max_per_file = if opts.files_only { 1000 } else if opts.limit > 80 { 50 } else { 20 };
    let max_total = (opts.limit * 5).max(400);
    let mut hits: Vec<Hit> = Vec::new();
    let mut searched = 0usize;
    for (full, rel) in files {
        if full.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&full) else { continue };
        searched += 1;
        let mut per_file = 0;
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                hits.push((rel.clone(), i + 1, l.to_string()));
                per_file += 1;
                if per_file >= max_per_file {
                    break;
                }
            }
        }
        if hits.len() >= max_total {
            break;
        }
    }
    Ok((hits, searched))
}

/// Relax rooted positive globs (`src/**` -> `**/src/**`) when a monorepo prefix was omitted.
fn relax_globs(globs: &[String]) -> Option<Vec<String>> {
    let mut changed = false;
    let relaxed: Vec<String> = globs
        .iter()
        .map(|g| {
            if !g.starts_with('!') && !g.starts_with("**/") && g.contains('/') {
                changed = true;
                format!("**/{g}")
            } else {
                g.clone()
            }
        })
        .collect();
    changed.then_some(relaxed)
}

fn run_search_pass(
    ws: &Path,
    query: &str,
    targets: &[String],
    globs: &[String],
    opts: &SearchOpts,
) -> Result<Option<Vec<Hit>>, String> {
    match rg_search(ws, query, targets, globs, opts)? {
        RgOutcome::Hits(h) => Ok(Some(h)),
        RgOutcome::NoFilesSearched => Ok(None),
        RgOutcome::Unavailable => {
            let (h, searched) = builtin_search(ws, query, targets, globs, opts)?;
            if searched == 0 && !globs.is_empty() {
                Ok(None)
            } else {
                Ok(Some(h))
            }
        }
    }
}

/// Search; when a regex finds nothing (or doesn't compile) but the query holds regex
/// metacharacters, retry once as a literal string and say so.
pub fn search(ws: &Path, query: &str, opts: &SearchOpts) -> Result<String, String> {
    let first = search_once(ws, query, opts);
    let retry = !opts.literal
        && query.chars().any(|c| "()[]{}.*+?^$|\\".contains(c))
        && match &first {
            Ok(t) => t.trim_end().ends_with("No matches found."),
            Err(e) => e.contains("regex"),
        };
    if !retry {
        return first;
    }
    let lit = SearchOpts { literal: true, ..opts.clone() };
    match search_once(ws, query, &lit) {
        Ok(t) if !t.trim_end().ends_with("No matches found.") => {
            Ok(format!("(note: no regex matches; showing literal matches for `{query}`)\n{t}"))
        }
        _ => first,
    }
}

/// Lockfiles and minified/generated bundles: huge, repetitive, and almost never what a code
/// search is after, so their hits are listed last instead of crowding out real source.
fn is_low_signal(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    name.ends_with(".lock")
        || name.ends_with(".lockb")
        || name.ends_with("-lock.json")
        || name.ends_with("-lock.yaml")
        || name == "go.sum"
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".map")
}
fn search_once(ws: &Path, query: &str, opts: &SearchOpts) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("empty query".into());
    }
    let mut scope = resolve_scope(ws, opts.path.as_deref(), &opts.globs, opts.cwd.as_deref())?;
    let mut hits = match run_search_pass(ws, query, &scope.targets, &scope.globs, opts)? {
        Some(h) => h,
        None => {
            // No files matched the glob at repo root: auto-retry with `**/` prefix!
            if let Some(relaxed) = relax_globs(&scope.globs) {
                match run_search_pass(ws, query, &scope.targets, &relaxed, opts)? {
                    Some(h) => {
                        scope.notes.push(format!(
                            "(note: no files matched `{}` at repo root; searched `{}` instead)",
                            scope.globs.join(", "),
                            relaxed.join(", ")
                        ));
                        h
                    }
                    None => {
                        let tops = top_level_dirs(ws);
                        let tip = if tops.is_empty() {
                            String::new()
                        } else {
                            format!(" (workspace root dirs: {})", tops.join(", "))
                        };
                        return Err(format!(
                            "no files matched glob `{}`{tip}",
                            scope.globs.join(", ")
                        ));
                    }
                }
            } else {
                let tops = top_level_dirs(ws);
                let tip = if tops.is_empty() {
                    String::new()
                } else {
                    format!(" (workspace root dirs: {})", tops.join(", "))
                };
                return Err(format!(
                    "no files matched filter `{}`{tip}",
                    scope.globs.join(", ")
                ));
            }
        }
    };
    // Asking for a lockfile/bundle by name or glob opts out of the demotion. Stable sort keeps the
    // original order within each group.
    let asked_for_noise = scope.targets.iter().chain(scope.globs.iter()).any(|t| {
        let t = t.to_lowercase();
        t.contains("lock") || t.contains(".min.") || t.contains(".map") || t.contains("go.sum")
    });
    if !asked_for_noise {
        hits.sort_by_key(|h| is_low_signal(&h.0));
    }

    let mut res = String::new();
    for n in &scope.notes {
        res.push_str(n);
        res.push('\n');
    }
    if hits.is_empty() {
        res.push_str("No matches found.");
        return Ok(res);
    }

    let limit = opts.limit.clamp(1, 500);

    if opts.files_only {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for (file, _, _) in &hits {
            *counts.entry(file.as_str()).or_default() += 1;
        }
        let mut pairs: Vec<(&str, usize)> = counts.into_iter().collect();
        pairs.sort_by(|a, b| {
            let noise = |f: &str| !asked_for_noise && is_low_signal(f);
            noise(a.0).cmp(&noise(b.0)).then_with(|| b.1.cmp(&a.1)).then_with(|| a.0.cmp(b.0))
        });
        for (i, (f, n)) in pairs.iter().enumerate() {
            if i >= limit {
                res.push_str(&format!("…[{} more files]\n", pairs.len() - i));
                break;
            }
            res.push_str(&format!("{f} ({n} hit{})\n", if *n == 1 { "" } else { "s" }));
        }
        return Ok(res);
    }

    // Narrow searches get ±2 anchored lines by default so the agent can edit straight from the
    // result without a separate `view`.
    let context = if opts.context == 0 && hits.len() <= 12 { 2 } else { opts.context };
    if context > 0 {
        let ctx = context.min(10);
        // Group hit line numbers per file, preserving discovery order.
        let mut by_file: Vec<(String, Vec<usize>)> = Vec::new();
        for (file, line, _) in &hits {
            if let Some(entry) = by_file.iter_mut().find(|(f, _)| f == file) {
                entry.1.push(*line);
            } else {
                by_file.push((file.clone(), vec![*line]));
            }
        }
        let mut shown_lines = 0usize;
        let max_lines = (limit * (2 * ctx + 1)).min(400);
        for (file, mut hit_lines) in by_file {
            hit_lines.sort_unstable();
            hit_lines.dedup();
            let Ok(text) = std::fs::read_to_string(ws.join(&file)) else { continue };
            let flines: Vec<&str> = text.lines().collect();
            let n = flines.len();
            if n == 0 {
                continue;
            }
            // Merge overlapping [l - ctx ..= l + ctx] windows.
            let mut windows: Vec<(usize, usize)> = Vec::new();
            for &ln in &hit_lines {
                let a = ln.saturating_sub(ctx).max(1);
                let b = (ln + ctx).min(n);
                if let Some(last) = windows.last_mut() {
                    if a <= last.1 + 1 {
                        last.1 = last.1.max(b);
                        continue;
                    }
                }
                windows.push((a, b));
            }
            res.push_str(&format!("# {file}\n"));
            for (wi, (a, b)) in windows.iter().enumerate() {
                if wi > 0 {
                    res.push_str("--\n");
                }
                for ln in *a..=*b {
                    if shown_lines >= max_lines || res.len() > 16_000 {
                        res.push_str("…[more matches; narrow query, path, or --glob]\n");
                        return Ok(res);
                    }
                    let raw = flines[ln - 1];
                    let sep = if hit_lines.binary_search(&ln).is_ok() { "|" } else { "-" };
                    let text: String = raw.chars().take(200).collect();
                    res.push_str(&format!("{ln}#{}{sep}{text}\n", line_hash(raw)));
                    shown_lines += 1;
                }
            }
        }
        return Ok(res);
    }

    for (count, (file, line, raw)) in hits.iter().enumerate() {
        if count >= limit || res.len() > 12_000 {
            res.push_str(&format!(
                "…[{} more matches; narrow the query, path, --glob, or raise --limit]\n",
                hits.len() - count
            ));
            break;
        }
        let text: String = raw.chars().take(200).collect();
        res.push_str(&format!("{file}:{line}#{}| {}\n", line_hash(raw), text.trim()));
    }
    Ok(res)
}
