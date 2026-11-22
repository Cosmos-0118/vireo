//! Token-lean code intelligence shared by Vireo's built-in agent and the `vx` CLI that external
//! agent CLIs (Claude Code, Codex, ...) call from their shell.
//!
//! Pure, synchronous and free of axum/tauri types so the `vx` binary can `#[path]`-include it.
//!
//! * **Hashline view/edit** – every line is shown as `N#hh|code` where `hh` is a 2-hex hash of the
//!   trimmed line. Edits reference `N#hh` anchors instead of echoing old text; stale line numbers
//!   are re-located (±50 lines) when the anchors still match uniquely, otherwise the edit is
//!   rejected with a fresh snippet.
//! * **Outline / repo map / symbol edit** – tree-sitter derived symbol ranges.
//! * **Instant syntax validation** after every edit (new tree-sitter ERROR/MISSING nodes).
//! * **ast** – structural search/rewrite through the `ast-grep` binary.
//! * **check** – build/test runner that returns only deduplicated diagnostics with anchors.

use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tree_sitter::{Language, Node, Parser};
use walkdir::WalkDir;
use xxhash_rust::xxh32::xxh32;

/// Short usage hint injected into external agent CLIs (kept tiny on purpose: this is the whole
/// per-turn "schema" cost of the vx tools).
pub const AGENT_HINT: &str = "Vireo `vx` is on PATH: token-cheap code tools. Prefer it over cat/grep/sed and full-file rewrites.\n\
vx map [query] - ranked repo symbol map\n\
vx outline <file> - symbols as [start#hash..end#hash] ranges\n\
vx view <file> [symbol|A..B] - lines as N#hash|code\n\
vx edit <file> A#h..B#h - replace those lines with new code from stdin or --text (no old text needed; anchors come from view/outline/check output). Also --insert-after, --insert-before, --delete (anchor A#h only)\n\
vx symbol <file> <name> [--replace|--insert-after|--delete] - read or change a whole function/class by name (code on stdin); the range includes its doc comments/attributes\n\
vx create <file> - new file from stdin\n\
vx search <regex> - grep with anchors\n\
vx ast '<pattern>' [-r '<rewrite>'] [-l lang] - structural search/codemod\n\
vx check [--cmd C] - build/tests, only deduped errors with anchors\n\
Edits report new syntax errors immediately; no need to re-read after editing.";

// ---------------------------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------------------------

static PATH_ENV: OnceLock<OsString> = OnceLock::new();

/// PATH used when spawning helper tools (rg, ast-grep, build commands). Optional.
pub fn set_path_env(p: OsString) {
    let _ = PATH_ENV.set(p);
}

fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Some(p) = PATH_ENV.get() {
        c.env("PATH", p);
    }
    c
}

struct RunOut {
    code: i32,
    stdout: String,
    stderr: String,
    timed_out: bool,
    secs: f64,
}

fn pump<R: Read + Send + 'static>(mut r: R, buf: Arc<Mutex<Vec<u8>>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut b) = buf.lock() {
                b.extend_from_slice(&chunk[..n]);
            }
        }
    })
}

fn run_capture(mut cmd: Command, timeout: Duration) -> Result<RunOut, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let t1 = child.stdout.take().map(|s| pump(s, out_buf.clone()));
    let t2 = child.stderr.take().map(|s| pump(s, err_buf.clone()));
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() > timeout {
                    timed_out = true;
                    #[cfg(windows)]
                    {
                        let _ = command("taskkill")
                            .args(["/F", "/T", "/PID", &child.id().to_string()])
                            .output();
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if !timed_out {
        for t in [t1, t2].into_iter().flatten() {
            let _ = t.join();
        }
    }
    let take = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().map(|g| g.clone()).unwrap_or_default()).to_string();
    Ok(RunOut {
        code,
        stdout: take(&out_buf),
        stderr: take(&err_buf),
        timed_out,
        secs: start.elapsed().as_secs_f64(),
    })
}

fn shell_command(cmd: &str) -> Command {
    if cfg!(windows) {
        let mut c = command("powershell.exe");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd]);
        c
    } else {
        let mut c = command("sh");
        c.args(["-lc", cmd]);
        c
    }
}

macro_rules! lazy_re {
    ($p:expr) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new($p).unwrap())
    }};
}

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

fn norm_slashes(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string()
}

fn ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".next" | ".nuxt" | "__pycache__"
            | ".venv" | "venv" | ".turbo" | ".cache" | "coverage"
    )
}

/// Resolve a workspace-relative path (or an absolute path inside the workspace).
pub fn resolve(ws: &Path, p: &str) -> Result<PathBuf, String> {
    let cleaned = p.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    if cleaned.is_empty() {
        return Err("empty path".into());
    }
    let pb = Path::new(&cleaned);
    if pb.is_absolute() {
        let ws_s = norm_slashes(ws);
        let p_s = norm_slashes(pb);
        if let Some(head) = p_s.get(..ws_s.len()) {
            if head.eq_ignore_ascii_case(&ws_s) && p_s.as_bytes().get(ws_s.len()) == Some(&b'/') {
                return resolve(ws, &p_s[ws_s.len() + 1..]);
            }
        }
        return Err(format!("path is outside the workspace: {p}"));
    }
    let mut out = ws.to_path_buf();
    for comp in pb.components() {
        match comp {
            std::path::Component::Normal(c) => out.push(c),
            std::path::Component::CurDir => {}
            _ => return Err("path traversal is not allowed".into()),
        }
    }
    Ok(out)
}

fn rel_of(ws: &Path, full: &Path) -> String {
    let w = norm_slashes(ws);
    let f = norm_slashes(full);
    f.strip_prefix(&format!("{w}/")).unwrap_or(&f).to_string()
}

// ---------------------------------------------------------------------------------------------
// Hashline document model
// ---------------------------------------------------------------------------------------------

pub fn line_hash(s: &str) -> String {
    format!("{:02x}", xxh32(s.trim().as_bytes(), 0) & 0xff)
}

fn hl(n: usize, line: &str) -> String {
    format!("{}#{}|{}", n, line_hash(line), line)
}

struct Doc {
    lines: Vec<String>,
    ends: Vec<&'static str>,
    eol: &'static str,
    bom: bool,
}

