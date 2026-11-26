//! Single-file anchor edits, file creation, and ast-grep structural search/rewrite.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use super::patch::{apply_patch, closest_hint, seek_lines};
use super::doc::{line_hash, parse_anchor_in_doc, read_doc_smart, splice, syntax_note, text_to_lines, Doc};
use super::symbols::{find_symbol, lang_for, symbols, syntax_errors, Lang};
use super::util::{command, rel_of, resolve_existing, resolve_new_file, run_capture, with_note};

pub(crate) fn finish_edit(
    full: &Path,
    rel: &str,
    doc: &Doc,
    lang: Option<Lang>,
    before: Option<Vec<(usize, String)>>,
    new_range: Option<(usize, usize)>,
    summary: String,
) -> Result<String, String> {
    let text = doc.to_text();
    let old_text = std::fs::read_to_string(full).ok();
    std::fs::write(full, text.as_bytes()).map_err(|e| format!("cannot write {rel}: {e}"))?;
    if let Some(old) = old_text {
        // Remember how rows moved, so anchors printed before this edit still resolve exactly.
        super::journal::record(full, &old, &text, None);
    }
    let mut msg = summary;
    if let Some((a, b)) = new_range {
        msg.push_str(&if a == b {
            format!(" New anchor: {}.", doc.anchor(a))
        } else {
            format!(" New anchors: {}..{}.", doc.anchor(a), doc.anchor(b))
        });
        // Fresh anchors for the edited block and its neighbours, so a follow-up edit needs no re-view.
        let last = doc.lines.len().saturating_sub(1);
        let (from, to) = (a.saturating_sub(2), (b + 2).min(last));
        if !doc.lines.is_empty() && from <= to {
            let (inner_a, inner_b) = (a.max(from), b.min(to));
            if inner_b.saturating_sub(inner_a) < 12 {
                msg.push_str(&format!("\n{}", doc.hashlines(from, to).trim_end()));
            } else {
                msg.push_str(&format!(
                    "\n{}\n…[{} lines]…\n{}",
                    doc.hashlines(from, inner_a + 5).trim_end(),
                    inner_b - inner_a - 11,
                    doc.hashlines(inner_b - 5, to).trim_end()
                ));
            }
        }
    }
    if let (Some(before), Some(after)) = (before, syntax_errors(lang, &text)) {
        if after.len() > before.len() {
            let pick = new_range
                .and_then(|(a, b)| after.iter().find(|(r, _)| *r + 1 >= a && *r <= b + 1))
                .or_else(|| after.first());
            if let Some((row, m)) = pick {
                let row = (*row).min(doc.lines.len().saturating_sub(1));
                msg.push_str(&format!("\n{}", syntax_note(doc, row, m, "new syntax error")));
            }
        }
    }
    Ok(msg)
}

/// `12#ab..15#cd` (optionally bracketed, as printed by `outline`) given as `start` alone.
fn split_range(start: &str, end: Option<&str>) -> (String, Option<String>) {
    let start = start.trim().trim_start_matches('[').trim_end_matches(']').trim();
    let end = end.map(str::trim).filter(|e| !e.is_empty());
    match (end, start.split_once("..")) {
        (None, Some((a, b))) if !a.trim().is_empty() && !b.trim().is_empty() => {
            (a.trim().to_string(), Some(b.trim().to_string()))
        }
        _ => (start.to_string(), end.map(str::to_string)),
    }
}

pub fn hashline_edit(
    ws: &Path,
    path: &str,
    op: &str,
    start: &str,
    end: Option<&str>,
    new_text: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let (full, rel, doc, note) = read_doc_smart(ws, path, cwd)?;
    hashline_edit_doc(full, rel, doc, op, start, end, new_text).map(|out| with_note(note, out))
}

