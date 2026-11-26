//! V4A + hashline-anchored patch parser.

use super::check::clip;
use super::doc::parse_anchor;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum AnchorKind {
    Replace,
    Delete,
    After,
    Before,
}

/// File positions that need no anchor: `@@ top`, `@@ bottom`, `@@ after imports`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Edge {
    /// Line 1 (after a `#!` shebang).
    Top,
    /// After the last line.
    Bottom,
    /// After the last top-level import/use/include (falls back to `Top`); lines already present
    /// in the file are skipped, so re-adding an import is a no-op.
    Imports,
}

pub(crate) fn parse_edge(h: &str) -> Option<Edge> {
    match h.trim().to_ascii_lowercase().as_str() {
        "top" | "bof" | "top of file" => Some(Edge::Top),
        "bottom" | "eof" | "end of file" => Some(Edge::Bottom),
        "imports" | "after imports" => Some(Edge::Imports),
        _ => None,
    }
}

pub(crate) enum Hunk {
    Anchored {
        kind: AnchorKind,
        start: (usize, String),
        end: (usize, String),
        new: Vec<String>,
        header: String,
    },
    /// Header looked like an anchored hunk (`@@ 42#z9` or `@@ after 42`) but had bad anchor syntax.
    /// Deferred to `plan_update` so we can print real lines from the file near that line number.
    BadAnchor {
        raw: String,
        header: String,
    },
    /// `@@ top` / `@@ bottom` / `@@ after imports`: insert the `+` lines there.
    Edge {
        at: Edge,
        new: Vec<String>,
        header: String,
    },
    /// `@@ symbol Foo::bar` / `@@ after|before|delete symbol Foo`: a whole declaration, found by
    /// name in the pre-patch file, replaced (`+` lines), deleted, or used as an insertion point.
    Symbol {
        kind: AnchorKind,
        name: String,
        new: Vec<String>,
        header: String,
    },
    Context {
        ctx: Option<String>,
        body: Vec<(u8, String)>,
        eof: bool,
        header: String,
    },
}

pub(crate) enum FileOp {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, hunks: Vec<Hunk> },
}

fn strip_hashline_prefix(l: &str) -> &str {
    let re = lazy_re!(r"^(?:>\s*)?\d+#[0-9a-fA-F]{2}[|\-]");
    match re.find(l) {
        Some(m) => &l[m.end()..],
        None => l,
    }
}

fn patch_header<'a>(l: &'a str, tag: &str) -> Option<&'a str> {
    l.trim_start().strip_prefix(tag).map(str::trim).filter(|s| !s.is_empty())
}

fn is_file_header(l: &str) -> bool {
    let t = l.trim_start();
    t == "*** End Patch"
        || t.starts_with("*** Add File:")
        || t.starts_with("*** Delete File:")
        || t.starts_with("*** Update File:")
}

fn parse_anchored_header(h: &str) -> Option<(AnchorKind, (usize, String), (usize, String))> {
    let re = lazy_re!(
        r"^(?i:(after|before|replace|delete)\s+)?\[?(\d+(?:#[0-9A-Fa-f]{2})?)(?:\s*(?:\.\.|-)\s*(\d+(?:#[0-9A-Fa-f]{2})?))?\]?$"
    );
    let c = re.captures(h.trim())?;
    let kind = match c.get(1).map(|m| m.as_str().to_ascii_lowercase()).as_deref() {
        Some("after") => AnchorKind::After,
        Some("before") => AnchorKind::Before,
        Some("delete") => AnchorKind::Delete,
        _ => AnchorKind::Replace,
    };
    let s = parse_anchor(&c[2]).ok()?;
    let e = match c.get(3) {
        Some(m) => parse_anchor(m.as_str()).ok()?,
        None => s.clone(),
    };
    Some((kind, s, e))
}

