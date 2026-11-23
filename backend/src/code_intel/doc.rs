//! Hashline document model, anchor parsing/verification, and smart file loading.

use std::path::{Path, PathBuf};
use xxhash_rust::xxh32::xxh32;

use super::util::{missing_path_err, rel_of, resolve, resolve_existing};

pub fn line_hash(s: &str) -> String {
    format!("{:02x}", xxh32(s.trim().as_bytes(), 0) & 0xff)
}

pub(crate) fn hl(n: usize, line: &str) -> String {
    format!("{}#{}|{}", n, line_hash(line), line)
}

pub(crate) struct Doc {
    pub lines: Vec<String>,
    pub ends: Vec<&'static str>,
    pub eol: &'static str,
    pub bom: bool,
}

impl Doc {
    pub(crate) fn parse(text: &str) -> Doc {
        let (bom, mut rest) = match text.strip_prefix('\u{feff}') {
            Some(r) => (true, r),
            None => (false, text),
        };
        let (mut lines, mut ends) = (Vec::new(), Vec::new());
        let (mut crlf, mut lf) = (0usize, 0usize);
        while !rest.is_empty() {
            match rest.find('\n') {
                Some(i) => {
                    let raw = &rest[..i];
                    if let Some(s) = raw.strip_suffix('\r') {
                        lines.push(s.to_string());
                        ends.push("\r\n");
                        crlf += 1;
                    } else {
                        lines.push(raw.to_string());
                        ends.push("\n");
                        lf += 1;
                    }
                    rest = &rest[i + 1..];
                }
                None => {
                    lines.push(rest.to_string());
                    ends.push("");
                    break;
                }
            }
        }
        Doc { lines, ends, eol: if crlf > lf { "\r\n" } else { "\n" }, bom }
    }

    pub(crate) fn to_text(&self) -> String {
        let mut s = String::new();
        if self.bom {
            s.push('\u{feff}');
        }
        for (l, e) in self.lines.iter().zip(&self.ends) {
            s.push_str(l);
            s.push_str(e);
        }
        s
    }

    pub(crate) fn anchor(&self, row0: usize) -> String {
        format!("{}#{}", row0 + 1, line_hash(&self.lines[row0]))
    }

    pub(crate) fn hashlines(&self, from0: usize, to0: usize) -> String {
        let mut out = String::new();
        for i in from0..=to0.min(self.lines.len().saturating_sub(1)) {
            if self.lines.is_empty() {
                break;
            }
            out.push_str(&hl(i + 1, &self.lines[i]));
            out.push('\n');
        }
        out
    }

    /// Snippet of ±`radius` lines around 1-based `line_no`, for instant error recovery.
    pub(crate) fn snippet_near(&self, line_no: usize, radius: usize) -> String {
        let n = self.lines.len();
        if n == 0 {
            return "(empty file)".to_string();
        }
        let center = line_no.saturating_sub(1).min(n - 1);
        let from = center.saturating_sub(radius);
        let to = (center + radius).min(n - 1);
        self.hashlines(from, to).trim_end().to_string()
    }
}

/// `⚠ {what} at N#hh: msg` plus the offending line (marked `>`) and its neighbours, so the agent
/// can fix it without another read. A tree-sitter MISSING token usually means an unclosed or extra
/// bracket/tag nearby, which the bare token name doesn't convey.
pub(crate) fn syntax_note(doc: &Doc, row: usize, msg: &str, what: &str) -> String {
    let n = doc.lines.len();
    if n == 0 {
        return format!("⚠ {what}: {msg}");
    }
    let row = row.min(n - 1);
    let hint = if msg.starts_with("missing ") {
        " (parser expected it here: look for an unclosed or extra bracket, tag or quote at or just above this line)"
    } else {
        ""
    };
    let mut out = format!("⚠ {what} at {}: {msg}{hint}", doc.anchor(row));
    for i in row.saturating_sub(1)..=(row + 1).min(n - 1) {
        out.push_str(&format!("\n{} {}", if i == row { ">" } else { " " }, hl(i + 1, &doc.lines[i])));
    }
    out
}

fn load_doc_at(ws: &Path, full: PathBuf) -> Result<(PathBuf, String, Doc), String> {
    let rel = rel_of(ws, &full);
    if full.is_dir() {
        return Err(format!("{rel} is a directory (list it with: vx map --dir {rel})"));
    }
    let bytes = std::fs::read(&full).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            missing_path_err(ws, &rel, true, false)
        } else {
            format!("cannot read {rel}: {e}")
        }
    })?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{rel} is not UTF-8 text"))?;
    Ok((full, rel, Doc::parse(&text)))
}