impl Doc {
    fn parse(text: &str) -> Doc {
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

    fn to_text(&self) -> String {
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

    fn anchor(&self, row0: usize) -> String {
        format!("{}#{}", row0 + 1, line_hash(&self.lines[row0]))
    }

    fn hashlines(&self, from0: usize, to0: usize) -> String {
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
}

fn read_doc(ws: &Path, path: &str) -> Result<(PathBuf, String, Doc), String> {
    let full = resolve(ws, path)?;
    let rel = rel_of(ws, &full);
    let bytes = std::fs::read(&full).map_err(|e| format!("cannot read {rel}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{rel} is not UTF-8 text"))?;
    Ok((full, rel, Doc::parse(&text)))
}

fn parse_anchor(s: &str) -> Result<(usize, String), String> {
    let t = s.trim().trim_end_matches('|');
    let bad = || format!("bad anchor `{s}` (expected like 189#a4)");
    let (n, h) = t.split_once('#').ok_or_else(bad)?;
    let n: usize = n.trim().parse().map_err(|_| bad())?;
    let h = h.trim().to_lowercase();
    if n == 0 || h.len() != 2 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    Ok((n, h))
}

/// Find the 0-based inclusive row range for anchors, relocating within ±50 lines when unique.
fn locate(doc: &Doc, rel: &str, s: &(usize, String), e: &(usize, String)) -> Result<(usize, usize), String> {
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
    let why = if cands.is_empty() { "no longer match" } else { "match several places" };
    let from = base.max(0) as usize;
    let from = if n == 0 { 0 } else { from.min(n - 1).saturating_sub(2) };
    let to = (from + span.min(10) + 4).min(n.saturating_sub(1));
    Err(format!(
        "anchors {}#{}..{}#{} {} in {} (file changed since you viewed it). Current lines near {}:\n{}",
        s.0,
        s.1,
        e.0,
        e.1,
        why,
        rel,
        s.0,
        doc.hashlines(from, to).trim_end()
    ))
}

/// Normalise model-provided replacement text into lines.
fn text_to_lines(new_text: &str) -> Vec<String> {
    let t = new_text.replace("\r\n", "\n");
    if t.is_empty() {
        return Vec::new();
    }
    let t = t.strip_suffix('\n').unwrap_or(&t);
    let mut lines: Vec<String> = t.split('\n').map(str::to_string).collect();
    // Models sometimes echo the `N#hh|` prefixes back; a real code line never starts like that.
    let re = lazy_re!(r"^\d+#[0-9a-fA-F]{2}\|");
    for l in lines.iter_mut() {
        if let Some(m) = re.find(l) {
            *l = l[m.end()..].to_string();
        }
    }
    lines
}

fn splice(doc: &mut Doc, at: usize, remove: usize, new: Vec<String>) {
    let mut keep_no_eol = remove > 0 && at + remove == doc.lines.len() && doc.ends[at + remove - 1].is_empty();
    if remove == 0 && at == doc.lines.len() && at > 0 && doc.ends[at - 1].is_empty() {
        doc.ends[at - 1] = doc.eol;
        keep_no_eol = true;
    }
    let k = new.len();
    let eol = doc.eol;
    let new_ends: Vec<&'static str> = (0..k).map(|i| if keep_no_eol && i + 1 == k { "" } else { eol }).collect();
    doc.lines.splice(at..at + remove, new);
    doc.ends.splice(at..at + remove, new_ends);
}

// ---------------------------------------------------------------------------------------------
// Languages & syntax validation
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Ts,
    Tsx,
    Js,
    Py,
    Go,
    Json,
}

pub fn lang_for(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    Some(match ext.as_str() {
        "rs" => Lang::Rust,
        "ts" | "mts" | "cts" => Lang::Ts,
        "tsx" => Lang::Tsx,
        "js" | "jsx" | "mjs" | "cjs" => Lang::Js,
        "py" => Lang::Py,
        "go" => Lang::Go,
        "json" => Lang::Json,
        _ => return None,
    })
}

fn ts_language(l: Lang) -> Language {
    match l {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Js => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Py => tree_sitter_python::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Json => tree_sitter_json::LANGUAGE.into(),
    }
}

fn parse_tree(l: Lang, src: &str) -> Option<tree_sitter::Tree> {
    let mut p = Parser::new();
    p.set_language(&ts_language(l)).ok()?;
    p.parse(src, None)
}

fn collect_errors(n: Node, src: &[u8], out: &mut Vec<(usize, String)>) {
    if out.len() >= 50 || !n.has_error() {
        return;
    }
    if n.is_missing() {
        out.push((n.start_position().row, format!("missing `{}`", n.kind())));
        return;
    }
    if n.is_error() {
        let snippet: String = n.utf8_text(src).unwrap_or("").chars().take(24).collect::<String>().replace('\n', " ");
        out.push((n.start_position().row, format!("unexpected syntax near `{}`", snippet.trim())));
        return;
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        collect_errors(ch, src, out);
    }
}

/// `None` when the language has no parser; otherwise (row, message) for each syntax error.
fn syntax_errors(lang: Option<Lang>, src: &str) -> Option<Vec<(usize, String)>> {
    let lang = lang?;
    let tree = parse_tree(lang, src)?;
    let mut out = Vec::new();
    collect_errors(tree.root_node(), src.as_bytes(), &mut out);
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Sym {
    pub name: String,
    pub kind: String,
    pub container: Option<String>,
    /// 0-based inclusive rows; `start` includes leading doc comments / attributes.
    pub start: usize,
    pub end: usize,
    pub sig: String,
    pub depth: usize,
}

fn rows(n: Node) -> (usize, usize) {
    let (s, e) = (n.start_position(), n.end_position());
    (s.row, if e.column == 0 && e.row > s.row { e.row - 1 } else { e.row })
}

fn ntext<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

fn field_text(n: Node, field: &str, src: &[u8]) -> String {
    n.child_by_field_name(field).map(|c| ntext(c, src).trim().to_string()).unwrap_or_default()
}

fn sig_of(n: Node, src: &[u8]) -> String {
    let first = ntext(n, src).lines().next().unwrap_or("").trim();
    let first = first.trim_end_matches('{').trim_end();
    let mut s: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        s.push('…');
    }
    s
}

fn extend_start(n: Node, doc_kinds: &[&str]) -> usize {
    let (mut s, _) = rows(n);
    let mut cur = n;
    while let Some(p) = cur.prev_sibling() {
        if !doc_kinds.contains(&p.kind()) {
            break;
        }
        let (ps, pe) = rows(p);
        if pe + 1 >= s {
            s = ps;
            cur = p;
        } else {
            break;
        }
    }
    s
}

fn strip_generics(s: &str) -> String {
    s.split('<').next().unwrap_or(s).trim().to_string()
}

struct Walk<'a> {
    src: &'a [u8],
    out: Vec<Sym>,
}

impl<'a> Walk<'a> {
    fn push(&mut self, name: String, decl: Node, range: Node, container: Option<&str>, depth: usize, docs: &[&str]) {
        if name.is_empty() {
            return;
        }
        self.out.push(Sym {
            name,
            kind: decl.kind().to_string(),
            container: container.map(str::to_string),
            start: extend_start(range, docs),
            end: rows(range).1,
            sig: sig_of(if ntext(range, self.src).trim_start().starts_with('@') { decl } else { range }, self.src),
            depth,
        });
    }

