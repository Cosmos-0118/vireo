//! Hashline-anchored file viewing with multi-range and multi-file batch support.

use std::path::Path;

use super::doc::{hl, read_doc_smart, Doc};
use super::symbols::{find_symbol, lang_for, qualified, symbols};

pub(crate) const MAX_VIEW_LINES: usize = 400;
pub(crate) const MAX_VIEW_BYTES: usize = 30_000;
/// One minified/generated line must not eat the whole budget (the hash stays that of the full line).
const MAX_LINE_CHARS: usize = 2_000;

pub(crate) fn show_line(doc: &Doc, i: usize) -> String {
    let line = hl(i + 1, &doc.lines[i]);
    let extra = line.chars().count().saturating_sub(MAX_LINE_CHARS);
    if extra == 0 {
        return line;
    }
    let mut s: String = line.chars().take(MAX_LINE_CHARS).collect();
    s.push_str(&format!(" …[+{extra} chars on this line]"));
    s
}

fn parse_line_range(t: &str) -> Option<(usize, Option<usize>)> {
    let re = lazy_re!(r"^(\d+)(?:#[0-9a-fA-F]{2})?(?:(?:\.\.|-)(\d+)?(?:#[0-9a-fA-F]{2})?)?$");
    let c = re.captures(t.trim())?;
    let a: usize = c[1].parse().ok()?;
    let b = c.get(2).and_then(|m| m.as_str().parse().ok());
    // `A` alone -> a single line; `A..` -> to the end.
    let single = !t.contains("..") && !t.contains('-');
    Some((a.max(1), if single { Some(a) } else { b }))
}

fn resolve_target(
    rel: &str,
    doc: &Doc,
    t: &str,
) -> Result<(usize, usize, String), String> {
    let n = doc.lines.len();
    if let Some((a, b)) = parse_line_range(t) {
        if a > n {
            return Err(format!("{rel} has only {n} line(s); line {a} is past the end"));
        }
        let a0 = (a - 1).min(n - 1);
        let b0 = b.map(|b| b.saturating_sub(1)).unwrap_or(n - 1).min(n - 1).max(a0);
        Ok((a0, b0, format!("lines {}..{} of {n}", a0 + 1, b0 + 1)))
    } else {
        let lang = lang_for(rel)
            .ok_or_else(|| format!("cannot resolve symbol `{t}` in {rel}: unsupported language"))?;
        let syms = symbols(lang, &doc.to_text());
        let s = find_symbol(&syms, t)?;
        Ok((s.start, s.end, format!("{} lines {}..{} of {n}", qualified(s), s.start + 1, s.end + 1)))
    }
}

pub fn view(ws: &Path, path: &str, target: Option<&str>, cwd: Option<&Path>) -> Result<String, String> {
    // `a.rs:Foo` / `a.rs:10..30` given as the path alone (MCP callers) behaves like the CLI spec.
    let target = target.map(str::trim).filter(|t| !t.is_empty());
    let (path, inline) = split_view_spec(path);
    let target = target.or(inline);
    let (_, rel, doc, note) = read_doc_smart(ws, path, cwd)?;
    let prefix = note.map(|n| format!("{n}\n")).unwrap_or_default();
    let n = doc.lines.len();
    if n == 0 {
        return Ok(format!("{prefix}# {rel} (empty)\n"));
    }
    let parts: Vec<&str> = match target {
        Some(t) if t.contains(',') => t.split(',').map(str::trim).filter(|p| !p.is_empty()).collect(),
        Some(t) => vec![t],
        None => Vec::new(),
    };
    if parts.len() <= 1 {
        let (from, to, label): (usize, usize, Option<String>) = match parts.first().copied() {
            None => (0, n - 1, None),
            Some(t) => {
                let (a0, b0, l) = resolve_target(&rel, &doc, t)?;
                (a0, b0, Some(l))
            }
        };
        let mut out = match &label {
            Some(l) => format!("{prefix}# {rel} {l}\n"),
            None => format!("{prefix}# {rel} ({n} lines)\n"),
        };
        let mut shown = 0usize;
        for i in from..=to {
            let line = show_line(&doc, i);
            if shown >= MAX_VIEW_LINES || (shown > 0 && out.len() + line.len() > MAX_VIEW_BYTES) {
                out.push_str(&format!(
                    "…[{} more lines; continue with {}:{}..{}]\n",
                    to + 1 - i,
                    rel,
                    i + 1,
                    to + 1
                ));
                break;
            }
            out.push_str(&line);
            out.push('\n');
            shown += 1;
        }
        return Ok(out);
    }

    // Multi-range / multi-symbol view within a single file.
    let mut sections: Vec<(usize, usize, String)> = Vec::with_capacity(parts.len());
    for p in parts {
        sections.push(resolve_target(&rel, &doc, p)?);
    }
    let mut out = format!("{prefix}# {rel} ({n} lines, {} sections)\n", sections.len());
    let mut shown = 0usize;
    for (si, (from, to, label)) in sections.iter().enumerate() {
        if si > 0 {
            out.push_str(&format!("-- {label} --\n"));
        } else {
            out.push_str(&format!("-- {label} --\n"));
        }
        for i in *from..=*to {
            let line = show_line(&doc, i);
            if shown >= MAX_VIEW_LINES || (shown > 0 && out.len() + line.len() > MAX_VIEW_BYTES) {
                let mut spec = format!("{}..{}", i + 1, to + 1);
                for (f, t, _) in &sections[si + 1..] {
                    spec.push_str(&format!(",{}..{}", f + 1, t + 1));
                }
                out.push_str(&format!("…[budget reached; continue with {rel}:{spec}]\n"));
                return Ok(out);
            }
            out.push_str(&line);
            out.push('\n');
            shown += 1;
        }
    }
    Ok(out)
}

/// Split a CLI `file[:target]` spec, preserving Windows drive letters (`C:\...`).
pub fn split_view_spec(spec: &str) -> (&str, Option<&str>) {
    let s = spec.trim();
    // Split at the FIRST colon so `a.rs:Foo:7` keeps `Foo:7` as the target, skipping a
    // Windows drive letter like `C:\foo` or `C:/foo`.
    let b = s.as_bytes();
    let from = if b.len() > 1 && b[1] == b':' && b[0].is_ascii_alphabetic() { 2 } else { 0 };
    let Some(idx) = s[from..].find(':').map(|i| i + from) else { return (s, None) };
    let after = &s[idx + 1..];
    if after.is_empty() || after.contains('/') || after.contains('\\') {
        return (s, None);
    }
    (&s[..idx], Some(after))
}

/// Batch view multiple `file[:target]` specs in one call (shared byte/line budget).
pub fn view_many(ws: &Path, specs: &[String], cwd: Option<&Path>) -> Result<String, String> {
    if specs.is_empty() {
        return Err("no files specified".into());
    }
    let mut out = String::new();
    for (idx, raw) in specs.iter().enumerate() {
        if out.len() > MAX_VIEW_BYTES {
            out.push_str(&format!("…[{} more file(s) omitted; view them individually]\n", specs.len() - idx));
            break;
        }
        let (p, t) = split_view_spec(raw);
        let chunk = view(ws, p, t, cwd)?;
        if idx > 0 && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&chunk);
    }
    Ok(out)
}
