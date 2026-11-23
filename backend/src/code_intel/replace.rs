//! Atomic literal (default) or regex find-and-replace across one file, a directory, or a glob.
//! Preserves CRLF, runs tree-sitter syntax validation, supports `--dry-run` and `--expected N`.

use regex::Regex;
use std::path::{Path, PathBuf};

use super::check::clip;
use super::doc::{line_hash, Doc};
use super::patch::{apply_shift, closest_hint, indent_shift, seek_lines, MatchLevel};
use super::search::{collect_files, GlobMatcher};
use super::symbols::{lang_for, syntax_errors};
use super::util::{missing_path_err, rel_of, resolve_with_cwd, top_level_dirs};

#[derive(Clone, Debug, Default)]
pub struct ReplaceOpts {
    pub path: Option<String>,
    pub globs: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub regex: bool,
    pub dry_run: bool,
    pub expected: Option<usize>,
    pub all_files: bool,
}

struct PlannedReplace {
    full: PathBuf,
    rel: String,
    original: Vec<u8>,
    new_text: String,
    count: usize,
    anchors: Vec<String>,
    preview_lines: Vec<String>,
    syntax_note: Option<String>,
    /// Matched after ignoring indentation / trailing whitespace (reported to the caller).
    loose: bool,
}

/// Atomic literal (default) or regex find-and-replace across a file, directory, or glob.
/// Eliminates the need for `sed -i` or ad-hoc Python scripts while keeping CRLF, syntax checks,
/// dry-run previews, and rollback guarantees.
pub fn replace_text(ws: &Path, old: &str, new: &str, opts: &ReplaceOpts) -> Result<String, String> {
    if old.is_empty() {
        return Err("`old` pattern cannot be empty".into());
    }
    if old == new && !opts.regex {
        return Ok("No change: `old` and `new` are identical.".into());
    }
    let has_path = opts.path.as_deref().map(|p| !p.trim().is_empty()).unwrap_or(false);
    let has_globs = opts.globs.iter().any(|g| !g.trim().is_empty());
    if !has_path && !has_globs && !opts.all_files && !opts.dry_run {
        return Err(
            "refusing to replace across the entire workspace without a path or --glob (pass a path/glob, --dry-run, or --all-files)"
                .into(),
        );
    }

    let files: Vec<(PathBuf, String)> = if let Some(p) = opts.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        let full = resolve_with_cwd(ws, opts.cwd.as_deref(), p)?;
        if !full.exists() {
            let rel_attempt = rel_of(ws, &full);
            return Err(missing_path_err(ws, &rel_attempt, true, true));
        }
        let rel = rel_of(ws, &full);
        if full.is_file() {
            if has_globs {
                let matcher = GlobMatcher::compile(&opts.globs)?;
                let fname = full.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if !matcher.matches(&rel, &fname) {
                    return Err(format!("{rel} does not match glob `{}`", opts.globs.join(", ")));
                }
            }
            vec![(full, rel)]
        } else {
            collect_files(ws, &[rel], &opts.globs)?
        }
    } else {
        collect_files(ws, &[], &opts.globs)?
    };

    if files.is_empty() {
        let tops = top_level_dirs(ws);
        let tip = if tops.is_empty() {
            String::new()
        } else {
            format!(" (workspace root dirs: {})", tops.join(", "))
        };
        return Err(format!("no files matched the target scope{tip}"));
    }

    let re = if opts.regex {
        Some(Regex::new(old).map_err(|e| format!("invalid regex `{old}`: {e}"))?)
    } else {
        None
    };
    let old_lf = old.replace("\r\n", "\n");
    let new_lf = new.replace("\r\n", "\n");
    let single_explicit = files.len() == 1 && has_path && files[0].0.is_file();

    let mut plans: Vec<PlannedReplace> = Vec::new();
    let mut total_count = 0usize;
    let mut miss_hint = String::new();

    for (full, rel) in files {
        if !single_explicit && full.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
            continue;
        }
        let bytes = match std::fs::read(&full) {
            Ok(b) => b,
            Err(e) if single_explicit => return Err(format!("cannot read {rel}: {e}")),
            Err(_) => continue,
        };
        let text = match String::from_utf8(bytes.clone()) {
            Ok(t) => t,
            Err(_) if single_explicit => return Err(format!("{rel} is not UTF-8 text")),
            Err(_) => continue,
        };
        let doc = Doc::parse(&text);
        let use_crlf = doc.eol == "\r\n";
        let mut loose = false;

        let (count, replaced, preview_lines) = if let Some(ref rx) = re {
            let mut count = 0usize;
            let mut preview = Vec::new();
            for m in rx.find_iter(&text) {
                count += 1;
                if preview.len() < 8 {
                    let ln = text[..m.start()].bytes().filter(|&b| b == b'\n').count() + 1;
                    let line_str = doc.lines.get(ln - 1).map(String::as_str).unwrap_or("");
                    preview.push(format!("  {rel}:{ln}#{}| {}", line_hash(line_str), line_str.trim()));
                }
            }
            if count == 0 {
                continue;
            }
            let rep_str = if use_crlf { new_lf.replace('\n', "\r\n") } else { new_lf.clone() };
            let out = rx.replace_all(&text, rep_str.as_str()).into_owned();
            (count, out, preview)
        } else {
            let needle = if use_crlf { old_lf.replace('\n', "\r\n") } else { old_lf.clone() };
            let repl = if use_crlf { new_lf.replace('\n', "\r\n") } else { new_lf.clone() };
            let mut count = 0usize;
            let mut preview = Vec::new();
            for (pos, _) in text.match_indices(&needle) {
                count += 1;
                if preview.len() < 8 {
                    let ln = text[..pos].bytes().filter(|&b| b == b'\n').count() + 1;
                    let line_str = doc.lines.get(ln - 1).map(String::as_str).unwrap_or("");
                    preview.push(format!("  {rel}:{ln}#{}| {}", line_hash(line_str), line_str.trim()));
                }
            }
            if count == 0 {
                if !single_explicit {
                    continue;
                }
                // One named file: tolerate indentation / trailing-whitespace drift (unique match
                // only), else show the nearest real lines so the retry needs no extra read.
                let old_lines: Vec<String> = old_lf.trim_end_matches('\n').split('\n').map(String::from).collect();
                let new_lines: Vec<String> = new_lf.trim_end_matches('\n').split('\n').map(String::from).collect();
                match seek_lines(&doc.lines, &old_lines, 0, false) {
                    Ok(Some((at, level, _))) if matches!(level, MatchLevel::Rstrip | MatchLevel::Trim) => {
                        let shift = indent_shift(&old_lines[0], &doc.lines[at]);
                        let shifted: Vec<String> = match &shift {
                            Some(s) => new_lines.iter().map(|l| apply_shift(l, s)).collect(),
                            None => new_lines,
                        };
                        let mut d = Doc::parse(&text);
                        d.lines.splice(at..at + old_lines.len(), shifted);
                        let line_str = doc.lines[at].as_str();
                        preview.push(format!("  {rel}:{}#{}| {}", at + 1, line_hash(line_str), line_str.trim()));
                        loose = true;
                        (1, d.to_text(), preview)
                    }
                    _ => {
                        miss_hint = closest_hint(&doc, &old_lines);
                        continue;
                    }
                }
            } else {
                let out = text.replace(&needle, &repl);
                (count, out, preview)
            }
        };

        if replaced == text {
            continue;
        }
        let new_doc = Doc::parse(&replaced);
        let mut anchors = Vec::new();
        for (i, line) in new_doc.lines.iter().enumerate() {
            if doc.lines.get(i).map(String::as_str) != Some(line.as_str()) {
                anchors.push(new_doc.anchor(i));
                if anchors.len() >= 6 {
                    break;
                }
            }
        }
        let lang = lang_for(&rel);
        let before_errs = syntax_errors(lang, &text);
        let mut syntax_note = None;
        if let (Some(before), Some(after)) = (before_errs, syntax_errors(lang, &replaced)) {
            if after.len() > before.len() {
                if let Some((row, m)) = after
                    .iter()
                    .find(|(r, _)| !before.iter().any(|(b, _)| b == r))
                    .or_else(|| after.first())
                {
                    let row = (*row).min(new_doc.lines.len().saturating_sub(1));
                    syntax_note = Some(super::doc::syntax_note(&new_doc, row, m, &format!("{rel}: new syntax error")));
                }
            }
        }
        total_count += count;
        plans.push(PlannedReplace {
            full,
            rel,
            original: bytes,
            new_text: replaced,
            count,
            anchors,
            preview_lines,
            syntax_note,
            loose,
        });
    }

    if let Some(exp) = opts.expected {
        if total_count != exp {
            return Err(format!(
                "expected {exp} occurrence(s), found {total_count} across {} file(s). Nothing was written.",
                plans.len()
            ));
        }
    }
    if plans.is_empty() {
        return Err(format!("no matches found for `{}`{miss_hint}", clip(old, 60)));
    }

    if opts.dry_run {
        let mut out = format!(
            "Would replace {total_count} occurrence(s) in {} file(s) (dry run, nothing written):\n",
            plans.len()
        );
        for p in plans.iter().take(40) {
            out.push_str(&format!("M {}  ({} hit{})\n", p.rel, p.count, if p.count == 1 { "" } else { "s" }));
            for l in &p.preview_lines {
                out.push_str(l);
                out.push('\n');
            }
        }
        if plans.len() > 40 {
            out.push_str(&format!("…[{} more files]\n", plans.len() - 40));
        }
        return Ok(out);
    }

    let mut done: Vec<&PlannedReplace> = Vec::new();
    for p in &plans {
        if let Err(e) = std::fs::write(&p.full, p.new_text.as_bytes()) {
            for prev in done.iter().rev() {
                let _ = std::fs::write(&prev.full, &prev.original);
            }
            return Err(format!("failed writing {} ({e}); rolled back all changes", p.rel));
        }
        done.push(p);
    }

    let mut out = format!("Replaced {total_count} occurrence(s) in {} file(s):\n", plans.len());
    for p in plans.iter().take(40) {
        let now = if p.anchors.is_empty() {
            String::new()
        } else {
            format!("  new anchors: {}", p.anchors.join(", "))
        };
        out.push_str(&format!(
            "M {}  ({} replacement{}{}){now}\n",
            p.rel,
            p.count,
            if p.count == 1 { "" } else { "s" },
            if p.loose { ", matched ignoring indentation/whitespace" } else { "" }
        ));
    }
    if plans.len() > 40 {
        out.push_str(&format!("…[{} more files]\n", plans.len() - 40));
    }
    for p in &plans {
        if let Some(ref note) = p.syntax_note {
            out.push_str(note);
            out.push('\n');
        }
    }
    Ok(out)
}