/// Strict file load (used by mutating operations): never silently redirects to another path,
/// but suggests exact matching workspace paths when not found.
pub(crate) fn read_doc(ws: &Path, path: &str) -> Result<(PathBuf, String, Doc), String> {
    let full = resolve(ws, path)?;
    load_doc_at(ws, full)
}

/// File load for reads and writes alike: resolves the exact path (cwd or ws) first; if missing,
/// auto-resolves a single unambiguous suffix/basename match in the workspace (returning a 1-line
/// note), so `view` and `edit` accept exactly the same paths.
pub(crate) fn read_doc_smart(
    ws: &Path,
    path: &str,
    cwd: Option<&Path>,
) -> Result<(PathBuf, String, Doc, Option<String>), String> {
    let (full, note) = resolve_existing(ws, cwd, path, true, false)?;
    let (f, rel, doc) = load_doc_at(ws, full)?;
    Ok((f, rel, doc, note))
}

pub(crate) fn parse_anchor(s: &str) -> Result<(usize, String), String> {
    let bad = || format!("bad anchor `{s}` (expected like 189#a4, copied from view/search output)");
    // Accept anchors as printed by any vx tool: `12#a4`, `12#a4|code`, `110#5e-code` (search context),
    // `> 12#a4|` (diagnostics), `[12#a4` / `12#a4]` (outline), `path/file.rs:12#a4` (search).
    let re = lazy_re!(r"^[>\[\s]*(?:[^\s:#]+:)?(\d+)#([0-9a-fA-F]{2})(?:$|[|\-\]\s.])");
    let c = re.captures(s.trim()).ok_or_else(bad)?;
    let n: usize = c[1].parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    Ok((n, c[2].to_lowercase()))
}

/// Parse an anchor, attaching live anchored lines near the typed line number if malformed so the
/// agent can fix typos in zero extra read turns.
pub(crate) fn parse_anchor_in_doc(doc: &Doc, rel: &str, raw: &str) -> Result<(usize, String), String> {
    match parse_anchor(raw) {
        Ok(a) => Ok(a),
        Err(msg) => {
            let digits: String = raw.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(line_no) = digits.parse::<usize>() {
                if line_no >= 1 && !doc.lines.is_empty() {
                    return Err(format!(
                        "{msg} in {rel}. Current lines near {line_no}:\n{}",
                        doc.snippet_near(line_no, 2)
                    ));
                }
            }
            Err(msg)
        }
    }
}

