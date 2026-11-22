//! Single-file anchor edits, file creation, and ast-grep structural search/rewrite.

use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::doc::{line_hash, locate, parse_anchor_in_doc, read_doc, splice, text_to_lines, Doc};
use super::symbols::{lang_for, syntax_errors, Lang};
use super::util::{command, rel_of, resolve, run_capture};

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
    std::fs::write(full, text.as_bytes()).map_err(|e| format!("cannot write {rel}: {e}"))?;
    let mut msg = summary;
    if let Some((a, b)) = new_range {
        msg.push_str(&format!(" Now {}..{}.", doc.anchor(a), doc.anchor(b)));
    }
    if let (Some(before), Some(after)) = (before, syntax_errors(lang, &text)) {
        if after.len() > before.len() {
            let pick = new_range
                .and_then(|(a, b)| after.iter().find(|(r, _)| *r + 1 >= a && *r <= b + 1))
                .or_else(|| after.first());
            if let Some((row, m)) = pick {
                let row = (*row).min(doc.lines.len().saturating_sub(1));
                msg.push_str(&format!("\n⚠ new syntax error at {}: {m}", doc.anchor(row)));
            }
        }
    }
    Ok(msg)
}

/// Replace / insert / delete lines identified by `N#hh` anchors.
pub fn hashline_edit(
    ws: &Path,
    path: &str,
    op: &str,
    start: &str,
    end: Option<&str>,
    new_text: &str,
) -> Result<String, String> {
    let (full, rel, mut doc) = read_doc(ws, path)?;
    let lang = lang_for(&rel);
    let before = syntax_errors(lang, &doc.to_text());
    let s = parse_anchor_in_doc(&doc, &rel, start)?;
    let e = match end {
        Some(e) if !e.trim().is_empty() => parse_anchor_in_doc(&doc, &rel, e)?,
        _ => s.clone(),
    };
    let (r0, r1) = locate(&doc, &rel, &s, &e)?;
    let new_lines = text_to_lines(new_text);
    match op {
        "replace" | "" => {
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

pub fn create(ws: &Path, path: &str, content: &str, force: bool) -> Result<String, String> {
    let full = resolve(ws, path)?;
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
            let line = content.lines().nth(*row).unwrap_or("");
            msg.push_str(&format!("\n⚠ syntax error at {}#{}: {m}", row + 1, line_hash(line)));
        }
    }
    Ok(msg)
}

// --------------------------------- ast-grep ---------------------------------------------------

pub fn ast(
    ws: &Path,
    pattern: &str,
    lang: Option<&str>,
    path: Option<&str>,
    rewrite: Option<&str>,
    dry_run: bool,
) -> Result<String, String> {
    if pattern.trim().is_empty() {
        return Err("empty pattern".into());
    }
    let target = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => resolve(ws, p)?,
        None => ws.to_path_buf(),
    };
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