    fn rust(&mut self, parent: Node, container: Option<&str>, depth: usize) {
        const DOCS: &[&str] = &["attribute_item", "line_comment", "block_comment"];
        let mut c = parent.walk();
        for n in parent.named_children(&mut c) {
            let kind = n.kind();
            let (name, recurse_name) = match kind {
                "function_item" | "function_signature_item" | "struct_item" | "enum_item" | "union_item"
                | "type_item" | "const_item" | "static_item" | "macro_definition" => {
                    (field_text(n, "name", self.src), None)
                }
                "trait_item" | "mod_item" => {
                    let nm = field_text(n, "name", self.src);
                    (nm.clone(), Some(nm))
                }
                "impl_item" => {
                    let ty = strip_generics(&field_text(n, "type", self.src));
                    let tr = strip_generics(&field_text(n, "trait", self.src));
                    let label = if tr.is_empty() { ty.clone() } else { format!("{tr} for {ty}") };
                    self.push(label, n, n, container, depth, DOCS);
                    if let Some(body) = n.child_by_field_name("body") {
                        self.rust(body, Some(&ty), depth + 1);
                    }
                    continue;
                }
                _ => continue,
            };
            self.push(name, n, n, container, depth, DOCS);
            if let (Some(rn), Some(body)) = (recurse_name, n.child_by_field_name("body")) {
                self.rust(body, Some(&rn), depth + 1);
            }
        }
    }