fn hashline_edit_doc(
    full: PathBuf,
    rel: String,
    mut doc: Doc,
    op: &str,
    start: &str,
    end: Option<&str>,
    new_text: &str,
) -> Result<String, String> {
    let (start, end) = split_range(start, end);
    let (start, end) = (start.as_str(), end.as_deref());
    let lang = lang_for(&rel);
    let before = syntax_errors(lang, &doc.to_text());
    let s = parse_anchor_in_doc(&doc, &rel, start)?;
    let e = match end {
        Some(e) if !e.trim().is_empty() => parse_anchor_in_doc(&doc, &rel, e)?,
        _ => s.clone(),
    };
    let hint = super::doc::anchor_snippet(start);
    let (r0, r1) = super::journal::locate_at(&full, &doc, &rel, &s, &e, hint)?;
    let new_lines = text_to_lines(new_text);
    match op {
        "replace" | "" => {
            if new_lines.is_empty() {
                return Err(
                    "`new_text` is empty, so a replace would delete the lines. Pass the replacement text, or use op `delete` to remove them."
                        .into(),
                );
            }
            if new_lines.len() == (r1 - r0 + 1)
                && new_lines.iter().zip(&doc.lines[r0..=r1]).all(|(a, b)| a == b)
            {
                return Ok(format!(
                    "No change: new text is identical to {rel} lines {}..{}.",
                    r0 + 1,
                    r1 + 1
                ));
            }
            let (k, removed) = (new_lines.len(), r1 - r0 + 1);
            splice(&mut doc, r0, removed, new_lines);
            let summary = if k == 0 {
                format!("Deleted {rel} lines {}..{} (-{removed}).", r0 + 1, r1 + 1)
            } else {
                format!("Edited {rel} lines {}..{} (+{k} -{removed}).", r0 + 1, r1 + 1)
            };
            let nr = if k > 0 { Some((r0, r0 + k - 1)) } else { None };
            finish_edit(&full, &rel, &doc, lang, before, nr, summary)
        }
        "delete" => {
            splice(&mut doc, r0, r1 - r0 + 1, Vec::new());
            finish_edit(
                &full,
                &rel,
                &doc,
                lang,
                before,
                None,
                format!("Deleted {rel} lines {}..{} (-{}).", r0 + 1, r1 + 1, r1 - r0 + 1),
            )
        }
        "insert_after" | "insert_before" => {
            if new_lines.is_empty() {
                return Err("new_text is empty".into());
            }
            let at = if op == "insert_after" { r1 + 1 } else { r0 };
            let k = new_lines.len();
            splice(&mut doc, at, 0, new_lines);
            finish_edit(
                &full,
                &rel,
                &doc,
                lang,
                before,
                Some((at, at + k - 1)),
                format!("Inserted {k} line(s) in {rel} at line {}.", at + 1),
            )
        }
        other => Err(format!("unknown edit op `{other}` (replace|insert_after|insert_before|delete)")),
    }
}

pub fn create(ws: &Path, path: &str, content: &str, force: bool, cwd: Option<&Path>) -> Result<String, String> {
    let (full, note) = resolve_new_file(ws, cwd, path)?;
    let rel = rel_of(ws, &full);
    if full.exists() && !force {
        return Err(format!("{rel} already exists (use edit, or force to overwrite)"));
    }
    if let Some(parent) = full.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    std::fs::write(&full, content.as_bytes()).map_err(|e| format!("cannot write {rel}: {e}"))?;
    let mut msg = format!("Created {rel} ({} bytes).", content.len());
    if let Some(errs) = syntax_errors(lang_for(&rel), content) {
        if let Some((row, m)) = errs.first() {
            msg.push_str(&format!("\n{}", syntax_note(&Doc::parse(content), *row, m, "syntax error")));
        }
    }
    Ok(with_note(note, msg))
}

// --------------------------------- ast-grep ---------------------------------------------------