/// Find the 0-based inclusive row range for anchors, relocating within ±50 lines when unique.
/// On failure, always prints live lines near whichever anchor(s) mismatched (including `end` when
/// far from `start`).
pub(crate) fn locate(
    doc: &Doc,
    rel: &str,
    s: &(usize, String),
    e: &(usize, String),
) -> Result<(usize, usize), String> {
    if e.0 < s.0 {
        return Err(format!("end anchor {}#{} is before start anchor {}#{}", e.0, e.1, s.0, s.1));
    }
    let n = doc.lines.len();
    let span = e.0 - s.0;
    let ok = |start0: isize| -> bool {
        start0 >= 0
            && (start0 as usize + span) < n
            && line_hash(&doc.lines[start0 as usize]) == s.1
            && line_hash(&doc.lines[start0 as usize + span]) == e.1
    };
    let base = s.0 as isize - 1;
    if ok(base) {
        return Ok((base as usize, base as usize + span));
    }
    let mut cands = Vec::new();
    for d in 1..=50isize {
        for c in [base - d, base + d] {
            if ok(c) {
                cands.push(c);
            }
        }
    }
    if cands.len() == 1 {
        let c = cands[0] as usize;
        return Ok((c, c + span));
    }
    // Range length drifted (an earlier edit inside the block, or an off-by-one on `end`): when the
    // start line is found exactly once nearby and the end line exactly once within ±5 of where it
    // should be, the pair is unambiguous.
    if cands.is_empty() {
        let mut pairs = Vec::new();
        for st in (base - 50)..=(base + 50) {
            if st < 0 || st as usize >= n || line_hash(&doc.lines[st as usize]) != s.1 {
                continue;
            }
            let want = st + span as isize;
            for en in (want - 5).max(st)..=(want + 5) {
                if en >= 0 && (en as usize) < n && line_hash(&doc.lines[en as usize]) == e.1 {
                    pairs.push((st as usize, en as usize));
                }
            }
        }
        if pairs.len() == 1 {
            return Ok(pairs[0]);
        }
        // Several closing lines share the hash (`}` `}`): the true end sits at the start line's indent.
        let indent = |r: usize| doc.lines[r].len() - doc.lines[r].trim_start().len();
        let same: Vec<&(usize, usize)> = pairs.iter().filter(|(st, en)| indent(*st) == indent(*en)).collect();
        let starts: std::collections::HashSet<usize> = same.iter().map(|p| p.0).collect();
        if starts.len() == 1 {
            // Prefer the candidate closest to the expected span.
            if let Some(p) = same.iter().min_by_key(|p| (p.1 - p.0).abs_diff(span)) {
                let best = (p.1 - p.0).abs_diff(span);
                if same.iter().filter(|q| (q.1 - q.0).abs_diff(span) == best).count() == 1 {
                    return Ok(**p);
                }
            }
        }
    }
    // Far-away shifts (a big earlier edit): accept a pair that is unique across the whole file.
    if cands.is_empty() {
        let wide: Vec<isize> = (0..n as isize).filter(|&c| ok(c)).collect();
        if wide.len() == 1 {
            let c = wide[0] as usize;
            return Ok((c, c + span));
        }
    }
    let why = if cands.is_empty() { "no longer match" } else { "match several places" };
    let s_ok = s.0 >= 1 && s.0 <= n && line_hash(&doc.lines[s.0 - 1]) == s.1;
    let e_ok = e.0 >= 1 && e.0 <= n && line_hash(&doc.lines[e.0 - 1]) == e.1;
    let mut detail = if s_ok && !e_ok {
        format!(
            "end anchor {}#{} did not match in {} (start {}#{} matched). Current lines near {}:\n{}",
            e.0,
            e.1,
            rel,
            s.0,
            s.1,
            e.0,
            doc.snippet_near(e.0, 2)
        )
    } else if !s_ok && e_ok {
        format!(
            "start anchor {}#{} did not match in {} (end {}#{} matched). Current lines near {}:\n{}",
            s.0,
            s.1,
            rel,
            e.0,
            e.1,
            s.0,
            doc.snippet_near(s.0, 2)
        )
    } else {
        let from = base.max(0) as usize;
        let from = if n == 0 { 0 } else { from.min(n - 1).saturating_sub(2) };
        let to = (from + span.min(10) + 4).min(n.saturating_sub(1));
        let mut msg = format!(
            "anchors {}#{}..{}#{} {} in {} (file changed since you viewed it). Current lines near {}:\n{}",
            s.0,
            s.1,
            e.0,
            e.1,
            why,
            rel,
            s.0,
            doc.hashlines(from, to).trim_end()
        );
        if e.0 > to + 1 && n > 0 {
            msg.push_str(&format!("\nCurrent lines near {}:\n{}", e.0, doc.snippet_near(e.0, 2)));
        }
        msg
    };
    if n == 0 {
        detail = format!("anchors {}#{}..{}#{} {} in {} (file is empty)", s.0, s.1, e.0, e.1, why, rel);
    }
    // Say where a moved line went, so one retry with the right anchor is enough.
    let moved = |label: &str, a: &(usize, String)| -> String {
        let at: Vec<String> = (0..n)
            .filter(|&i| i + 1 != a.0 && line_hash(&doc.lines[i]) == a.1)
            .take(3)
            .map(|i| doc.anchor(i))
            .collect();
        if at.is_empty() { String::new() } else { format!("\n{label} hash #{} also appears at: {}", a.1, at.join(", ")) }
    };
    if n > 0 {
        if !s_ok {
            detail.push_str(&moved("start", s));
        }
        if !e_ok && e != s {
            detail.push_str(&moved("end", e));
        }
    }
    Err(detail)
}

/// Normalise model-provided replacement text into lines.
pub(crate) fn text_to_lines(new_text: &str) -> Vec<String> {
    let t = new_text.replace("\r\n", "\n");
    if t.is_empty() {
        return Vec::new();
    }
    let t = t.strip_suffix('\n').unwrap_or(&t);
    let mut lines: Vec<String> = t.split('\n').map(str::to_string).collect();
    // Models sometimes echo the `N#hh|` prefixes back; a real code line never starts like that.
    let re = lazy_re!(r"^(?:>\s*)?\d+#[0-9a-fA-F]{2}[|\-]");
    for l in lines.iter_mut() {
        if let Some(m) = re.find(l) {
            *l = l[m.end()..].to_string();
        }
    }
    lines
}

pub(crate) fn splice(doc: &mut Doc, at: usize, remove: usize, new: Vec<String>) {
    let mut keep_no_eol =
        remove > 0 && at + remove == doc.lines.len() && doc.ends[at + remove - 1].is_empty();
    if remove == 0 && at == doc.lines.len() && at > 0 && doc.ends[at - 1].is_empty() {
        doc.ends[at - 1] = doc.eol;
        keep_no_eol = true;
    }
    let k = new.len();
    let eol = doc.eol;
    let new_ends: Vec<&'static str> =
        (0..k).map(|i| if keep_no_eol && i + 1 == k { "" } else { eol }).collect();
    doc.lines.splice(at..at + remove, new);
    doc.ends.splice(at..at + remove, new_ends);
}
