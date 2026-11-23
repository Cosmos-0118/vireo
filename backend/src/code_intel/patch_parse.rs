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
    let re = lazy_re!(r"^\d+#[0-9a-fA-F]{2}\|");
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
        r"^(?i:(after|before|replace|delete)\s+)?(\d+#[0-9A-Fa-f]{2})(?:\s*(?:\.\.|-)\s*(\d+#[0-9A-Fa-f]{2}))?$"
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

fn looks_like_anchored_header(h: &str) -> bool {
    let t = h.trim();
    let re = lazy_re!(r"^(?i:(after|before|replace|delete)\s+)?\d+(?:#\S*|\s*\.\.\s*\d+\S*)?$");
    if !re.is_match(t) {
        return false;
    }
    let low = t.to_ascii_lowercase();
    t.contains('#')
        || t.contains("..")
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
            while i < lines.len() && !is_file_header(lines[i]) {
                let b = lines[i];
                let line_no = i + 1;
                i += 1;
                if let Some(h) = b.strip_prefix("@@") {
                    pending_blanks = 0;
                    let h = h.trim().trim_end_matches("@@").trim();
                    if let Some((kind, start, end)) = parse_anchored_header(h) {
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
                let Some(cur) = hunks.last_mut() else { continue };
                match cur {
                    Hunk::BadAnchor { .. } => {}
                    Hunk::Anchored { new, header, kind, .. } => {
                        if let Some(rest) = b.strip_prefix('+') {
                            new.extend(std::iter::repeat(String::new()).take(pending_blanks));
                            pending_blanks = 0;
                            new.push(strip_hashline_prefix(rest).to_string());
                        } else if b.trim().is_empty() {
                            pending_blanks += 1;
                        } else {
                            let what = if *kind == AnchorKind::Delete {
                                "takes no body lines"
                            } else {
                                "takes only `+` lines (the anchors already say which lines change)"
                            };
                            return Err(format!(
                                "patch line {line_no}: anchored hunk `{header}` {what}; got `{}`",
                                clip(b, 60)
                            ));
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