pub fn ast(
    ws: &Path,
    pattern: &str,
    lang: Option<&str>,
    path: Option<&str>,
    rewrite: Option<&str>,
    dry_run: bool,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let (target, note) = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => resolve_existing(ws, cwd, p, true, true)?,
        None => (ws.to_path_buf(), None),
    };
    ast_at(ws, pattern, lang, target, rewrite, dry_run).map(|out| with_note(note, out))
}

fn ast_at(
    ws: &Path,
    pattern: &str,
    lang: Option<&str>,
    target: PathBuf,
    rewrite: Option<&str>,
    dry_run: bool,
) -> Result<String, String> {
    if pattern.trim().is_empty() {
        return Err("empty pattern".into());
    }
    let Some(langs) = super::astgrep::builtin_langs(lang) else {
        // Not one of the built-in grammars: defer to an installed ast-grep CLI.
        return ast_cli(ws, &rel_of(ws, &target), pattern, lang, rewrite, dry_run);
    };
    // A dry run only reports what would change, so never write.
    let apply = rewrite.filter(|_| !dry_run);
    let out = super::astgrep::run(ws, &target, &langs, pattern, apply)?;
    if out.matches.is_empty() && lang.is_none_or(|l| l.trim().is_empty()) && cli_available() {
        // No hit in the built-in languages; other languages may still match via the CLI.
        return ast_cli(ws, &rel_of(ws, &target), pattern, lang, rewrite, dry_run);
    }
    if apply.is_some() && !out.rewritten.is_empty() {
        return Ok(rewrite_report(ws, &out.rewritten, out.rewritten.iter().map(|r| r.1).sum()));
    }
    Ok(match_report(&out.matches, dry_run && rewrite.is_some()))
}

fn match_report(matches: &[(String, usize, String)], dry_run: bool) -> String {
    if matches.is_empty() {
        return "No matches.".into();
    }
    let mut res = if dry_run {
        format!("{} match(es) (dry run, nothing written):\n", matches.len())
    } else {
        format!("{} match(es):\n", matches.len())
    };
    for (f, l, t) in matches.iter().take(60) {
        let shown: String = t.trim().chars().take(160).collect();
        res.push_str(&format!("{f}:{l}#{}| {shown}\n", line_hash(t)));
    }
    if matches.len() > 60 {
        res.push_str(&format!("…[{} more]\n", matches.len() - 60));
    }
    res
}

fn rewrite_report(ws: &Path, per_file: &[(String, usize)], total: usize) -> String {
    let mut res = format!("Rewrote {} match(es) in {} file(s):\n", total, per_file.len());
    for (f, n) in per_file.iter().take(40) {
        res.push_str(&format!("  {f} ({n})\n"));
    }
    for (f, _) in per_file.iter().take(40) {
        if let Ok(text) = std::fs::read_to_string(ws.join(f)) {
            if let Some(errs) = syntax_errors(lang_for(f), &text) {
                if let Some((row, m)) = errs.first() {
                    let line = text.lines().nth(*row).unwrap_or("");
                    res.push_str(&format!(
                        "⚠ {f} has a syntax error at {}#{}: {m}\n",
                        row + 1,
                        line_hash(line)
                    ));
                }
            }
        }
    }
    res
}