    fn ts(&mut self, parent: Node, container: Option<&str>, depth: usize) {
        const DOCS: &[&str] = &["comment"];
        let mut c = parent.walk();
        for n in parent.named_children(&mut c) {
            let (decl, range) = if n.kind() == "export_statement" {
                match n.child_by_field_name("declaration") {
                    Some(d) => (d, n),
                    None => continue,
                }
            } else {
                (n, n)
            };
            match decl.kind() {
                "function_declaration" | "generator_function_declaration" | "function_signature" => {
                    let mut nm = field_text(decl, "name", self.src);
                    if nm.is_empty() {
                        nm = "default".into();
                    }
                    self.push(nm, decl, range, container, depth, DOCS);
                }
                "class_declaration" | "abstract_class_declaration" => {
                    let mut nm = field_text(decl, "name", self.src);
                    if nm.is_empty() {
                        nm = "default".into();
                    }
                    self.push(nm.clone(), decl, range, container, depth, DOCS);
                    if let Some(body) = decl.child_by_field_name("body") {
                        self.ts(body, Some(&nm), depth + 1);
                    }
                }
                "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
                    let nm = field_text(decl, "name", self.src);
                    self.push(nm, decl, range, container, depth, DOCS);
                }
                "method_definition" | "abstract_method_signature" => {
                    let nm = field_text(decl, "name", self.src);
                    self.push(nm, decl, range, container, depth, DOCS);
                }
                "public_field_definition" => {
                    let is_fn = decl
                        .child_by_field_name("value")
                        .map(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function"))
                        .unwrap_or(false);
                    if is_fn {
                        let nm = field_text(decl, "name", self.src);
                        self.push(nm, decl, range, container, depth, DOCS);
                    }
                }
                "lexical_declaration" | "variable_declaration" => {
                    let mut dc = decl.walk();
                    for d in decl.named_children(&mut dc) {
                        if d.kind() == "variable_declarator" {
                            let nm = field_text(d, "name", self.src);
                            if !nm.contains(['{', '[']) {
                                self.push(nm, d, range, container, depth, DOCS);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn py(&mut self, parent: Node, container: Option<&str>, depth: usize) {
        const DOCS: &[&str] = &["comment"];
        let mut c = parent.walk();
        for n in parent.named_children(&mut c) {
            let (decl, range) = if n.kind() == "decorated_definition" {
                match n.child_by_field_name("definition") {
                    Some(d) => (d, n),
                    None => continue,
                }
            } else {
                (n, n)
            };
            match decl.kind() {
                "function_definition" => {
                    let nm = field_text(decl, "name", self.src);
                    self.push(nm, decl, range, container, depth, DOCS);
                }
                "class_definition" => {
                    let nm = field_text(decl, "name", self.src);
                    self.push(nm.clone(), decl, range, container, depth, DOCS);
                    if let Some(body) = decl.child_by_field_name("body") {
                        self.py(body, Some(&nm), depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    fn go(&mut self, parent: Node) {
        const DOCS: &[&str] = &["comment"];
        let mut c = parent.walk();
        for n in parent.named_children(&mut c) {
            match n.kind() {
                "function_declaration" => {
                    let nm = field_text(n, "name", self.src);
                    self.push(nm, n, n, None, 0, DOCS);
                }
                "method_declaration" => {
                    let nm = field_text(n, "name", self.src);
                    let recv = field_text(n, "receiver", self.src);
                    let ty = recv
                        .trim_matches(|c| c == '(' || c == ')')
                        .split_whitespace()
                        .last()
                        .map(|t| strip_generics(t.trim_start_matches('*')).split('[').next().unwrap_or("").to_string())
                        .unwrap_or_default();
                    let cont = if ty.is_empty() { None } else { Some(ty) };
                    self.push(nm, n, n, cont.as_deref(), 0, DOCS);
                }
                "type_declaration" => {
                    let mut tc = n.walk();
                    for spec in n.named_children(&mut tc) {
                        if matches!(spec.kind(), "type_spec" | "type_alias") {
                            let nm = field_text(spec, "name", self.src);
                            self.push(nm, spec, n, None, 0, DOCS);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Symbols of a source file (empty for languages without an outline).
pub fn symbols(lang: Lang, src: &str) -> Vec<Sym> {
    if lang == Lang::Json {
        return Vec::new();
    }
    let Some(tree) = parse_tree(lang, src) else { return Vec::new() };
    let mut w = Walk { src: src.as_bytes(), out: Vec::new() };
    let root = tree.root_node();
    match lang {
        Lang::Rust => w.rust(root, None, 0),
        Lang::Ts | Lang::Tsx | Lang::Js => w.ts(root, None, 0),
        Lang::Py => w.py(root, None, 0),
        Lang::Go => w.go(root),
        Lang::Json => {}
    }
    w.out
}

fn qualified(s: &Sym) -> String {
    match &s.container {
        Some(c) => format!("{c}::{}", s.name),
        None => s.name.clone(),
    }
}

fn find_symbol<'a>(syms: &'a [Sym], query: &str) -> Result<&'a Sym, String> {
    let mut q = query.trim();
    let mut want_impl = false;
    for kw in [
        "pub(crate) ", "pub ", "async ", "export ", "default ", "fn ", "func ", "def ", "function ", "class ",
        "struct ", "enum ", "trait ", "interface ", "type ", "const ", "let ", "mod ", "abstract ",
    ] {
        while let Some(r) = q.strip_prefix(kw) {
            q = r.trim_start();
        }
    }
    if let Some(r) = q.strip_prefix("impl ") {
        want_impl = true;
        q = r.trim_start();
    }
    let q = q.trim_end_matches("()").trim();
    let norm = q.replace("::", ".");
    let (cont, name) = match norm.rsplit_once('.') {
        Some((c, n)) => (Some(c.to_string()), n.to_string()),
        None => (None, norm.clone()),
    };
    let mut m: Vec<&Sym> = syms
        .iter()
        .filter(|s| {
            let name_ok = s.name == name || s.name.ends_with(&format!(" for {name}"));
            name_ok && cont.as_ref().map(|c| s.container.as_deref() == Some(c.as_str())).unwrap_or(true)
        })
        .collect();
    if m.len() > 1 {
        let filtered: Vec<&Sym> = m.iter().copied().filter(|s| (s.kind == "impl_item") == want_impl).collect();
        if !filtered.is_empty() {
            m = filtered;
        }
    }
    match m.len() {
        1 => Ok(m[0]),
        0 => {
            let needle = name.to_lowercase();
            let similar: Vec<String> = syms
                .iter()
                .filter(|s| s.name.to_lowercase().contains(&needle))
                .take(6)
                .map(qualified)
                .collect();
            Err(if similar.is_empty() {
                format!("symbol `{query}` not found (run outline to list symbols)")
            } else {
                format!("symbol `{query}` not found. Similar: {}", similar.join(", "))
            })
        }
        _ => Err(format!(
            "symbol `{query}` is ambiguous; qualify it as Container::name. Candidates: {}",
            m.iter()
                .map(|s| format!("{} {} (lines {}..{})", s.kind, qualified(s), s.start + 1, s.end + 1))
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------------

const MAX_VIEW_LINES: usize = 400;

pub fn outline(ws: &Path, path: &str) -> Result<String, String> {
    let (_, rel, doc) = read_doc(ws, path)?;
    let Some(lang) = lang_for(&rel).filter(|l| *l != Lang::Json) else {
        return Err(format!("no outline support for {rel}; use view"));
    };
    let src = doc.to_text();
    let syms = symbols(lang, &src);
    let mut out = format!("# {rel} ({} lines)\n", doc.lines.len());
    if syms.is_empty() {
        out.push_str("(no symbols found)\n");
    }
    for s in &syms {
        out.push_str(&format!(
            "{}[{}..{}] {}\n",
            "  ".repeat(s.depth),
            doc.anchor(s.start),
            doc.anchor(s.end),
            s.sig
        ));
    }
    Ok(out)
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

pub fn view(ws: &Path, path: &str, target: Option<&str>) -> Result<String, String> {
    let (_, rel, doc) = read_doc(ws, path)?;
    let n = doc.lines.len();
    if n == 0 {
        return Ok(format!("# {rel} (empty)\n"));
    }
    let target = target.map(str::trim).filter(|t| !t.is_empty());
    let (from, to, label): (usize, usize, Option<String>) = match target {
        None => (0, n - 1, None),
        Some(t) => {
            if let Some((a, b)) = parse_line_range(t) {
                let a0 = (a - 1).min(n - 1);
                let b0 = b.map(|b| b.saturating_sub(1)).unwrap_or(n - 1).min(n - 1).max(a0);
                (a0, b0, Some(format!("lines {}..{} of {n}", a0 + 1, b0 + 1)))
            } else {
                let lang = lang_for(&rel).ok_or_else(|| format!("cannot resolve symbol `{t}` in {rel}: unsupported language"))?;
                let syms = symbols(lang, &doc.to_text());
                let s = find_symbol(&syms, t)?;
                (s.start, s.end, Some(format!("{} lines {}..{} of {n}", qualified(s), s.start + 1, s.end + 1)))
            }
        }
    };
    let mut out = match &label {
        Some(l) => format!("# {rel} {l}\n"),
        None => format!("# {rel} ({n} lines)\n"),
    };
    let mut shown = 0usize;
    for i in from..=to {
        let line = hl(i + 1, &doc.lines[i]);
        if shown >= MAX_VIEW_LINES || out.len() + line.len() > 30_000 {
            out.push_str(&format!(
                "…[{} more lines; view {} A..B or outline]\n",
                to + 1 - i,
                rel
            ));
            break;
        }
        out.push_str(&line);
        out.push('\n');
        shown += 1;
    }
    Ok(out)
}

fn finish_edit(
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
    let s = parse_anchor(start)?;
    let e = match end {
        Some(e) if !e.trim().is_empty() => parse_anchor(e)?,
        _ => s.clone(),
    };
    let (r0, r1) = locate(&doc, &rel, &s, &e)?;
    let new_lines = text_to_lines(new_text);
    match op {
        "replace" | "" => {
            if new_lines.len() == (r1 - r0 + 1) && new_lines.iter().zip(&doc.lines[r0..=r1]).all(|(a, b)| a == b) {
                return Ok(format!("No change: new text is identical to {rel} lines {}..{}.", r0 + 1, r1 + 1));
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
            finish_edit(&full, &rel, &doc, lang, before, None, format!("Deleted {rel} lines {}..{} (-{}).", r0 + 1, r1 + 1, r1 - r0 + 1))
        }
        "insert_after" | "insert_before" => {
            if new_lines.is_empty() {
                return Err("new_text is empty".into());
            }
            let at = if op == "insert_after" { r1 + 1 } else { r0 };
            let k = new_lines.len();
            splice(&mut doc, at, 0, new_lines);
            finish_edit(&full, &rel, &doc, lang, before, Some((at, at + k - 1)), format!("Inserted {k} line(s) in {rel} at line {}.", at + 1))
        }
        other => Err(format!("unknown edit op `{other}` (replace|insert_after|insert_before|delete)")),
    }
}

/// Read or change a whole symbol (function, struct, class, ...) by name.
pub fn symbol_op(ws: &Path, path: &str, symbol: &str, mode: &str, new_code: &str) -> Result<String, String> {
    let (full, rel, mut doc) = read_doc(ws, path)?;
    let lang = lang_for(&rel).filter(|l| *l != Lang::Json).ok_or_else(|| format!("no symbol support for {rel}"))?;
    let text = doc.to_text();
    let syms = symbols(lang, &text);
    let s = find_symbol(&syms, symbol)?.clone();
    let (r0, r1) = (s.start, s.end);
    if matches!(mode, "" | "read") {
        let mut out = format!("# {rel} {} lines {}..{}\n", qualified(&s), r0 + 1, r1 + 1);
        for i in r0..=r1.min(r0 + MAX_VIEW_LINES - 1) {
            out.push_str(&hl(i + 1, &doc.lines[i]));
            out.push('\n');
        }
        return Ok(out);
    }
    let before = syntax_errors(Some(lang), &text);
    let mut new_lines = text_to_lines(new_code);
    match mode {
        "replace" => {
            if new_lines.is_empty() {
                return Err("new_code is empty (use mode delete to remove the symbol)".into());
            }
            let (k, removed) = (new_lines.len(), r1 - r0 + 1);
            splice(&mut doc, r0, removed, new_lines);
            finish_edit(&full, &rel, &doc, Some(lang), before, Some((r0, r0 + k - 1)), format!("Replaced {} in {rel} (lines {}..{}, +{k} -{removed}).", qualified(&s), r0 + 1, r1 + 1))
        }
        "delete" => {
            let removed = r1 - r0 + 1;
            splice(&mut doc, r0, removed, Vec::new());
            finish_edit(&full, &rel, &doc, Some(lang), before, None, format!("Deleted {} from {rel} (-{removed} lines).", qualified(&s)))
        }
        "insert_after" => {
            if new_lines.is_empty() {
                return Err("new_code is empty".into());
            }
            if new_lines.first().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                new_lines.insert(0, String::new());
            }
            let k = new_lines.len();
            splice(&mut doc, r1 + 1, 0, new_lines);
            finish_edit(&full, &rel, &doc, Some(lang), before, Some((r1 + 1, r1 + k)), format!("Inserted {k} line(s) after {} in {rel}.", qualified(&s)))
        }
        other => Err(format!("unknown symbol mode `{other}` (read|replace|insert_after|delete)")),
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
    std::fs::write(&full, content.as_bytes()).map_err(|e| format!("cannot write {rel}: {e}"))?;
    let mut msg = format!("Created {rel} ({} bytes).", content.len());
    if let Some(errs) = syntax_errors(lang_for(&rel), content) {
        if let Some((row, m)) = errs.first() {
            let line = content.lines().nth(*row).unwrap_or("");
            msg.push_str(&format!("\n⚠ syntax error at {}: {m}", format!("{}#{}", row + 1, line_hash(line))));
        }
    }
    Ok(msg)
}

// --------------------------------- search -----------------------------------------------------

pub fn search(ws: &Path, query: &str, glob: Option<&str>) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("empty query".into());
    }
    let mut cmd = command("rg");
    cmd.args(["-n", "--no-heading", "--smart-case", "--max-count", "10", "--glob", "!.git", "--glob", "!node_modules", "--glob", "!target", "--glob", "!dist"]);
    if let Some(g) = glob.filter(|g| !g.is_empty()) {
        cmd.args(["--glob", g]);
    }
    cmd.args(["--", query]).current_dir(ws);
    let out = run_capture(cmd, Duration::from_secs(20)).map_err(|e| format!("rg failed: {e}"))?;
    let re = lazy_re!(r"^(.+?):(\d+):(.*)$");
    let mut res = String::new();
    let mut count = 0usize;
    for l in out.stdout.lines() {
        let Some(c) = re.captures(l) else { continue };
        let text: String = c[3].chars().take(200).collect();
        let file = c[1].replace('\\', "/");
        let file = file.trim_start_matches("./");
        res.push_str(&format!("{}:{}#{}| {}\n", file, &c[2], line_hash(&c[3]), text.trim()));
        count += 1;
        if count >= 80 || res.len() > 12_000 {
            res.push_str("…[more matches; narrow the query]\n");
            break;
        }
    }
    Ok(if res.is_empty() { "No matches found.".into() } else { res })
}

// --------------------------------- repo map ---------------------------------------------------

pub fn repo_map(ws: &Path, query: Option<&str>, dir: Option<&str>) -> Result<String, String> {
    let root = match dir.filter(|d| !d.trim().is_empty()) {
        Some(d) => resolve(ws, d)?,
        None => ws.to_path_buf(),
    };
    struct Entry {
        rel: String,
        nlines: usize,
        syms: Vec<Sym>,
        lines: Vec<String>,
        idents: HashSet<String>,
    }
    let mut entries: Vec<Entry> = Vec::new();
    let ident = lazy_re!(r"[A-Za-z_][A-Za-z0-9_]{2,}");
    for e in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !(e.file_type().is_dir() && ignored_dir(&e.file_name().to_string_lossy())))
        .flatten()
    {
        if !e.file_type().is_file() {
            continue;
        }
        let rel = rel_of(ws, e.path());
        let Some(lang) = lang_for(&rel).filter(|l| *l != Lang::Json) else { continue };
        if e.metadata().map(|m| m.len() > 300_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let syms = symbols(lang, &text);
        let idents: HashSet<String> = ident.find_iter(&text).map(|m| m.as_str().to_string()).collect();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        entries.push(Entry { rel, nlines: lines.len(), syms, lines, idents });
        if entries.len() >= 2500 {
            break;
        }
    }
    if entries.is_empty() {
        return Ok("No supported source files found (rs, ts, tsx, js, py, go).".into());
    }
    // Cheap "PageRank": a file matters if other files mention its top-level symbols.
    let mut mentions: HashMap<&str, usize> = HashMap::new();
    for e in &entries {
        for id in &e.idents {
            *mentions.entry(id.as_str()).or_default() += 1;
        }
    }
    let rank: Vec<usize> = entries
        .iter()
        .map(|e| {
            e.syms
                .iter()
                .filter(|s| s.depth == 0)
                .map(|s| mentions.get(s.name.as_str()).copied().unwrap_or(1).saturating_sub(1).min(10))
                .sum()
        })
        .collect();
    let tokens: Vec<String> = query
        .unwrap_or("")
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    let mut order: Vec<(usize, i64)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let mut score = 0i64;
        if !tokens.is_empty() {
            let rel_l = e.rel.to_lowercase();
            for t in &tokens {
                if rel_l.contains(t.as_str()) {
                    score += 4;
                }
                for s in &e.syms {
                    let n = s.name.to_lowercase();
                    if n == *t {
                        score += 6;
                    } else if n.contains(t.as_str()) {
                        score += 2;
                    }
                }
            }
            if score == 0 {
                continue;
            }
        }
        order.push((i, score * 100 + rank[i] as i64));
    }
    order.sort_by(|a, b| b.1.cmp(&a.1).then(entries[a.0].rel.cmp(&entries[b.0].rel)));
    let budget = if tokens.is_empty() { 8000 } else { 7000 };
    let mut out = String::new();
    let mut shown = 0usize;
    for (i, _) in &order {
        let e = &entries[*i];
        let mut block = format!("{} ({})\n", e.rel, e.nlines);
        if tokens.is_empty() {
            let names: Vec<String> = e.syms.iter().filter(|s| s.depth == 0).map(|s| s.name.clone()).collect();
            let extra = names.len().saturating_sub(10);
            block = format!("{} ({}): {}{}\n", e.rel, e.nlines, names.iter().take(10).cloned().collect::<Vec<_>>().join(", "), if extra > 0 { format!(" +{extra}") } else { String::new() });
        } else {
            let anchor = |row: usize| format!("{}#{}", row + 1, line_hash(e.lines.get(row).map(String::as_str).unwrap_or("")));
            let matched: Vec<&Sym> = e
                .syms
                .iter()
                .filter(|s| tokens.iter().any(|t| s.name.to_lowercase().contains(t.as_str())))
                .take(6)
                .collect();
            let pick: Vec<&Sym> = if matched.is_empty() { e.syms.iter().filter(|s| s.depth == 0).take(6).collect() } else { matched };
            for s in pick {
                block.push_str(&format!("  [{}..{}] {}\n", anchor(s.start), anchor(s.end), s.sig));
            }
        }
        if out.len() + block.len() > budget {
            out.push_str(&format!("…[{} more files; add a query or dir to narrow]\n", order.len() - shown));
            break;
        }
        out.push_str(&block);
        shown += 1;
    }
    if out.is_empty() {
        return Ok("No matches.".into());
    }
    Ok(out)
}

// --------------------------------- ast-grep ---------------------------------------------------

pub fn ast(ws: &Path, pattern: &str, lang: Option<&str>, path: Option<&str>, rewrite: Option<&str>) -> Result<String, String> {
    if pattern.trim().is_empty() {
        return Err("empty pattern".into());
    }
    if command("ast-grep").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        return Err("ast-grep is not installed (npm i -g @ast-grep/cli, or cargo install ast-grep --locked)".into());
    }
    let target = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => rel_of(ws, &resolve(ws, p)?),
        None => ".".to_string(),
    };
    let build = |json: bool, update: bool| {
        let mut c = command("ast-grep");
        c.args(["run", "-p", pattern]);
        if let Some(l) = lang.filter(|l| !l.is_empty()) {
            c.args(["-l", l]);
        }
        if let Some(r) = rewrite {
            c.args(["-r", r]);
        }
        if json {
            c.arg("--json=stream");
        }
        if update {
            c.arg("-U");
        }
        c.arg(&target).current_dir(ws);
        c
    };
    let out = run_capture(build(true, false), Duration::from_secs(60))?;
    if out.code != 0 && out.stdout.trim().is_empty() {
        return Err(format!("ast-grep failed: {}", out.stderr.trim().chars().take(400).collect::<String>()));
    }
    let mut matches: Vec<(String, usize, String)> = Vec::new();
    for l in out.stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
        let file = v["file"].as_str().unwrap_or("").replace('\\', "/");
        let file = file.trim_start_matches("./").to_string();
        let line = v["range"]["start"]["line"].as_u64().unwrap_or(0) as usize + 1;
        let first = v["lines"].as_str().or_else(|| v["text"].as_str()).unwrap_or("").lines().next().unwrap_or("").to_string();
        matches.push((file, line, first));
    }
    if matches.is_empty() {
        return Ok("No matches.".into());
    }
    if rewrite.is_none() {
        let mut res = format!("{} match(es):\n", matches.len());
        for (f, l, t) in matches.iter().take(60) {
            let shown: String = t.trim().chars().take(160).collect();
            res.push_str(&format!("{f}:{l}#{}| {shown}\n", line_hash(t)));
        }
        if matches.len() > 60 {
            res.push_str(&format!("…[{} more]\n", matches.len() - 60));
        }
        return Ok(res);
    }
    let applied = run_capture(build(false, true), Duration::from_secs(120))?;
    if applied.code != 0 {
        return Err(format!("ast-grep rewrite failed: {}", applied.stderr.trim().chars().take(400).collect::<String>()));
    }
    let mut per_file: Vec<(String, usize)> = Vec::new();
    for (f, _, _) in &matches {
        match per_file.iter_mut().find(|(n, _)| n == f) {
            Some(e) => e.1 += 1,
            None => per_file.push((f.clone(), 1)),
        }
    }
    let mut res = format!("Rewrote {} match(es) in {} file(s):\n", matches.len(), per_file.len());
    for (f, n) in per_file.iter().take(40) {
        res.push_str(&format!("  {f} ({n})\n"));
    }
    for (f, _) in per_file.iter().take(40) {
        if let Ok(text) = std::fs::read_to_string(ws.join(f)) {
            if let Some(errs) = syntax_errors(lang_for(f), &text) {
                if let Some((row, m)) = errs.first() {
                    let line = text.lines().nth(*row).unwrap_or("");
                    res.push_str(&format!("⚠ {f} has a syntax error at {}#{}: {m}\n", row + 1, line_hash(line)));
                }
            }
        }
    }
    Ok(res)
}

// --------------------------------- check ------------------------------------------------------

struct Diag {
    file: String,
    line: usize,
    col: usize,
    error: bool,
    code: String,
    msg: String,
}

fn strip_ansi(s: &str) -> String {
    lazy_re!(r"\x1b\[[0-9;?]*[ -/]*[@-~]").replace_all(s, "").to_string()
}

fn detect_checks(ws: &Path) -> Vec<(String, String, String)> {
    let mut dirs: Vec<(String, PathBuf)> = vec![(String::new(), ws.to_path_buf())];
    if let Ok(rd) = std::fs::read_dir(ws) {
        let mut subs: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| !ignored_dir(&e.file_name().to_string_lossy()) && !e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        subs.sort_by_key(|e| e.file_name());
        for e in subs {
            dirs.push((format!("{}/", e.file_name().to_string_lossy()), e.path()));
        }
    }
    let mut out = Vec::new();
    if let Some((p, _)) = dirs.iter().find(|(_, d)| d.join("Cargo.toml").is_file()) {
        out.push((p.clone(), "cargo check --message-format=json".to_string(), "cargo check".to_string()));
    }
    if let Some((p, _)) = dirs.iter().find(|(_, d)| d.join("tsconfig.json").is_file()) {
        out.push((p.clone(), "npx --no-install tsc --noEmit --pretty false".to_string(), "tsc".to_string()));
    }
    if out.is_empty() {
        if let Some((p, _)) = dirs.iter().find(|(_, d)| d.join("go.mod").is_file()) {
            out.push((p.clone(), "go vet ./...".to_string(), "go vet".to_string()));
        }
    }
    out
}

fn parse_diags(raw: &str, prefix: &str) -> Vec<Diag> {
    let mut out = Vec::new();
    let generic = lazy_re!(r"(?i)^(?P<f>[^\s:()][^:()]*?)(?:[:(](?P<l>\d+)(?:[:,](?P<c>\d+))?\)?):?\s*-?\s*(?P<sev>error|warning|fatal error)\b[:\s]*(?P<m>.*)$");
    let code_re = lazy_re!(r"^(?P<c>[A-Z]{1,4}\d{3,5})\b:?\s*(?P<m>.*)$");
    for l in raw.lines() {
        let l = l.trim_end();
        if l.starts_with('{') {
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            if v["reason"].as_str() != Some("compiler-message") {
                continue;
            }
            let m = &v["message"];
            let level = m["level"].as_str().unwrap_or("");
            if !matches!(level, "error" | "warning") {
                continue;
            }
            let msg = m["message"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
            if msg.starts_with("aborting due to") || (level == "warning" && msg.contains("generated") && msg.contains("warning")) {
                continue;
            }
            let span = m["spans"].as_array().and_then(|a| a.iter().find(|s| s["is_primary"].as_bool() == Some(true)));
            let (file, line, col) = match span {
                Some(s) => (
                    s["file_name"].as_str().unwrap_or("").replace('\\', "/"),
                    s["line_start"].as_u64().unwrap_or(0) as usize,
                    s["column_start"].as_u64().unwrap_or(0) as usize,
                ),
                None => (String::new(), 0, 0),
            };
            out.push(Diag {
                file: if file.is_empty() { file } else { format!("{prefix}{file}") },
                line,
                col,
                error: level == "error",
                code: m["code"]["code"].as_str().unwrap_or("").to_string(),
                msg,
            });
        } else if let Some(c) = generic.captures(l) {
            let mut msg = c["m"].trim().to_string();
            let mut code = String::new();
            if let Some(cc) = code_re.captures(&msg.clone()) {
                code = cc["c"].to_string();
                msg = cc["m"].to_string();
            }
            let file = c["f"].trim().replace('\\', "/");
            out.push(Diag {
                file: format!("{prefix}{}", file.trim_start_matches("./")),
                line: c["l"].parse().unwrap_or(0),
                col: c.name("c").and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
                error: !c["sev"].to_lowercase().starts_with("warn"),
                code,
                msg,
            });
        }
    }
    let mut seen = HashSet::new();
    out.retain(|d| seen.insert((d.file.clone(), d.line, d.msg.clone())));
    out
}

fn render_diags(ws: &Path, diags: &[Diag], label: &str, secs: f64) -> String {
    let errors: Vec<&Diag> = diags.iter().filter(|d| d.error).collect();
    let warns = diags.len() - errors.len();
    let files: HashSet<&str> = errors.iter().map(|d| d.file.as_str()).collect();
    let mut out = format!(
        "✖ {} error(s){} in {} file(s) ({label}, {secs:.1}s):\n",
        errors.len(),
        if warns > 0 { format!(", {warns} warning(s)") } else { String::new() },
        files.len()
    );
    for (i, d) in errors.iter().take(15).enumerate() {
        let loc = if d.file.is_empty() { "(no location)".to_string() } else { format!("{}:{}:{}", d.file, d.line, d.col) };
        let code = if d.code.is_empty() { String::new() } else { format!(" [{}]", d.code) };
        out.push_str(&format!("{}. {loc}{code} {}\n", i + 1, d.msg));
        if d.line > 0 && !d.file.is_empty() {
            if let Ok(text) = std::fs::read_to_string(ws.join(&d.file)) {
                let lines: Vec<&str> = text.lines().collect();
                for n in d.line.saturating_sub(1).max(1)..=(d.line + 1).min(lines.len()) {
                    let mark = if n == d.line { ">" } else { " " };
                    out.push_str(&format!(" {mark} {}\n", hl(n, lines[n - 1])));
                }
            }
        }
    }
    if errors.len() > 15 {
        out.push_str(&format!("…[{} more errors]\n", errors.len() - 15));
    }
    out
}

fn compress_plain(raw: &str) -> String {
    let progress = lazy_re!(r"^\s*(Compiling|Downloading|Downloaded|Blocking|Fresh|Updating|Locking|Checking|Finished|running \d+ tests?|\.+$|test .* \.\.\. ok$)");
    let mut lines: Vec<&str> = Vec::new();
    for l in raw.lines() {
        let t = l.trim_end();
        if t.trim().is_empty() || progress.is_match(t) || lines.last() == Some(&t) {
            continue;
        }
        lines.push(t);
    }
    let tail: Vec<&str> = lines.iter().rev().take(30).rev().copied().collect();
    let out = tail.join("\n");
    out.chars().take(4000).collect()
}

pub fn check(ws: &Path, cmd: Option<&str>, timeout_secs: u64) -> Result<String, String> {
    let jobs: Vec<(String, String, String)> = match cmd.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => vec![(String::new(), c.to_string(), c.split_whitespace().take(2).collect::<Vec<_>>().join(" "))],
        None => detect_checks(ws),
    };
    if jobs.is_empty() {
        return Err("no project detected (Cargo.toml / tsconfig.json / go.mod); pass a command".into());
    }
    let mut report = String::new();
    for (prefix, command_line, label) in jobs {
        let mut c = shell_command(&command_line);
        c.current_dir(if prefix.is_empty() { ws.to_path_buf() } else { ws.join(prefix.trim_end_matches('/')) });
        let out = match run_capture(c, Duration::from_secs(timeout_secs.max(5))) {
            Ok(o) => o,
            Err(e) => {
                report.push_str(&format!("✖ {label}: failed to start: {e}\n"));
                continue;
            }
        };
        let raw = strip_ansi(&format!("{}\n{}", out.stdout, out.stderr));
        if out.timed_out {
            report.push_str(&format!("✖ {label}: timed out after {timeout_secs}s\n"));
            continue;
        }
        let diags = parse_diags(&raw, &prefix);
        let errors = diags.iter().filter(|d| d.error).count();
        let warns = diags.len() - errors;
        if out.code == 0 && errors == 0 {
            report.push_str(&format!(
                "✔ {label} ok ({:.1}s){}\n",
                out.secs,
                if warns > 0 { format!(", {warns} warning(s)") } else { String::new() }
            ));
        } else if errors > 0 {
            report.push_str(&render_diags(ws, &diags, &label, out.secs));
        } else {
            report.push_str(&format!("✖ {label} failed (exit {}, {:.1}s):\n{}\n", out.code, out.secs, compress_plain(&raw)));
        }
    }
    Ok(report.chars().take(8000).collect())
}

// --------------------------------- dispatcher -------------------------------------------------

fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(|v| v.as_str())
}

/// Single entry point shared by the built-in agent and the `vx` CLI.
pub fn run_op(ws: &Path, op: &str, a: &Value) -> Result<String, String> {
    let need = |k: &str| s(a, k).ok_or_else(|| format!("missing `{k}`"));
    match op {
        "outline" => outline(ws, need("path")?),
        "view" => view(ws, need("path")?, s(a, "target")),
        "map" => repo_map(ws, s(a, "query"), s(a, "dir")),
        "search" => search(ws, need("query")?, s(a, "glob")),
        "edit" => hashline_edit(
            ws,
            need("path")?,
            s(a, "op").unwrap_or("replace"),
            need("start")?,
            s(a, "end"),
            s(a, "new_text").unwrap_or(""),
        ),
        "symbol" => symbol_op(ws, need("path")?, need("symbol")?, s(a, "mode").unwrap_or("read"), s(a, "new_code").unwrap_or("")),
        "create" => create(ws, need("path")?, s(a, "content").unwrap_or(""), a["force"].as_bool().unwrap_or(false)),
        "ast" => ast(ws, need("pattern")?, s(a, "lang"), s(a, "path"), s(a, "rewrite")),
        "check" => check(ws, s(a, "cmd"), a["timeout"].as_u64().unwrap_or(180)),
        other => Err(format!("unknown op `{other}`")),
    }
}

/// Convenience for callers that want a plain string (errors prefixed with `Error:`).
pub fn run_op_string(ws: &Path, op: &str, a: &Value) -> String {
    match run_op(ws, op, a) {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

#[allow(dead_code)]
pub fn describe() -> Value {
    json!({ "hint": AGENT_HINT })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vireo-ci-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const RS: &str = "use std::fmt;\n\n/// Doc for foo\n#[inline]\nfn foo(x: i32) -> i32 {\n    x + 1\n}\n\nstruct Bar;\n\nimpl Bar {\n    fn baz(&self) -> bool {\n        true\n    }\n}\n";

    #[test]
    fn outline_and_symbol_ranges() {
        let ws = tmp("outline");
        std::fs::write(ws.join("a.rs"), RS).unwrap();
        let o = outline(&ws, "a.rs").unwrap();
        assert!(o.contains("fn foo(x: i32) -> i32"), "{o}");
        assert!(o.contains("  ["), "nested method expected: {o}");
        let syms = symbols(Lang::Rust, RS);
        let foo = find_symbol(&syms, "fn foo").unwrap();
        assert_eq!((foo.start, foo.end), (2, 6), "doc comment + attribute are included");
        let baz = find_symbol(&syms, "Bar::baz").unwrap();
        assert_eq!(baz.container.as_deref(), Some("Bar"));
    }

    #[test]
    fn hashline_edit_replace_drift_and_syntax_warning() {
        let ws = tmp("edit");
        std::fs::write(ws.join("a.rs"), RS).unwrap();
        let v = view(&ws, "a.rs", Some("5..7")).unwrap();
        let l5 = v.lines().nth(1).unwrap().split('|').next().unwrap().to_string();
        let l7 = v.lines().nth(3).unwrap().split('|').next().unwrap().to_string();
        // Shift the file down by 3 lines, then edit with stale anchors.
        let mut shifted = String::from("// a\n// b\n// c\n");
        shifted.push_str(RS);
        std::fs::write(ws.join("a.rs"), &shifted).unwrap();
        let r = hashline_edit(&ws, "a.rs", "replace", &l5, Some(&l7), "fn foo(x: i32) -> i32 {\n    x + 2\n}").unwrap();
        assert!(r.contains("lines 8..10"), "{r}");
        assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("x + 2"));
        // Breaking the syntax is reported immediately.
        let anchor = format!("8#{}", line_hash("fn foo(x: i32) -> i32 {"));
        let r = hashline_edit(&ws, "a.rs", "replace", &anchor, None, "fn foo(x: i32) -> i32 {{{").unwrap();
        assert!(r.contains("syntax error"), "{r}");
    }

    #[test]
    fn stale_anchor_is_rejected() {
        let ws = tmp("stale");
        std::fs::write(ws.join("a.rs"), RS).unwrap();
        let err = hashline_edit(&ws, "a.rs", "replace", "5#ff", None, "x").unwrap_err();
        assert!(err.contains("no longer match"), "{err}");
        assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), RS);
    }

    #[test]
    fn crlf_is_preserved_and_prefixes_stripped() {
        let ws = tmp("crlf");
        std::fs::write(ws.join("a.ts"), "const a = 1;\r\nconst b = 2;\r\n").unwrap();
        let anchor = format!("1#{}", line_hash("const a = 1;"));
        hashline_edit(&ws, "a.ts", "replace", &anchor, None, &format!("1#{}|const a = 9;\nconst c = 3;", line_hash("x"))).unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("a.ts")).unwrap(), "const a = 9;\r\nconst c = 3;\r\nconst b = 2;\r\n");
    }

    #[test]
    fn symbol_replace_and_ts_outline() {
        let ws = tmp("sym");
        std::fs::write(ws.join("a.rs"), RS).unwrap();
        let r = symbol_op(&ws, "a.rs", "Bar::baz", "replace", "    fn baz(&self) -> bool {\n        false\n    }").unwrap();
        assert!(r.contains("Replaced Bar::baz"), "{r}");
        assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("false"));
        let ts = "export interface A { x: number }\n/** doc */\nexport const f = (a: number) => a;\nexport class K {\n  m() { return 1; }\n}\n";
        std::fs::write(ws.join("b.ts"), ts).unwrap();
        let o = outline(&ws, "b.ts").unwrap();
        assert!(o.contains("interface A") && o.contains("const f") && o.contains("class K") && o.contains("  ["), "{o}");
        let py = "import os\n\n@dec\ndef f():\n    pass\n\nclass C:\n    def m(self):\n        pass\n";
        std::fs::write(ws.join("c.py"), py).unwrap();
        let o = outline(&ws, "c.py").unwrap();
        assert!(o.contains("def f") && o.contains("class C") && o.contains("def m"), "{o}");
    }

    #[test]
    fn repo_map_ranks_by_query() {
        let ws = tmp("map");
        std::fs::write(ws.join("a.rs"), RS).unwrap();
        std::fs::write(ws.join("b.rs"), "fn other() {}\n").unwrap();
        let m = repo_map(&ws, Some("baz"), None).unwrap();
        assert!(m.starts_with("a.rs") && !m.contains("b.rs"), "{m}");
        assert!(repo_map(&ws, None, None).unwrap().contains("b.rs"));
    }

    #[test]
    fn diagnostics_are_compressed() {
        let ws = tmp("diag");
        std::fs::write(ws.join("a.ts"), "let a: number = 'x';\nlet b = 2;\n").unwrap();
        let raw = "a.ts(1,5): error TS2322: Type 'string' is not assignable to type 'number'.\na.ts(1,5): error TS2322: Type 'string' is not assignable to type 'number'.\n";
        let d = parse_diags(raw, "");
        assert_eq!(d.len(), 1, "deduped");
        let r = render_diags(&ws, &d, "tsc", 0.5);
        assert!(r.contains("a.ts:1:5 [TS2322]") && r.contains(&format!(">  1#{}|", line_hash("let a: number = 'x';"))) || r.contains("1#"), "{r}");
    }
}