/// `symbol Foo::bar`, `after symbol Foo`, `before symbol Foo`, `delete symbol Foo`.
fn parse_symbol_header(h: &str) -> Option<(AnchorKind, String)> {
    let re = lazy_re!(r"^(?i:(after|before|replace|delete)\s+)?(?i:symbol)\s+(\S.*)$");
    let c = re.captures(h.trim())?;
    let kind = match c.get(1).map(|m| m.as_str().to_ascii_lowercase()).as_deref() {
        Some("after") => AnchorKind::After,
        Some("before") => AnchorKind::Before,
        Some("delete") => AnchorKind::Delete,
        _ => AnchorKind::Replace,
    };
    Some((kind, c[2].trim().to_string()))
}
fn looks_like_anchored_header(h: &str) -> bool {
    let t = h.trim();
    let re = lazy_re!(r"^(?i:(after|before|replace|delete)\s+)?\d+(?:#\S*|\s*\.\.\s*\d+\S*)?$");
    if !re.is_match(t) {
        return false;
    }
    let low = t.to_ascii_lowercase();
    t.contains('#')
        || t.contains("..")
        || t.bytes().all(|b| b.is_ascii_digit())
        || low.starts_with("after ")
        || low.starts_with("before ")
        || low.starts_with("replace ")
        || low.starts_with("delete ")
}

