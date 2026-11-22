//! Atomic multi-file / multi-hunk V4A patch engine with hashline-anchored and classic context hunks.
//!
//! Envelope (OpenAI V4A format, extended with hashline-anchored hunks):
//!
//!   *** Begin Patch            (optional if patch starts directly with `*** Update/Add/Delete File:`)
//!   *** Update File: src/a.rs
//!   @@ 12#ab..15#cd            replace lines 12..15 with the `+` lines (no old text needed)
//!   +new line
//!   @@ after 40#ef             insert `+` lines after line 40 (also: `before 40#ef`)
//!   +inserted
//!   @@ delete 50#aa..52#bb     remove lines 50..52
//!   @@ fn helper() {           classic V4A context hunk: ' ' context, '-' remove, '+' add
//!    context line
//!   -old line
//!   +new line
//!   *** Add File: src/new.rs
//!   +content
//!   *** Delete File: src/old.rs
//!   *** End Patch

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::doc::{line_hash, locate, parse_anchor_in_doc, read_doc, splice, Doc};
use super::patch_parse::{parse_patch, AnchorKind, FileOp, Hunk};
use super::symbols::{lang_for, syntax_errors};
use super::util::{missing_path_err, rel_of, resolve};

/// Fold typographic punctuation to ASCII (what models often type instead of the file's text).
fn fold_punct(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// Last-resort line equality: folded punctuation, and a `?` in the pattern may stand for one
/// non-ASCII character (Windows PowerShell 5.1 pipes text to native programs as ASCII).
fn fuzzy_eq(file: &str, pat: &str, wild: &mut bool) -> bool {
    if fold_punct(file) == fold_punct(pat) {
        return true;
    }
    let (f, p) = (file.trim(), pat.trim());
    if !p.contains('?') {
        return false;
    }
    let (fc, pc): (Vec<char>, Vec<char>) = (f.chars().collect(), p.chars().collect());
    if fc.len() != pc.len() {
        return false;
    }
    let ok = fc.iter().zip(&pc).all(|(a, b)| a == b || (*b == '?' && !a.is_ascii()));
    if ok {
        *wild = true;
    }
    ok
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum MatchLevel {
    Exact,
    Rstrip,
    Trim,
    Fuzzy,
}

/// Find `pat` in `lines` at or after `from`. Exact/rstrip take the first hit (V4A semantics);
/// the looser passes must be unique so they can never pick the wrong place.
fn seek_lines(
    lines: &[String],
    pat: &[String],
    from: usize,
    eof: bool,
) -> Result<Option<(usize, MatchLevel, bool)>, String> {
    if pat.is_empty() || pat.len() > lines.len() {
        return Ok(None);
    }
    let last = lines.len() - pat.len();
    let levels = [MatchLevel::Exact, MatchLevel::Rstrip, MatchLevel::Trim, MatchLevel::Fuzzy];
    for level in levels {
        let at = |i: usize| -> Option<bool> {
            let mut wild = false;
            pat.iter()
                .enumerate()
                .all(|(k, p)| {
                    let a = &lines[i + k];
                    match level {
                        MatchLevel::Exact => a == p,
                        MatchLevel::Rstrip => a.trim_end() == p.trim_end(),
                        MatchLevel::Trim => a.trim() == p.trim(),
                        MatchLevel::Fuzzy => fuzzy_eq(a, p, &mut wild),
                    }
                })
                .then_some(wild)
        };
        if eof && last >= from {
            if let Some(w) = at(last) {
                return Ok(Some((last, level, w)));
            }
        }
        let mut hits = Vec::new();
        for i in from..=last {
            if let Some(w) = at(i) {
                hits.push((i, w));
                if matches!(level, MatchLevel::Exact | MatchLevel::Rstrip) {
                    return Ok(Some((i, level, w)));
                }
            }
        }
        if hits.len() == 1 {
            return Ok(Some((hits[0].0, level, hits[0].1)));
        }
        if hits.len() > 1 {
            let where_s: Vec<String> = hits.iter().take(4).map(|(i, _)| (i + 1).to_string()).collect();
            return Err(format!(
                "matches {} places (lines {}); add a surrounding context line or use `@@ N#hh..M#hh` anchors",
                hits.len(),
                where_s.join(", ")
            ));
        }
    }
    Ok(None)
}

fn leading_ws(s: &str) -> &str {
    let n = s.len() - s.trim_start_matches([' ', '\t']).len();
    &s[..n]
}

enum Shift {
    Add(String),
    Drop(String),
    Replace(String, String),
}

fn indent_shift(old_first: &str, file_first: &str) -> Option<Shift> {
    let (a, b) = (leading_ws(old_first), leading_ws(file_first));
    if a == b {
        return None;
    }
    if let Some(extra) = b.strip_prefix(a) {
        Some(Shift::Add(extra.to_string()))
    } else if let Some(drop) = a.strip_prefix(b) {
        Some(Shift::Drop(drop.to_string()))
    } else {
        Some(Shift::Replace(a.to_string(), b.to_string()))
    }
}

fn apply_shift(l: &str, s: &Shift) -> String {
    if l.trim().is_empty() {
        return l.to_string();
    }
    match s {
        Shift::Add(e) => format!("{e}{l}"),
        Shift::Drop(d) => l.strip_prefix(d.as_str()).unwrap_or(l).to_string(),
        Shift::Replace(a, b) => l.strip_prefix(a.as_str()).map(|r| format!("{b}{r}")).unwrap_or_else(|| l.to_string()),
    }
}

fn closest_hint(doc: &Doc, old: &[String]) -> String {
    let wanted: Vec<&str> = old.iter().map(|l| l.trim()).filter(|l| l.len() >= 4).collect();
    if wanted.is_empty() || doc.lines.is_empty() {
        return String::new();
    }
    let (mut best_i, mut best_score) = (0usize, 0usize);
    let win = old.len().max(1);
    for i in 0..doc.lines.len() {
        let end = (i + win).min(doc.lines.len());
        let score = wanted
            .iter()
            .filter(|w| doc.lines[i..end].iter().any(|dl| dl.trim() == **w || dl.contains(**w)))
            .count();
        if score > best_score {
            best_score = score;
            best_i = i;
        }
    }
    if best_score == 0 {
        return String::new();
    }
    let from = best_i.saturating_sub(1);
    let to = (best_i + win.min(6) + 1).min(doc.lines.len() - 1);
    format!("\n  Closest lines near {}:\n{}", best_i + 1, doc.hashlines(from, to).trim_end())
}

struct PlannedFile {
    full: PathBuf,
    rel: String,
    original: Option<Vec<u8>>,
    new_text: Option<String>,
    remove_after: Option<PathBuf>,
    summary: String,
    notes: Vec<String>,
}

fn patch_path(ws: &Path, cwd: Option<&Path>, p: &str, must_exist: bool) -> Result<PathBuf, String> {
    let p = p.trim();
    if p.is_empty() {
        return Err("empty file path".into());
    }
    let from_ws = resolve(ws, p);
    let from_cwd = cwd.and_then(|c| {
        if Path::new(p).is_absolute() {
            None
        } else {
            resolve(ws, &c.join(p).to_string_lossy()).ok()
        }
    });
    if must_exist {
        if let Some(c) = from_cwd.filter(|f| f.exists()) {
            return Ok(c);
        }
        return from_ws;
    }
    match (from_ws, from_cwd) {
        (Ok(w), _) => Ok(w),
        (Err(_), Some(c)) => Ok(c),
        (Err(e), None) => Err(e),
    }
}

fn plan_update(
    ws: &Path,
    cwd: Option<&Path>,
    path: &str,
    move_to: Option<&str>,
    hunks: &[Hunk],
    errors: &mut Vec<String>,
) -> Option<PlannedFile> {
    let full = match patch_path(ws, cwd, path, true) {
        Ok(f) => f,
        Err(e) => {
            errors.push(format!("{path}: {e}"));
            return None;
        }
    };
    let (full, rel, doc) = match read_doc(ws, &full.to_string_lossy()) {
        Ok(x) => x,
        Err(e) => {
            errors.push(format!("{path}: {e}"));
            return None;
        }
    };
    let lang = lang_for(&rel);
    let before_errs = syntax_errors(lang, &doc.to_text());
    let mut edits: Vec<(usize, usize, Vec<String>, usize)> = Vec::new();
    let mut notes = Vec::new();
    let mut cursor = 0usize;
    let mut failed = false;
    for (hi, h) in hunks.iter().enumerate() {
        let tag = |header: &str| format!("{rel} hunk {} (`{header}`)", hi + 1);
        match h {
            Hunk::BadAnchor { raw, header } => {
                // Strip optional leading keyword and diagnose each anchor piece against `doc`.
                let rest = raw
                    .trim()
                    .strip_prefix("after")
                    .or_else(|| raw.trim().strip_prefix("before"))
                    .or_else(|| raw.trim().strip_prefix("replace"))
                    .or_else(|| raw.trim().strip_prefix("delete"))
                    .unwrap_or(raw)
                    .trim();
                let mut piece_errs = Vec::new();
                for part in rest.split("..") {
                    if let Err(e) = parse_anchor_in_doc(&doc, &rel, part.trim()) {
                        piece_errs.push(e);
                    }
                }
                if piece_errs.is_empty() {
                    errors.push(format!(
                        "{}: malformed anchored header (expected e.g. `@@ 12#ab..15#cd` or `@@ after 12#ab`)",
                        tag(header)
                    ));
                } else {
                    errors.push(format!("{}: {}", tag(header), piece_errs.join("\n")));
                }
                failed = true;
            }
            Hunk::Anchored { kind, start, end, new, header } => {
                let (r0, r1) = match locate(&doc, &rel, start, end) {
                    Ok(r) => r,
                    Err(e) => {
                        errors.push(format!("{}: {e}", tag(header)));
                        failed = true;
                        continue;
                    }
                };
                match kind {
                    AnchorKind::Replace if new.is_empty() => {
                        errors.push(format!(
                            "{}: no `+` lines. To remove lines say so: `@@ delete {}#{}..{}#{}`",
                            tag(header),
                            start.0,
                            start.1,
                            end.0,
                            end.1
                        ));
                        failed = true;
                    }
                    AnchorKind::Replace => edits.push((r0, r1 - r0 + 1, new.clone(), hi)),
                    AnchorKind::Delete => edits.push((r0, r1 - r0 + 1, Vec::new(), hi)),
                    AnchorKind::After | AnchorKind::Before if new.is_empty() => {
                        errors.push(format!("{}: nothing to insert (no `+` lines)", tag(header)));
                        failed = true;
                    }
                    AnchorKind::After => edits.push((r1 + 1, 0, new.clone(), hi)),
                    AnchorKind::Before => edits.push((r0, 0, new.clone(), hi)),
                }
            }
            Hunk::Context { ctx, body, eof, header } => {
                let old: Vec<String> = body.iter().filter(|(k, _)| *k != b'+').map(|(_, l)| l.clone()).collect();
                let mut hunk_cursor = cursor;
                if let Some(c) = ctx {
                    let pat = vec![c.clone()];
                    match seek_lines(&doc.lines, &pat, hunk_cursor, false) {
                        Ok(Some((idx, _, _))) => {
                            hunk_cursor = if old.first().map_or(false, |o| o.trim() == c.trim()) {
                                idx
                            } else {
                                idx + 1
                            };
                        }
                        Ok(None) => {
                            if !old.iter().any(|o| o.trim() == c.trim()) {
                                errors.push(format!(
                                    "{}: the `@@` context line was not found{}",
                                    tag(header),
                                    closest_hint(&doc, &pat)
                                ));
                                failed = true;
                                continue;
                            }
                        }
                        Err(e) => {
                            errors.push(format!("{}: {e}", tag(header)));
                            failed = true;
                            continue;
                        }
                    }
                }
                if old.is_empty() {
                    let at = if ctx.is_some() && hunk_cursor > 0 { hunk_cursor } else { doc.lines.len() };
                    edits.push((
                        at,
                        0,
                        body.iter().filter(|(k, _)| *k == b'+').map(|(_, l)| l.clone()).collect(),
                        hi,
                    ));
                    continue;
                }
                let mut pat_len = old.len();
                let mut found = seek_lines(&doc.lines, &old[..pat_len], hunk_cursor, *eof);
                let mut dropped_trailing_blank = false;
                if matches!(found, Ok(None)) && old.last().map_or(false, |l| l.trim().is_empty()) && pat_len > 1 {
                    pat_len -= 1;
                    dropped_trailing_blank = true;
                    found = seek_lines(&doc.lines, &old[..pat_len], hunk_cursor, *eof);
                }
                if matches!(found, Ok(None)) && ctx.is_none() && hunk_cursor > 0 {
                    found = seek_lines(&doc.lines, &old[..pat_len], 0, *eof);
                }
                match found {
                    Ok(Some((idx, level, wild))) => {
                        let pat = &old[..pat_len];
                        let shift = if matches!(level, MatchLevel::Trim | MatchLevel::Fuzzy) {
                            pat.iter()
                                .enumerate()
                                .find(|(_, l)| !l.trim().is_empty())
                                .and_then(|(k, l)| indent_shift(l, &doc.lines[idx + k]))
                        } else {
                            None
                        };
                        let mut new_lines: Vec<String> = Vec::new();
                        let mut oi = 0usize;
                        let mut reindented = false;
                        for (k, l) in body {
                            match *k {
                                b' ' => {
                                    if oi < pat_len {
                                        new_lines.push(doc.lines[idx + oi].clone());
                                        oi += 1;
                                    } else if !dropped_trailing_blank {
                                        new_lines.push(l.clone());
                                    }
                                }
                                b'-' => {
                                    oi += 1;
                                }
                                b'+' => {
                                    if let Some(ref sh) = shift {
                                        let shifted = apply_shift(l, sh);
                                        if shifted != *l {
                                            reindented = true;
                                        }
                                        new_lines.push(shifted);
                                    } else {
                                        new_lines.push(l.clone());
                                    }
                                }
                                _ => {}
                            }
                        }
                        if reindented {
                            notes.push(format!(
                                "{rel} hunk {} matched after ignoring indentation; re-indented its `+` lines",
                                hi + 1
                            ));
                        }
                        if wild {
                            notes.push(format!(
                                "{rel} hunk {}: `?` stood in for non-ASCII characters - your shell re-encoded the patch as ASCII. Check the result, or pass the patch with --file / vx mcp",
                                hi + 1
                            ));
                        }
                        edits.push((idx, pat_len, new_lines, hi));
                        cursor = idx + pat_len;
                    }
                    Ok(None) => {
                        let shown: Vec<String> = old.iter().take(4).map(|l| format!("\n    -{l}")).collect();
                        errors.push(format!(
                            "{}: context not found{}. Expected:{}{}",
                            tag(header),
                            if cursor > 0 { format!(" after line {cursor}") } else { String::new() },
                            shown.concat(),
                            closest_hint(&doc, &old)
                        ));
                        failed = true;
                    }
                    Err(e) => {
                        errors.push(format!("{}: {e}", tag(header)));
                        failed = true;
                    }
                }
            }
        }
    }
    if failed {
        return None;
    }
    edits.sort_by_key(|e| (e.0, e.3));
    for w in edits.windows(2) {
        if w[0].0 + w[0].1 > w[1].0 {
            errors.push(format!(
                "{rel}: hunks {} and {} touch the same lines; merge them into one hunk",
                w[0].3 + 1,
                w[1].3 + 1
            ));
            return None;
        }
    }
    let mut new_doc = Doc { lines: doc.lines.clone(), ends: doc.ends.clone(), eol: doc.eol, bom: doc.bom };
    for (start, remove, new, _) in edits.iter().rev() {
        splice(&mut new_doc, *start, *remove, new.clone());
    }
    let (mut delta, mut ranges, mut plus, mut minus) = (0isize, Vec::new(), 0usize, 0usize);
    for (start, remove, new, _) in &edits {
        let s = (*start as isize + delta) as usize;
        if !new.is_empty() {
            ranges.push(format!("{}..{}", new_doc.anchor(s), new_doc.anchor(s + new.len() - 1)));
        }
        delta += new.len() as isize - *remove as isize;
        plus += new.len();
        minus += remove;
    }
    let text = new_doc.to_text();
    let mut target_full = full.clone();
    let mut target_rel = rel.clone();
    let mut remove_after = None;
    if let Some(m) = move_to {
        match patch_path(ws, cwd, m, false) {
            Ok(t) if t.exists() => {
                errors.push(format!("{rel}: cannot move to {m}: it already exists"));
                return None;
            }
            Ok(t) => {
                target_rel = rel_of(ws, &t);
                target_full = t;
                remove_after = Some(full.clone());
            }
            Err(e) => {
                errors.push(format!("{rel}: cannot move to {m}: {e}"));
                return None;
            }
        }
    }
    if let (Some(before), Some(after)) = (before_errs, syntax_errors(lang_for(&target_rel), &text)) {
        if after.len() > before.len() {
            if let Some((row, m)) = after
                .iter()
                .find(|(r, _)| !before.iter().any(|(b, _)| b == r))
                .or_else(|| after.first())
            {
                let row = (*row).min(new_doc.lines.len().saturating_sub(1));
                notes.push(format!("⚠ {target_rel}: new syntax error at {}: {m}", new_doc.anchor(row)));
            }
        }
    }
    let moved = if remove_after.is_some() { format!(" -> {target_rel}") } else { String::new() };
    let now = if ranges.is_empty() { String::new() } else { format!("  now {}", ranges.join(", ")) };
    let summary = if edits.is_empty() {
        format!("R {rel}{moved}")
    } else {
        format!("M {rel}{moved}  {} hunk(s) +{plus} -{minus}{now}", edits.len())
    };
    Some(PlannedFile {
        original: std::fs::read(&full).ok(),
        full: target_full,
        rel: target_rel,
        new_text: Some(text),
        remove_after,
        summary,
        notes,
    })
}

/// Apply a V4A patch (with hashline-anchored hunks) atomically.
pub fn apply_patch(
    ws: &Path,
    patch: &str,
    cwd: Option<&Path>,
    dry_run: bool,
) -> Result<String, String> {
    let ops = parse_patch(patch)?;
    let mut errors: Vec<String> = Vec::new();
    let mut plans: Vec<PlannedFile> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for op in &ops {
        let key = match op {
            FileOp::Add { path, .. } | FileOp::Delete { path } | FileOp::Update { path, .. } => {
                path.replace('\\', "/").to_lowercase()
            }
        };
        if !seen.insert(key) {
            errors.push(format!(
                "{}: appears twice in the patch; put all its hunks under one `*** Update File:`",
                match op {
                    FileOp::Add { path, .. } | FileOp::Delete { path } | FileOp::Update { path, .. } => path,
                }
            ));
            continue;
        }
        match op {
            FileOp::Add { path, lines } => match patch_path(ws, cwd, path, false) {
                Ok(full) if full.exists() => errors.push(format!(
                    "{path}: already exists; use `*** Update File:` (or delete it first)"
                )),
                Ok(full) => {
                    let rel = rel_of(ws, &full);
                    let mut text = lines.join("\n");
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    let mut notes = Vec::new();
                    if let Some(errs) = syntax_errors(lang_for(&rel), &text) {
                        if let Some((row, m)) = errs.first() {
                            let line = text.lines().nth(*row).unwrap_or("");
                            notes.push(format!(
                                "⚠ {rel}: syntax error at {}#{}: {m}",
                                row + 1,
                                line_hash(line)
                            ));
                        }
                    }
                    plans.push(PlannedFile {
                        full,
                        summary: format!("A {rel}  ({} lines)", lines.len()),
                        rel,
                        original: None,
                        new_text: Some(text),
                        remove_after: None,
                        notes,
                    });
                }
                Err(e) => errors.push(format!("{path}: {e}")),
            },
            FileOp::Delete { path } => match patch_path(ws, cwd, path, true) {
                Ok(full) if full.is_file() => {
                    let rel = rel_of(ws, &full);
                    plans.push(PlannedFile {
                        original: std::fs::read(&full).ok(),
                        full,
                        summary: format!("D {rel}"),
                        rel,
                        new_text: None,
                        remove_after: None,
                        notes: Vec::new(),
                    });
                }
                Ok(full) => errors.push(missing_path_err(ws, &rel_of(ws, &full), true, false)),
                Err(e) => errors.push(format!("{path}: {e}")),
            },
            FileOp::Update { path, move_to, hunks } => {
                if let Some(p) = plan_update(ws, cwd, path, move_to.as_deref(), hunks, &mut errors) {
                    plans.push(p);
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "patch not applied - nothing was written ({} problem(s)):\n{}\nFix those hunks and send the whole patch again.",
            errors.len(),
            errors.iter().map(|e| format!("✖ {e}")).collect::<Vec<_>>().join("\n")
        ));
    }
    if !dry_run {
        let mut done: Vec<&PlannedFile> = Vec::new();
        for p in &plans {
            let res = match &p.new_text {
                Some(t) => {
                    if let Some(parent) = p.full.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::write(&p.full, t.as_bytes()).and_then(|_| match &p.remove_after {
                        Some(old) => std::fs::remove_file(old),
                        None => Ok(()),
                    })
                }
                None => std::fs::remove_file(&p.full),
            };
            if let Err(e) = res {
                // Restore the failing file if a move partially wrote the destination.
                if let Some(ref old_path) = p.remove_after {
                    let _ = std::fs::remove_file(&p.full);
                    if let Some(ref orig) = p.original {
                        let _ = std::fs::write(old_path, orig);
                    }
                }
                for prev in done.iter().rev() {
                    if let Some(ref old_path) = prev.remove_after {
                        let _ = std::fs::remove_file(&prev.full);
                        if let Some(ref orig) = prev.original {
                            let _ = std::fs::write(old_path, orig);
                        }
                    } else if let Some(ref orig) = prev.original {
                        let _ = std::fs::write(&prev.full, orig);
                    } else {
                        let _ = std::fs::remove_file(&prev.full);
                    }
                }
                return Err(format!("failed writing {} ({e}); rolled back all changes", p.rel));
            }
            done.push(p);
        }
    }
    let mut out = if dry_run {
        format!("Dry run OK: {} file(s) would change (nothing written):\n", plans.len())
    } else {
        format!("Applied patch: {} file(s):\n", plans.len())
    };
    for p in &plans {
        out.push_str(&p.summary);
        out.push('\n');
    }
    for p in &plans {
        for n in &p.notes {
            out.push_str(n);
            out.push('\n');
        }
    }
    Ok(out)
}
