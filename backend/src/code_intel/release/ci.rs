//! Inspection of tag-triggered GitHub Actions workflows (`.github/workflows/*.{yml,yaml}`).
//!
//! Used in `ci` release mode to verify that the workflow actually reads the release notes file
//! and matches the tag being released, rather than silently dropping written notes on tag push.

use regex::Regex;
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct CiWorkflow {
    pub file: String,
    pub tag_patterns: Vec<String>,
    /// Repo-relative notes directory read by the workflow (e.g. `.github/release-notes`).
    pub notes_dir: Option<String>,
    /// True when the workflow reads a repository notes file or `CHANGELOG.md`.
    pub reads_notes: bool,
    /// Human-readable description of the notes source (e.g. `.github/release-notes/<tag>.md` or `CHANGELOG.md`).
    pub notes_source: Option<String>,
    /// True when the workflow appears to create or update a GitHub Release.
    pub creates_release: bool,
    /// True when the workflow asks GitHub to auto-generate release notes.
    pub auto_notes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesWiring {
    /// No tag-triggered CI workflows exist.
    NoCi,
    /// A workflow reads the exact notes directory or file we write to.
    Aligned { workflow: String, source: String },
    /// A workflow reads a different notes path than `notes_rel`.
    Mismatch { workflow: String, expected: String, actual: String },
    /// A workflow reads `CHANGELOG.md` rather than a per-tag notes file.
    Changelog { workflow: String },
    /// None of the tag-triggered workflows read a release notes file or changelog.
    Missing { workflows: Vec<String>, auto_notes: bool },
}

/// Strip YAML `# ...` comments while leaving `#` inside single/double quotes intact.
fn strip_yaml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.replace("\r\n", "\n").lines() {
        let mut in_single = false;
        let mut in_double = false;
        let mut cut = line.len();
        let bytes = line.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single && (i == 0 || bytes[i - 1] != b'\\') => in_double = !in_double,
                b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                    cut = i;
                    break;
                }
                _ => {}
            }
        }
        out.push_str(line[..cut].trim_end());
        out.push('\n');
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|b| *b == b' ' || *b == b'\t').count()
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\''))) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Extract tag filter patterns under `on.push.tags`. Returns `None` if the workflow does not
/// trigger on `push.tags`.
pub fn extract_push_tag_patterns(clean_yaml: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = clean_yaml.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if indent_of(lines[i]) == 0 && (trimmed == "on:" || trimmed.starts_with("on:")) {
            let on_rest = trimmed.strip_prefix("on:").unwrap_or("").trim();
            if on_rest.contains("push:") && on_rest.contains("tags:") {
                return Some(Vec::new());
            }
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                let t = l.trim();
                if t.is_empty() {
                    i += 1;
                    continue;
                }
                let ind = indent_of(l);
                if ind == 0 {
                    break;
                }
                if t == "push:" || t.starts_with("push:") {
                    let push_ind = ind;
                    let push_rest = t.strip_prefix("push:").unwrap_or("").trim();
                    if push_rest.contains("tags:") {
                        return Some(Vec::new());
                    }
                    i += 1;
                    while i < lines.len() {
                        let pl = lines[i];
                        let pt = pl.trim();
                        if pt.is_empty() {
                            i += 1;
                            continue;
                        }
                        let pind = indent_of(pl);
                        if pind <= push_ind {
                            break;
                        }
                        if pt == "tags:" || pt.starts_with("tags:") {
                            let tags_ind = pind;
                            let rest = pt.strip_prefix("tags:").unwrap_or("").trim();
                            let mut pats = Vec::new();
                            if !rest.is_empty() {
                                let inner = rest.trim_start_matches('[').trim_end_matches(']');
                                for part in inner.split(',') {
                                    let p = unquote(part);
                                    if !p.is_empty() {
                                        pats.push(p);
                                    }
                                }
                                return Some(pats);
                            }
                            i += 1;
                            while i < lines.len() {
                                let tl = lines[i];
                                let tt = tl.trim();
                                if tt.is_empty() {
                                    i += 1;
                                    continue;
                                }
                                let tind = indent_of(tl);
                                if tind < tags_ind || (tind == tags_ind && !tt.starts_with('-')) {
                                    break;
                                }
                                if let Some(item) = tt.strip_prefix('-') {
                                    let p = unquote(item);
                                    if !p.is_empty() {
                                        pats.push(p);
                                    }
                                }
                                i += 1;
                            }
                            return Some(pats);
                        }
                        i += 1;
                    }
                    continue;
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    None
}

fn is_temp_path(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    lower.contains("runner_temp")
        || lower.contains("runner.temp")
        || lower.starts_with("/tmp/")
        || lower.starts_with("$tmp")
        || lower.starts_with("${tmp")
        || lower.starts_with("artifacts/")
        || lower.starts_with("./artifacts/")
}

fn normalize_notes_dir(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    let s = s.trim_start_matches("./").replace('\\', "/");
    if s.is_empty() || is_temp_path(&s) {
        return None;
    }
    if let Some((dir, _)) = s.rsplit_once('/') {
        let dir = dir.trim_end_matches('/');
        if !dir.is_empty() && !dir.contains('$') {
            return Some(dir.to_string());
        }
    }
    None
}

pub fn analyze_workflow(file: String, raw: &str) -> Option<CiWorkflow> {
    let clean = strip_yaml_comments(raw);
    let tag_patterns = extract_push_tag_patterns(&clean)?;
    let lower = clean.to_ascii_lowercase();

    let creates_release = lower.contains("gh release create")
        || lower.contains("gh release edit")
        || lower.contains("action-gh-release")
        || lower.contains("create-release")
        || lower.contains("release-action")
        || lower.contains("tauri-action")
        || lower.contains("goreleaser")
        || lower.contains("release-please")
        || lower.contains("semantic-release")
        || lower.contains("publish-release");

    let auto_notes = lower.contains("--generate-notes")
        || lazy_re!(r"(?i)generate_release_notes\s*:\s*true").is_match(&clean);

    // 1. Explicit repo notes directories (e.g. `.github/release-notes`, `docs/release-notes`, `.vireo/release-notes`).
    let dir_re = lazy_re!(r#"(?i)(?:^|[\s"'`(=:])(\.?/?(?:\.github|\.vireo|docs)/release-notes)(?:[/\s"'`$]|$)"#);
    let mut notes_dir = dir_re
        .captures(&clean)
        .map(|c| c[1].trim_start_matches("./").to_string());

    // 2. Action inputs like `body_path:` / `releaseBodyPath:` / `release_body_path:`.
    let body_path_re = lazy_re!(r#"(?im)^\s*(?:body_path|releaseBodyPath|release_body_path)\s*:\s*([^\n]+)$"#);
    let body_path = body_path_re
        .captures(&clean)
        .map(|c| unquote(&c[1]))
        .filter(|p| !p.is_empty() && !is_temp_path(p));

    if notes_dir.is_none() {
        if let Some(bp) = &body_path {
            if bp.to_ascii_lowercase().contains("release-notes") {
                notes_dir = normalize_notes_dir(bp);
            }
        }
    }

    // 3. Direct `--notes-file <repo-path>` (ignoring temp files like `$RUNNER_TEMP/release-notes.md`).
    let notes_file_re = lazy_re!(r#"(?i)--notes-file(?:=|\s+)(?:"([^"]+)"|'([^']+)'|(\S+))"#);
    let direct_notes_file = notes_file_re
        .captures_iter(&clean)
        .filter_map(|c| c.get(1).or(c.get(2)).or(c.get(3)).map(|m| m.as_str().to_string()))
        .find(|p| !is_temp_path(p));

    if notes_dir.is_none() {
        if let Some(nf) = &direct_notes_file {
            if nf.to_ascii_lowercase().contains("release-notes") {
                notes_dir = normalize_notes_dir(nf);
            }
        }
    }

    let reads_changelog = lower.contains("changelog.md");

    let (reads_notes, notes_source) = if let Some(dir) = &notes_dir {
        (true, Some(format!("{dir}/<tag>.md")))
    } else if let Some(bp) = body_path {
        (true, Some(bp))
    } else if let Some(nf) = direct_notes_file {
        (true, Some(nf))
    } else if reads_changelog {
        (true, Some("CHANGELOG.md".to_string()))
    } else {
        (false, None)
    };

    Some(CiWorkflow {
        file,
        tag_patterns,
        notes_dir,
        reads_notes,
        notes_source,
        creates_release,
        auto_notes,
    })
}

/// Inspect every tag-triggered workflow under `.github/workflows/`.
pub fn scan_workflows(ws: &Path) -> Vec<CiWorkflow> {
    let Ok(rd) = std::fs::read_dir(ws.join(".github/workflows")) else { return Vec::new() };
    let mut out: Vec<CiWorkflow> = rd
        .flatten()
        .filter(|e| matches!(e.path().extension().and_then(|x| x.to_str()), Some("yml" | "yaml")))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let raw = std::fs::read_to_string(e.path()).ok()?;
            analyze_workflow(name, &raw)
        })
        .collect();
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// Evaluate whether `notes_rel` will actually be picked up by the repo's CI workflow(s).
pub fn evaluate_notes_wiring(workflows: &[CiWorkflow], notes_rel: &str) -> NotesWiring {
    if workflows.is_empty() {
        return NotesWiring::NoCi;
    }
    let rel = notes_rel.replace('\\', "/");
    let rel = rel.trim_start_matches("./");

    // Prefer workflows that look like release workflows, falling back to any tag-triggered workflow.
    let relevant: Vec<&CiWorkflow> = {
        let rel_wfs: Vec<&CiWorkflow> = workflows.iter().filter(|w| w.creates_release).collect();
        if rel_wfs.is_empty() { workflows.iter().collect() } else { rel_wfs }
    };

    for w in &relevant {
        if let Some(dir) = &w.notes_dir {
            let prefix = format!("{}/", dir.trim_end_matches('/'));
            if rel.starts_with(&prefix) {
                return NotesWiring::Aligned {
                    workflow: w.file.clone(),
                    source: w.notes_source.clone().unwrap_or_else(|| format!("{dir}/<tag>.md")),
                };
            }
        }
    }
    for w in &relevant {
        if let Some(dir) = &w.notes_dir {
            return NotesWiring::Mismatch {
                workflow: w.file.clone(),
                expected: format!("{dir}/<tag>.md"),
                actual: rel.to_string(),
            };
        }
    }
    for w in &relevant {
        if w.reads_notes {
            if w.notes_source.as_deref() == Some("CHANGELOG.md") {
                return NotesWiring::Changelog { workflow: w.file.clone() };
            }
            if let Some(src) = &w.notes_source {
                return NotesWiring::Aligned { workflow: w.file.clone(), source: src.clone() };
            }
        }
    }
    NotesWiring::Missing {
        workflows: relevant.iter().map(|w| w.file.clone()).collect(),
        auto_notes: relevant.iter().any(|w| w.auto_notes),
    }
}

/// Match a tag name against GitHub Actions `push.tags` glob/pattern rules (`*`, `?`, `[0-9]+`, `!exclude`).
pub fn tag_matches_patterns(tag: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let mut matched = false;
    for raw in patterns {
        let (neg, pat) = match raw.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, raw.as_str()),
        };
        if gh_pattern_matches(tag, pat) {
            matched = !neg;
        }
    }
    matched
}

fn gh_pattern_matches(text: &str, pattern: &str) -> bool {
    let mut rx = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                rx.push_str(".*");
                i += 1;
            }
            '?' => {
                rx.push('.');
                i += 1;
            }
            '[' => {
                if let Some(end) = chars[i + 1..].iter().position(|&c| c == ']') {
                    let close = i + 1 + end;
                    for &c in &chars[i..=close] {
                        rx.push(c);
                    }
                    i = close + 1;
                    // GitHub Actions tag filters support `[0-9]+` quantifier syntax.
                    if i < chars.len() && chars[i] == '+' {
                        rx.push('+');
                        i += 1;
                    }
                } else {
                    rx.push_str("\\[");
                    i += 1;
                }
            }
            c => {
                rx.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
    }
    rx.push('$');
    Regex::new(&rx).is_ok_and(|re| re.is_match(text))
}