pub(crate) fn parse_patch(text: &str) -> Result<Vec<FileOp>, String> {
    let t = text.replace("\r\n", "\n");
    let lines: Vec<&str> = t.split('\n').collect();
    let (mut i, had_begin) = if let Some(begin) = lines.iter().position(|l| l.trim() == "*** Begin Patch") {
        (begin + 1, true)
    } else if let Some(first_op) = lines.iter().position(|l| {
        let tr = l.trim_start();
        tr.starts_with("*** Update File:")
            || tr.starts_with("*** Add File:")
            || tr.starts_with("*** Delete File:")
    }) {
        (first_op, false)
    } else {
        return Err("not a patch: expected `*** Begin Patch` or `*** Update File:` (see `vx help apply`)".into());
    };

    let mut ops = Vec::new();
    let mut ended = false;
    while i < lines.len() {
        let l = lines[i];
        if l.trim() == "*** End Patch" {
            ended = true;
            break;
        }
        if let Some(p) = patch_header(l, "*** Add File:") {
            i += 1;
            let mut body: Vec<String> = Vec::new();
            let mut blanks = 0usize;
            // Forgiving form: when no line carries a `+`, the section is raw file content (the
            // easy mistake). Mixed sections stay an error so a stray line is never silently kept.
            let end = (i..lines.len()).find(|&k| is_file_header(lines[k])).unwrap_or(lines.len());
            let section = &lines[i..end];
            let literal = !section.iter().any(|b| b.starts_with('+')) && section.iter().any(|b| !b.trim().is_empty());
            while i < lines.len() && !is_file_header(lines[i]) {
                let b = lines[i];
                if literal && !b.trim().is_empty() {
                    body.extend(std::iter::repeat(String::new()).take(blanks));
                    blanks = 0;
                    body.push(b.to_string());
                } else if let Some(rest) = b.strip_prefix('+') {
                    body.extend(std::iter::repeat(String::new()).take(blanks));
                    blanks = 0;
                    body.push(strip_hashline_prefix(rest).to_string());
                } else if b.trim().is_empty() {
                    blanks += 1;
                } else {
                    return Err(format!(
                        "patch line {}: lines of `*** Add File: {p}` must all start with `+` (or none do, for raw content); got `{}`",
                        i + 1,
                        clip(b, 60)
                    ));
                }
                i += 1;
            }
            ops.push(FileOp::Add { path: p.to_string(), lines: body });
            continue;
        }
        if let Some(p) = patch_header(l, "*** Delete File:") {
            ops.push(FileOp::Delete { path: p.to_string() });
            i += 1;
            continue;
        }
        if let Some(p) = patch_header(l, "*** Update File:") {
            i += 1;
            let mut move_to = None;
            if i < lines.len() {
                if let Some(m) = patch_header(lines[i], "*** Move to:") {
                    move_to = Some(m.to_string());
                    i += 1;
                }
            }
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut pending_blanks = 0usize;
            // `-` lines tolerated inside anchored hunks: (hunk index, patch line). The anchors already
            // name the replaced lines, so a diff-style `-old` line is redundant, never content.
            let mut echoed_minus: Vec<(usize, usize)> = Vec::new();
            // Body lines that carry no `+` in anchored/symbol/edge hunks: (patch line, text) per hunk index.
            // When a hunk has no `+` line at all they are the replacement verbatim (prefix-free form).
            let mut raw: std::collections::BTreeMap<usize, Vec<(usize, String)>> = std::collections::BTreeMap::new();
            while i < lines.len() && !is_file_header(lines[i]) {
                let b = lines[i];
                let line_no = i + 1;
                i += 1;
                if let Some(h) = b.strip_prefix("@@") {
                    pending_blanks = 0;
                    let h = h.trim().trim_end_matches("@@").trim();
                    if let Some(at) = parse_edge(h) {
                        hunks.push(Hunk::Edge { at, new: Vec::new(), header: format!("@@ {h}") });
                    } else if let Some((kind, name)) = parse_symbol_header(h) {
                        hunks.push(Hunk::Symbol { kind, name, new: Vec::new(), header: format!("@@ {h}") });
                    } else if let Some((kind, start, end)) = parse_anchored_header(h) {
                        hunks.push(Hunk::Anchored {
                            kind,
                            start,
                            end,
                            new: Vec::new(),
                            header: format!("@@ {h}"),
                        });
                    } else if looks_like_anchored_header(h) {
                        hunks.push(Hunk::BadAnchor {
                            raw: h.to_string(),
                            header: format!("@@ {h}"),
                        });
                    } else {
                        let ctx = if h.is_empty() { None } else { Some(h.to_string()) };
                        hunks.push(Hunk::Context {
                            ctx,
                            body: Vec::new(),
                            eof: false,
                            header: format!("@@ {h}").trim_end().to_string(),
                        });
                    }
                    continue;
                }
                if b.trim_end() == "*** End of File" {
                    if let Some(Hunk::Context { eof, .. }) = hunks.last_mut() {
                        *eof = true;
                    }
                    continue;
                }
                if hunks.is_empty() {
                    hunks.push(Hunk::Context {
                        ctx: None,
                        body: Vec::new(),
                        eof: false,
                        header: "@@".into(),
                    });
                }
                let cur_idx = hunks.len() - 1;
                let Some(cur) = hunks.last_mut() else { continue };
                match cur {
                    Hunk::BadAnchor { .. } => {}
                    Hunk::Edge { new, .. } => {
                        if let Some(rest) = b.strip_prefix('+') {
                            new.extend(std::iter::repeat(String::new()).take(pending_blanks));
                            pending_blanks = 0;
                            new.push(strip_hashline_prefix(rest).to_string());
                        } else {
                            if b.trim().is_empty() {
                                pending_blanks += 1;
                            }
                            raw.entry(cur_idx).or_default().push((line_no, b.to_string()));
                        }
                    }
                    Hunk::Anchored { new, header, kind, .. } | Hunk::Symbol { new, header, kind, .. } => {
                        if let Some(rest) = b.strip_prefix('+') {
                            new.extend(std::iter::repeat(String::new()).take(pending_blanks));
                            pending_blanks = 0;
                            new.push(strip_hashline_prefix(rest).to_string());
                        } else if b.trim().is_empty() {
                            pending_blanks += 1;
                            raw.entry(cur_idx).or_default().push((line_no, String::new()));
                        } else if b.starts_with('-') {
                            // diff-style echo of a line being replaced/deleted: harmless, skip it
                            // (unless the whole hunk turns out to be prefix-free code, see `raw`)
                            echoed_minus.push((cur_idx, line_no));
                            raw.entry(cur_idx).or_default().push((line_no, b.to_string()));
                        } else if *kind == AnchorKind::Delete {
                            return Err(format!(
                                "patch line {line_no}: anchored hunk `{header}` takes no body lines; got `{}`",
                                clip(b, 60)
                            ));
                        } else {
                            raw.entry(cur_idx).or_default().push((line_no, b.to_string()));
                        }
                    }
                    Hunk::Context { body, .. } => {
                        if let Some(rest) = b.strip_prefix('+') {
                            body.push((b'+', strip_hashline_prefix(rest).to_string()));
                        } else if let Some(rest) = b.strip_prefix('-') {
                            body.push((b'-', strip_hashline_prefix(rest).to_string()));
                        } else if let Some(rest) = b.strip_prefix(' ') {
                            body.push((b' ', strip_hashline_prefix(rest).to_string()));
                        } else if b.trim().is_empty() {
                            body.push((b' ', String::new()));
                        } else {
                            return Err(format!(
                                "patch line {line_no}: hunk lines must start with ' ' (context), '-' or '+'; got `{}`",
                                clip(b, 60)
                            ));
                        }
                    }
                }
            }
            // Prefix-free hunks: with no `+` line at all, the body is the code to write, verbatim.
            // Mixing `+` lines with bare lines stays an error so a stray line is never silently kept.
            for (idx, entries) in &raw {
                let Some(h) = hunks.get_mut(*idx) else { continue };
                let (is_anchored, new, header) = match h {
                    Hunk::Anchored { new, header, .. } | Hunk::Symbol { new, header, .. } => (true, new, header.clone()),
                    Hunk::Edge { new, header, .. } => (false, new, header.clone()),
                    _ => continue,
                };
                let content: Vec<&(usize, String)> = entries.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
                if content.is_empty() {
                    continue;
                }
                if new.is_empty() && !content.iter().all(|(_, t)| t.starts_with('-')) {
                    let mut lines: Vec<String> = entries.iter().map(|(_, t)| strip_hashline_prefix(t).to_string()).collect();
                    while lines.last().is_some_and(|l| l.trim().is_empty()) {
                        lines.pop();
                    }
                    while lines.first().is_some_and(|l| l.trim().is_empty()) {
                        lines.remove(0);
                    }
                    *new = lines;
                    echoed_minus.retain(|(i, _)| i != idx);
                } else if let Some((line_no, t)) = content.iter().find(|(_, t)| !(is_anchored && t.starts_with('-'))) {
                    let what = if is_anchored {
                        "takes only `+` lines (every line to write must start with `+`, or none of them: then the body is taken verbatim; the anchors already say which lines change)"
                    } else {
                        "takes only `+` lines (the lines to insert), or none at all for verbatim text"
                    };
                    return Err(format!("patch line {line_no}: hunk `{header}` {what}; got `{}`", clip(t, 60)));
                }
            }
            // A replace/insert hunk made only of `-` lines would silently become a deletion (or a
            // no-op) when the author most likely forgot the `+` prefixes: refuse instead.
            for (idx, line_no) in &echoed_minus {
                if let Some(Hunk::Anchored { kind, new, header, .. } | Hunk::Symbol { kind, new, header, .. }) = hunks.get(*idx) {
                    if *kind != AnchorKind::Delete && new.is_empty() {
                        return Err(format!(
                            "patch line {line_no}: anchored hunk `{header}` has `-` lines but no `+` lines. Prefix every line to write with `+`; to remove the lines use `@@ delete A#h..B#h`"
                        ));
                    }
                }
            }
            if hunks.is_empty() && move_to.is_none() {
                return Err(format!("`*** Update File: {p}` has no hunks"));
            }
            ops.push(FileOp::Update { path: p.to_string(), move_to, hunks });
            continue;
        }
        if l.trim().is_empty() {
            i += 1;
            continue;
        }
        return Err(format!(
            "patch line {}: expected `*** Add File:`, `*** Update File:`, `*** Delete File:` or `*** End Patch`; got `{}`",
            i + 1,
            clip(l, 60)
        ));
    }
    if had_begin && !ended {
        return Err("patch has no `*** End Patch` line (was it cut off?). Nothing was changed.".into());
    }
    if ops.is_empty() {
        return Err("patch contains no file operations".into());
    }
    Ok(ops)
}