fn cli_available() -> bool {
    command("ast-grep")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// External `ast-grep` CLI, for languages without a built-in grammar.
fn ast_cli(
    ws: &Path,
    target: &str,
    pattern: &str,
    lang: Option<&str>,
    rewrite: Option<&str>,
    dry_run: bool,
) -> Result<String, String> {
    if !cli_available() {
        return Err(format!(
            "language '{}' is not built in (built in: rust, typescript, tsx, javascript, python, go, json); \
             install the ast-grep CLI for others (npm i -g @ast-grep/cli)",
            lang.unwrap_or("?")
        ));
    }
    let target = if target.is_empty() { "." } else { target };
    let build = |json: bool, update: bool| {
        let mut c = command("ast-grep");
        // `--opt=value` and `--` keep a pattern, rewrite or path starting with `-` from being
        // read as a flag.
        c.arg("run").arg(format!("--pattern={pattern}"));
        if let Some(l) = lang.filter(|l| !l.is_empty()) {
            c.arg(format!("--lang={l}"));
        }
        if let Some(r) = rewrite {
            c.arg(format!("--rewrite={r}"));
        }
        if json {
            c.arg("--json=stream");
        }
        if update {
            c.arg("-U");
        }
        c.arg("--").arg(target).current_dir(ws);
        c
    };
    let out = run_capture(build(true, false), Duration::from_secs(60))?;
    // ast-grep exits 1 with empty output when nothing matched; that is not a failure.
    if out.code != 0
        && out.stdout.trim().is_empty()
        && !out.stderr.trim().is_empty()
        && !out.stderr.contains("No matches")
    {
        return Err(format!(
            "ast-grep failed: {}",
            out.stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    let mut matches: Vec<(String, usize, String)> = Vec::new();
    for l in out.stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
        let file = v["file"].as_str().unwrap_or("").replace('\\', "/");
        let file = file.trim_start_matches("./").to_string();
        let line = v["range"]["start"]["line"].as_u64().unwrap_or(0) as usize + 1;
        let first = v["lines"]
            .as_str()
            .or_else(|| v["text"].as_str())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        matches.push((file, line, first));
    }
    if matches.is_empty() || rewrite.is_none() || dry_run {
        return Ok(match_report(&matches, dry_run && rewrite.is_some()));
    }
    let applied = run_capture(build(false, true), Duration::from_secs(120))?;
    if applied.code != 0 {
        return Err(format!(
            "ast-grep rewrite failed: {}",
            applied.stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    let mut per_file: Vec<(String, usize)> = Vec::new();
    for (f, _, _) in &matches {
        match per_file.iter_mut().find(|(n, _)| n == f) {
            Some(e) => e.1 += 1,
            None => per_file.push((f.clone(), 1)),
        }
    }
    Ok(rewrite_report(ws, &per_file, matches.len()))
}

/// Selector shorthand shared by `edit` and every entry of `edits`: `at` is an alias of `start`, and
/// `start` may carry the op and target kind inline, the same words `apply` headers use:
/// `delete 12#ab..15#cd`, `after 12`, `before 12#ab`, `symbol Foo::bar`, `delete symbol Foo`.
/// An explicit `op` always wins. Parameter aliases (`file_path`, `new_string`, ...) are applied too.
pub(crate) fn canon_edit(v: &Value) -> Value {
    let mut map = match super::with_aliases("edit", v) {
        Value::Object(m) => m,
        other => return other,
    };
    let at = map.remove("at");
    if map.get("start").map_or(true, Value::is_null) {
        if let Some(a) = at.filter(|a| !a.is_null()) {
            map.insert("start".into(), a);
        }
    }
    let Some(sel) = map.get("start").and_then(Value::as_str).map(|s| s.trim().to_string()) else {
        return Value::Object(map);
    };
    if super::patch_parse::parse_edge(&sel).is_some() {
        return Value::Object(map);
    }
    let re = lazy_re!(r"^(?i:(delete|after|before|replace|insert_after|insert_before)\s+)?(?i:(symbol)\s+)?(\S.*)$");
    if let Some(c) = re.captures(&sel) {
        let kw = c.get(1).map(|m| m.as_str().to_ascii_lowercase());
        let is_symbol = c.get(2).is_some();
        if kw.is_some() || is_symbol {
            if let Some(kw) = kw {
                let op = match kw.as_str() {
                    "delete" => "delete",
                    "after" | "insert_after" => "insert_after",
                    "before" | "insert_before" => "insert_before",
                    _ => "replace",
                };
                if map.get("op").map_or(true, Value::is_null) {
                    map.insert("op".into(), Value::String(op.into()));
                }
            }
            let rest = Value::String(c[3].trim().to_string());
            if is_symbol {
                map.remove("start");
                map.insert("symbol".into(), rest);
            } else {
                map.insert("start".into(), rest);
            }
        }
    }
    Value::Object(map)
}

enum OldHit {
    None,
    /// 0-based rows where each occurrence starts.
    Many(Vec<usize>),
    /// The rows to replace and what they become (empty = delete them).
    One { r0: usize, r1: usize, lines: Vec<String> },
}

/// Find the exact text `old` in rows `lo..=hi` of `doc` and work out the rows `new` replaces it in.
/// Unlike a whole-line context match this also handles text from the middle of a line.
fn resolve_old_text(doc: &Doc, old: &str, new: &str, lo: usize, hi: usize) -> OldHit {
    let needle = old.replace("\r\n", "\n");
    let new = new.replace("\r\n", "\n");
    if needle.is_empty() || doc.lines.is_empty() || lo > hi || hi >= doc.lines.len() {
        return OldHit::None;
    }
    let mut hay = String::new();
    let mut starts = Vec::with_capacity(hi - lo + 2);
    for r in lo..=hi {
        starts.push(hay.len());
        hay.push_str(&doc.lines[r]);
        hay.push('\n');
    }
    starts.push(hay.len());
    let row_of = |p: usize| starts.partition_point(|&s| s <= p) - 1;
    let hits: Vec<usize> = hay.match_indices(needle.as_str()).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [] => OldHit::None,
        [p] => {
            let end = p + needle.len();
            let (i0, i1) = (row_of(*p), row_of(end - 1));
            let text = format!("{}{}{}", &hay[starts[i0]..*p], new, &hay[end..starts[i1 + 1]]);
            let lines = if new.trim().is_empty() && text.trim().is_empty() {
                Vec::new()
            } else {
                text.strip_suffix('\n').unwrap_or(&text).split('\n').map(str::to_string).collect()
            };
            OldHit::One { r0: lo + i0, r1: lo + i1, lines }
        }
        many => OldHit::Many(many.iter().map(|p| lo + row_of(*p)).collect()),
    }
}

fn push_plus(patch: &mut String, lines: impl IntoIterator<Item = impl AsRef<str>>) {
    for l in lines {
        patch.push('+');
        patch.push_str(l.as_ref());
        patch.push('\n');
    }
}

/// `edit {old_text, new_text}` matched more than once: say where, so the retry can scope it.
pub(crate) fn with_match_anchors(ws: &Path, path: &str, cwd: Option<&Path>, old: &str, err: String) -> String {
    if !err.starts_with("expected 1 occurrence(s), found") {
        return err;
    }
    let Ok((_, _, doc, _)) = read_doc_smart(ws, path, cwd) else { return err };
    match resolve_old_text(&doc, old, "", 0, doc.lines.len().saturating_sub(1)) {
        OldHit::Many(rows) => format!("{err}\n{}", many_hint(&doc, &rows)),
        _ => err,
    }
}

fn many_hint(doc: &Doc, rows: &[usize]) -> String {
    let shown: Vec<String> = rows.iter().take(6).map(|r| doc.anchor(*r)).collect();
    format!(
        "old_text starts at {}{}. Add surrounding text, or scope it: pass `start` (a line range around the one you mean, e.g. \"{}..{}\") or `symbol` together with `old_text`.",
        shown.join(", "),
        if rows.len() > 6 { format!(" (+{} more)", rows.len() - 6) } else { String::new() },
        shown[0],
        doc.anchor((rows[0] + 3).min(doc.lines.len() - 1))
    )
}

/// `edit {path, patch}`: a patch given without the `*** Update File:` envelope is wrapped for `path`.
/// Hunks may use plain line numbers and need no `+` prefix (see `apply`).
pub fn patch_edit(ws: &Path, path: &str, patch: &str, cwd: Option<&Path>) -> Result<String, String> {
    let has_envelope = patch.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("*** Begin Patch")
            || t.starts_with("*** Update File:")
            || t.starts_with("*** Add File:")
            || t.starts_with("*** Delete File:")
    });
    let full = if has_envelope {
        patch.to_string()
    } else if path.trim().is_empty() {
        return Err("`patch` needs `path` (or `*** Update File: <path>` in the patch)".into());
    } else {
        format!("*** Begin Patch\n*** Update File: {}\n{}\n*** End Patch\n", path.trim(), patch.trim_end_matches('\n'))
    };
    apply_patch(ws, &full, cwd, false)
}

/// Several edits in a single atomic call, built on the `apply` patch path. Every anchor refers to
/// the file as it is now (not after earlier entries). Each entry picks its target one of three ways
/// (`start`/`at` anchors, `old_text` exact text, or `symbol` name) and may carry its own `path`, so
/// one call can edit several files; `path` is the default for entries without one. `old_text` may be
/// part of a line and may be scoped by a `start` range or a `symbol`.
pub fn edit_batch(ws: &Path, path: &str, edits: &[serde_json::Value], cwd: Option<&Path>) -> Result<String, String> {
    if edits.is_empty() {
        return Err("`edits` is empty".into());
    }
    let edits: Vec<Value> = edits.iter().map(canon_edit).collect();
    // Group entries per file (first-appearance order) so each file is one `Update File` section.
    let mut files: Vec<(String, Vec<(usize, &serde_json::Value)>)> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        let p = e.get("path").and_then(|v| v.as_str()).filter(|p| !p.trim().is_empty()).unwrap_or(path).trim();
        if p.is_empty() {
            return Err(format!("edits[{}]: missing `path` (pass it per edit or once at the top)", i + 1));
        }
        match files.iter_mut().find(|(f, _)| f == p) {
            Some((_, list)) => list.push((i + 1, e)),
            None => files.push((p.to_string(), vec![(i + 1, e)])),
        }
    }
    const NEW_KEYS: [&str; 5] = ["new_text", "new_string", "newString", "content", "text"];
    const OLD_KEYS: [&str; 4] = ["old_text", "old_string", "oldString", "old"];
    let mut patch = String::from("*** Begin Patch\n");
    for (file, list) in &files {
        patch.push_str(&format!("*** Update File: {file}\n"));
        // The file as it is now, for `old_text` entries (unreadable: the patch engine says why).
        let loaded = read_doc_smart(ws, file, cwd).ok();
        let written: Vec<(usize, &str)> = list
            .iter()
            .filter_map(|(n, e)| NEW_KEYS.iter().find_map(|k| e.get(*k).and_then(|v| v.as_str())).map(|t| (*n, t)))
            .collect();
        for (n, e) in list {
            let n = *n;
            let get = |ks: &[&str]| ks.iter().find_map(|k| e.get(*k).and_then(|v| v.as_str()));
            let text = get(&NEW_KEYS).unwrap_or("");
            let body = |patch: &mut String| {
                push_plus(patch, text.replace("\r\n", "\n").trim_end_matches('\n').split('\n'));
            };
            let op = get(&["op"]).unwrap_or("replace");
            // `old_text`: exact text replaced with `new_text`, no anchors; `start`/`symbol` beside it
            // only narrow where it is searched.
            if let Some(old) = get(&OLD_KEYS) {
                if old.trim().is_empty() {
                    return Err(format!("edits[{n}]: old_text is empty"));
                }
                if let Some((full, rel, doc)) = loaded.as_ref().map(|(full, rel, doc, _)| (full, rel, doc)).filter(|(_, _, d)| !d.lines.is_empty()) {
                    let all = (0, doc.lines.len() - 1);
                    let (lo, hi, scope) = if let Some(st) = get(&["start"]) {
                        let (a, b) = split_range(st, get(&["end"]));
                        let s = parse_anchor_in_doc(doc, rel, &a).map_err(|e| format!("edits[{n}]: {e}"))?;
                        let en = match &b {
                            Some(b) => parse_anchor_in_doc(doc, rel, b).map_err(|e| format!("edits[{n}]: {e}"))?,
                            None => s.clone(),
                        };
                        let (r0, r1) = super::journal::locate_at(full, doc, rel, &s, &en, super::doc::anchor_snippet(&a))
                            .map_err(|e| format!("edits[{n}]: {e}"))?;
                        (r0, r1, format!(" within lines {}..{}", r0 + 1, r1 + 1))
                    } else if let Some(sym) = get(&["symbol"]) {
                        let lang = lang_for(rel).filter(|l| *l != Lang::Json).ok_or_else(|| format!("edits[{n}]: no symbol support for {rel}"))?;
                        let syms = symbols(lang, &doc.to_text());
                        let s = find_symbol(&syms, sym).map_err(|e| format!("edits[{n}]: {e}"))?;
                        (s.start, s.end, format!(" within symbol {}", sym.trim()))
                    } else {
                        (all.0, all.1, String::new())
                    };
                    match resolve_old_text(doc, old, text, lo, hi) {
                        OldHit::One { r0, r1, lines } => {
                            let range = if r0 == r1 { doc.anchor(r0) } else { format!("{}..{}", doc.anchor(r0), doc.anchor(r1)) };
                            if lines.is_empty() {
                                patch.push_str(&format!("@@ delete {range}\n"));
                            } else {
                                patch.push_str(&format!("@@ {range}\n"));
                                push_plus(&mut patch, &lines);
                            }
                            continue;
                        }
                        OldHit::Many(rows) => {
                            return Err(format!("edits[{n}]: old_text occurs {} times{scope} in {file}. {}", rows.len(), many_hint(doc, &rows)));
                        }
                        OldHit::None => {}
                    }
                    // No exact hit: a whole-line match that ignores indentation is still fine (the
                    // patch engine re-indents `new_text`); otherwise fail here with a precise reason.
                    let old_lines: Vec<String> = old.replace("\r\n", "\n").trim_end_matches('\n').split('\n').map(str::to_string).collect();
                    let loose = if scope.is_empty() { seek_lines(&doc.lines, &old_lines, 0, false) } else { Ok(None) };
                    match loose {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            let needle = old.trim();
                            let overlap = written
                                .iter()
                                .find(|(j, t)| *j < n && t.contains(needle))
                                .map(|(j, _)| format!(" That text is written by edits[{j}]: every entry refers to the file as it is now, so merge the two into one edit."))
                                .unwrap_or_default();
                            return Err(format!(
                                "edits[{n}]: old_text not found{scope} in {file}.{overlap}{}",
                                if scope.is_empty() { closest_hint(doc, &old_lines) } else { String::new() }
                            ));
                        }
                        Err(e) => return Err(format!("edits[{n}]: old_text {e}")),
                    }
                }
                patch.push_str("@@\n");
                for l in old.replace("\r\n", "\n").trim_end_matches('\n').split('\n') {
                    patch.push('-');
                    patch.push_str(l);
                    patch.push('\n');
                }
                if !text.is_empty() {
                    body(&mut patch);
                }
                continue;
            }
            // `symbol`: a whole declaration by name, no anchors.
            if get(&["start"]).is_none() {
                if let Some(sym) = get(&["symbol"]) {
                    let head = match op {
                        "replace" => "symbol",
                        "delete" => "delete symbol",
                        "insert_after" => "after symbol",
                        "insert_before" => "before symbol",
                        other => return Err(format!("edits[{n}]: unknown op `{other}` (replace, insert_after, insert_before, delete)")),
                    };
                    if op != "delete" && text.is_empty() {
                        return Err(format!("edits[{n}]: new_text is empty (use op `delete` to remove the symbol)"));
                    }
                    patch.push_str(&format!("@@ {head} {}\n", sym.trim()));
                    if op != "delete" {
                        body(&mut patch);
                    }
                    continue;
                }
            }
            let start = get(&["start"]).ok_or_else(|| format!("edits[{n}]: needs `start`/`at` (anchor), `old_text`, or `symbol`"))?;
            let (a, b) = split_range(start, get(&["end"]));
            let range = match &b {
                Some(b) => format!("{a}..{b}"),
                None => a.clone(),
            };
            // `start: "top" | "bottom" | "imports"` inserts there whatever the op says (except delete).
            if super::patch_parse::parse_edge(start).is_some() {
                if text.is_empty() || op == "delete" {
                    return Err(format!("edits[{n}]: `{start}` inserts lines; pass new_text (delete needs anchors)"));
                }
                patch.push_str(&format!("@@ {}\n", start.trim()));
                body(&mut patch);
                continue;
            }
            match op {
                "replace" => {
                    if text.is_empty() {
                        return Err(format!("edits[{n}]: new_text is empty (use op `delete` to remove lines)"));
                    }
                    patch.push_str(&format!("@@ {range}\n"));
                    body(&mut patch);
                }
                "delete" => patch.push_str(&format!("@@ delete {range}\n")),
                "insert_after" | "insert_before" if text.is_empty() => {
                    return Err(format!("edits[{n}]: new_text is empty"));
                }
                "insert_after" => {
                    patch.push_str(&format!("@@ after {}\n", b.as_deref().unwrap_or(&a)));
                    body(&mut patch);
                }
                "insert_before" => {
                    patch.push_str(&format!("@@ before {a}\n"));
                    body(&mut patch);
                }
                other => return Err(format!("edits[{n}]: unknown op `{other}` (replace, insert_after, insert_before, delete)")),
            }
        }
    }
    patch.push_str("*** End Patch\n");
    let single = files.len() == 1;
    apply_patch(ws, &patch, cwd, false).map_err(|e| {
        let e = if single { name_overlapping_edits(&e, &files[0].1) } else { e };
        format!("{e}\n(no edits were applied)")
    })
}

/// `hunks 2 and 3 touch the same lines` -> also say which `edits[..]` entries those are.
fn name_overlapping_edits(err: &str, list: &[(usize, &Value)]) -> String {
    let re = lazy_re!(r"hunks (\d+) and (\d+) touch the same lines");
    let Some(c) = re.captures(err) else { return err.to_string() };
    let entry = |h: &str| h.parse::<usize>().ok().and_then(|h| list.get(h.checked_sub(1)?)).map(|(n, _)| *n);
    match (entry(&c[1]), entry(&c[2])) {
        (Some(a), Some(b)) => format!("{err}\n(edits[{a}] and edits[{b}] overlap: merge them into one edit; all entries refer to the file as it is now)"),
        _ => err.to_string(),
    }
}

/// Fresh `N#hh` anchors for `new_text` after an anchor-free (`old_text`) replace, so a caller that
/// never viewed the file can keep editing by anchor without a re-read.
pub(crate) fn anchors_after_replace(ws: &Path, path: &str, cwd: Option<&Path>, new_text: &str) -> Option<String> {
    let new_lines = text_to_lines(new_text);
    let k = new_lines.len();
    if k == 0 || new_text.trim().is_empty() {
        return None;
    }
    let (_, _, doc, _) = read_doc_smart(ws, path, cwd).ok()?;
    let a = (0..=doc.lines.len().checked_sub(k)?).find(|&i| doc.lines[i..i + k] == new_lines[..])?;
    let b = a + k - 1;
    let head = if a == b {
        format!("New anchor: {}.", doc.anchor(a))
    } else {
        format!("New anchors: {}..{}.", doc.anchor(a), doc.anchor(b))
    };
    if k > 12 {
        return Some(head);
    }
    let last = doc.lines.len() - 1;
    Some(format!("{head}\n{}", doc.hashlines(a.saturating_sub(2), (b + 2).min(last)).trim_end()))
}
