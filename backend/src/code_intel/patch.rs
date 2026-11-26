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

use super::doc::{fill_plain, parse_anchor_in_doc, read_doc, splice, syntax_note, Doc};
use super::journal::{self, locate_at, Splice};
use super::patch_parse::{parse_patch, AnchorKind, Edge, FileOp, Hunk};
use super::symbols::{find_symbol, import_end_row, keep_leading_docs, lang_for, symbols, syntax_errors};
use super::util::{rel_of, resolve_existing, resolve_new_file};

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
fn fuzzy_eq(file: &str, pat: &str, file_fold: &str, pat_fold: &str, wild: &mut bool) -> bool {
    if file_fold == pat_fold {
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
pub(crate) enum MatchLevel {
    Exact,
    Rstrip,
    Trim,
    Fuzzy,
}

/// Find `pat` in `lines` at or after `from`. Exact/rstrip take the first hit (V4A semantics);
/// the looser passes must be unique so they can never pick the wrong place.
pub(crate) fn seek_lines(
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
        // Fold punctuation once per line / pattern line, not once per (line, pattern) comparison.
        let (lines_fold, pat_fold): (Vec<String>, Vec<String>) = if level == MatchLevel::Fuzzy {
            (lines.iter().map(|l| fold_punct(l)).collect(), pat.iter().map(|p| fold_punct(p)).collect())
        } else {
            (Vec::new(), Vec::new())
        };
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
                        MatchLevel::Fuzzy => fuzzy_eq(a, p, &lines_fold[i + k], &pat_fold[k], &mut wild),
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

#[derive(Clone, PartialEq)]
pub(crate) enum Shift {
    Add(String),
    Drop(String),
    Replace(String, String),
    /// Only the first written line gets this indentation (the old text was copied from mid-line).
    FirstLine(String),
}

/// The indentation fix for a hunk matched while ignoring indentation, or `None` when the lines
/// already agree or the shift is not uniform. `old` is often copied from mid-line, so its first line
/// lacks the leading whitespace the rest of it has: when only that line disagrees, trust the others
/// (re-indenting every line by its offset would double the indentation).
pub(crate) fn consistent_shift(old: &[String], file: &[String]) -> Option<Shift> {
    let shifts: Vec<Option<Shift>> = old
        .iter()
        .zip(file)
        .filter(|(o, _)| !o.trim().is_empty())
        .map(|(o, f)| indent_shift(o, f))
        .collect();
    let uniform = |s: &[Option<Shift>]| s.windows(2).all(|w| w[0] == w[1]);
    match shifts.len() {
        0 => None,
        _ if uniform(&shifts) => shifts.into_iter().next().flatten(),
        n if n >= 2 && matches!(shifts[0], Some(Shift::Add(_))) && uniform(&shifts[1..]) => {
            let rest = shifts[1].clone();
            match (shifts.into_iter().next().flatten(), rest) {
                (Some(Shift::Add(first)), None) => Some(Shift::FirstLine(first)),
                (_, rest) => rest,
            }
        }
        _ => None,
    }
}

pub(crate) fn indent_shift(old_first: &str, file_first: &str) -> Option<Shift> {
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

pub(crate) fn apply_shift(l: &str, s: &Shift, first_done: &mut bool) -> String {
    if l.trim().is_empty() {
        return l.to_string();
    }
    match s {
        Shift::FirstLine(e) if !std::mem::replace(first_done, true) => format!("{e}{l}"),
        Shift::FirstLine(_) => l.to_string(),
        Shift::Add(e) => format!("{e}{l}"),
        Shift::Drop(d) => l.strip_prefix(d.as_str()).unwrap_or(l).to_string(),
        Shift::Replace(a, b) => l.strip_prefix(a.as_str()).map(|r| format!("{b}{r}")).unwrap_or_else(|| l.to_string()),
    }
}

pub(crate) fn closest_hint(doc: &Doc, old: &[String]) -> String {
    let wanted: Vec<&str> = old.iter().map(|l| l.trim()).filter(|l| l.len() >= 4).collect();
    if wanted.is_empty() || doc.lines.is_empty() {
        return String::new();
    }
    let (mut best_i, mut best_score) = (0usize, 0usize);
    let win = old.len().max(1);
    let n = doc.lines.len();
    // `score[i]` = wanted lines found in `lines[i..i+win]`. One scan per wanted line marks the
    // window starts that cover one of its hits (difference array): O(N*W) instead of O(N*W^2).
    let mut diff = vec![0i32; n + 1];
    for w in &wanted {
        let mut covered = 0usize; // window starts below this are already counted for `w`
        for (j, dl) in doc.lines.iter().enumerate() {
            if !dl.contains(*w) {
                continue;
            }
            let lo = (j + 1).saturating_sub(win).max(covered);
            if lo <= j {
                diff[lo] += 1;
                diff[j + 1] -= 1;
                covered = j + 1;
            }
        }
    }
    let mut run = 0i32;
    for i in 0..n {
        run += diff[i];
        let score = run as usize;
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
    /// Exact row changes of an update (original coordinates), for the edit journal.
    splices: Option<Vec<Splice>>,
}

/// Which file an `Update`/`Delete` names: exact path first, else a unique workspace match (noted).
fn existing_path(ws: &Path, cwd: Option<&Path>, p: &str) -> Result<(PathBuf, Option<String>), String> {
    if p.trim().is_empty() {
        return Err("empty file path".into());
    }
    resolve_existing(ws, cwd, p.trim(), true, false)
}

fn plan_update(
    ws: &Path,
    cwd: Option<&Path>,
    path: &str,
    move_to: Option<&str>,
    hunks: &[Hunk],
    errors: &mut Vec<String>,
) -> Option<PlannedFile> {
    let (full, resolved) = match existing_path(ws, cwd, path) {
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
    let mut edits: Vec<(usize, usize, Vec<String>, usize)> = Vec::new();
    let mut notes: Vec<String> = resolved.into_iter().collect();
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
            Hunk::Edge { at, new, header } => {
                if new.iter().all(|l| l.trim().is_empty()) {
                    errors.push(format!("{}: nothing to insert (no `+` lines)", tag(header)));
                    failed = true;
                    continue;
                }
                let mut lines = new.clone();
                let row = match at {
                    Edge::Top => top_row(&doc.lines),
                    Edge::Bottom => doc.lines.len(),
                    Edge::Imports => {
                        // Re-adding an import that is already there is a no-op, not a duplicate.
                        let have: HashSet<&str> = doc.lines.iter().map(|l| l.trim()).collect();
                        let before = lines.len();
                        lines.retain(|l| l.trim().is_empty() || !have.contains(l.trim()));
                        if lines.iter().all(|l| l.trim().is_empty()) {
                            notes.push(format!("{}: already present, nothing inserted", tag(header)));
                            continue;
                        }
                        if lines.len() < before {
                            notes.push(format!("{}: skipped {} line(s) already present", tag(header), before - lines.len()));
                        }
                        match import_end_row(lang, &doc.to_text()) {
                            Some(r) => r + 1,
                            None => top_row(&doc.lines),
                        }
                    }
                };
                edits.push((row, 0, lines, hi));
            }
            Hunk::Anchored { kind, start, end, new, header } => {
                // Plain line numbers (`@@ 12..15`) take the hash of the live line.
                let (start, end) = match (fill_plain(&doc, &rel, start, None), fill_plain(&doc, &rel, end, None)) {
                    (Ok(s), Ok(e)) => (s, e),
                    (Err(e), _) | (_, Err(e)) => {
                        errors.push(format!("{}: {e}", tag(header)));
                        failed = true;
                        continue;
                    }
                };
                let (start, end) = (&start, &end);
                let (r0, r1) = match locate_at(&full, &doc, &rel, start, end, None) {
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
            Hunk::Symbol { kind, name, new, header } => {
                let Some(l) = lang.filter(|l| *l != super::symbols::Lang::Json) else {
                    errors.push(format!("{}: no symbol support for {rel}", tag(header)));
                    failed = true;
                    continue;
                };
                let syms = symbols(l, &doc.to_text());
                let (r0, r1) = match find_symbol(&syms, name) {
                    Ok(s) => (s.start, s.end),
                    Err(e) => {
                        errors.push(format!("{}: {e}", tag(header)));
                        failed = true;
                        continue;
                    }
                };
                match kind {
                    AnchorKind::Replace if new.is_empty() => {
                        errors.push(format!("{}: no `+` lines. To remove the symbol say so: `@@ delete symbol {name}`", tag(header)));
                        failed = true;
                    }
                    AnchorKind::Replace => {
                        let k = keep_leading_docs(&doc.lines, r0, r1, new);
                        if k > 0 {
                            notes.push(format!("{}: kept {k} leading doc/attribute line(s) your new text did not restate", tag(header)));
                        }
                        edits.push((r0 + k, r1 - r0 + 1 - k, new.clone(), hi))
                    }
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
                            consistent_shift(pat, &doc.lines[idx..idx + pat_len])
                        } else {
                            None
                        };
                        let mut new_lines: Vec<String> = Vec::new();
                        let mut oi = 0usize;
                        let mut reindented = false;
                        let mut first_done = false;
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
                                        let shifted = apply_shift(l, sh, &mut first_done);
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
            let (a, b) = (new_doc.anchor(s), new_doc.anchor(s + new.len() - 1));
            ranges.push(if new.len() == 1 { a } else { format!("{a}..{b}") });
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
        match resolve_new_file(ws, cwd, m).map(|(t, _)| t) {
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
    // Parse the original only now: failed hunks bail out above without paying for two parses.
    let after_errs = syntax_errors(lang_for(&target_rel), &text);
    if let (Some(before), Some(after)) = (after_errs.as_ref().and_then(|_| syntax_errors(lang, &doc.to_text())), after_errs) {
        if after.len() > before.len() {
            if let Some((row, m)) = after
                .iter()
                .find(|(r, _)| !before.iter().any(|(b, _)| b == r))
                .or_else(|| after.first())
            {
                let row = (*row).min(new_doc.lines.len().saturating_sub(1));
                notes.push(syntax_note(&new_doc, row, m, &format!("{target_rel}: new syntax error")));
            }
        }
    }
    let moved = if remove_after.is_some() { format!(" -> {target_rel}") } else { String::new() };
    let now = if ranges.is_empty() { String::new() } else { format!("  new anchors: {}", ranges.join(", ")) };
    let summary = if edits.is_empty() && moved.is_empty() {
        format!("= {rel}  (no change)")
    } else if edits.is_empty() {
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
        splices: Some(edits.iter().map(|(s, r, n, _)| (*s, *r, n.len())).collect()),
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
            FileOp::Add { path, lines } => match resolve_new_file(ws, cwd, path) {
                Ok((full, _)) if full.exists() => errors.push(format!(
                    "{path}: already exists; use `*** Update File:` (or delete it first)"
                )),
                Ok((full, resolved)) => {
                    let rel = rel_of(ws, &full);
                    let eol = sibling_eol(&full);
                    let mut text = lines.join(eol);
                    if !text.is_empty() {
                        text.push_str(eol);
                    }
                    let mut notes: Vec<String> = resolved.into_iter().collect();
                    if let Some(errs) = syntax_errors(lang_for(&rel), &text) {
                        if let Some((row, m)) = errs.first() {
                            notes.push(syntax_note(&Doc::parse(&text), *row, m, &format!("{rel}: syntax error")));
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
                        splices: None,
                    });
                }
                Err(e) => errors.push(format!("{path}: {e}")),
            },
            FileOp::Delete { path } => match existing_path(ws, cwd, path) {
                Ok((full, resolved)) => {
                    let rel = rel_of(ws, &full);
                    plans.push(PlannedFile {
                        original: std::fs::read(&full).ok(),
                        full,
                        summary: format!("D {rel}"),
                        rel,
                        new_text: None,
                        remove_after: None,
                        notes: resolved.into_iter().collect(),
                        splices: None,
                    });
                }
                Err(e) => errors.push(format!("{path}: {e}")),
            },
            FileOp::Update { path, move_to, hunks } => {
                if let Some(p) = plan_update(ws, cwd, path, move_to.as_deref(), hunks, &mut errors) {
                    plans.push(p);
                }
            }
        }
    }
    // Two spellings (e.g. a full path and a short suffix) can name the same file.
    for (i, p) in plans.iter().enumerate() {
        if plans[..i].iter().any(|q| q.full == p.full) {
            errors.push(format!("{}: appears twice in the patch; put all its hunks under one `*** Update File:`", p.rel));
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
        // Remember how rows moved, so anchors printed before this patch still resolve exactly.
        for p in &plans {
            match (&p.new_text, &p.original, &p.remove_after) {
                (Some(new), Some(orig), None) => journal::record(&p.full, &String::from_utf8_lossy(orig), new, p.splices.clone()),
                (Some(_), _, Some(old)) => journal::forget(old),
                (None, _, _) => journal::forget(&p.full),
                _ => {}
            }
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

/// Line ending for a new file: follow its neighbours (the first sibling with a newline decides),
/// defaulting to LF.
fn sibling_eol(full: &Path) -> &'static str {
    let Some(dir) = full.parent() else { return "\n" };
    let Ok(rd) = std::fs::read_dir(dir) else { return "\n" };
    for e in rd.flatten().take(40) {
        let p = e.path();
        if !p.is_file() || lang_for(&p.to_string_lossy()).is_none() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if let Some(i) = bytes.iter().take(1 << 16).position(|&b| b == b'\n') {
            return if i > 0 && bytes[i - 1] == b'\r' { "\r\n" } else { "\n" };
        }
    }
    "\n"
}

/// Row for `@@ top`: line 1, but after a `#!` shebang and leading file-level directives that must
/// stay first (`//!` / `#![...]` in Rust, `'use client'` / `"use strict"` in JS).
fn top_row(lines: &[String]) -> usize {
    let mut row = 0;
    if lines.first().is_some_and(|l| l.starts_with("#!") && !l.starts_with("#![")) {
        row = 1;
    }
    while let Some(l) = lines.get(row) {
        let t = l.trim_start();
        let directive = t.starts_with("//!")
            || t.starts_with("#![")
            || ((t.starts_with("'use ") || t.starts_with("\"use ")) && t.trim_end().trim_end_matches(';').len() < 40);
        if !directive {
            break;
        }
        row += 1;
    }
    row
}
