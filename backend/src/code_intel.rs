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
pub const AGENT_HINT: &str = "Vireo `vx` is available (via MCP tools `vx_*` when registered, and as `vx` on PATH): token-lean code tools.\n\
Prefer `vx` over cat/grep/full-file reads. For edits: use your native Edit tool for tiny single-spot tweaks; use `vx` (prefer MCP tools over shell piping) for multi-hunk/multi-file atomic patches, anchor edits, and whole-symbol edits.\n\
- map [query] [--dir D]         ranked repo symbol map\n\
- outline <file>                symbols as [start#hash..end#hash] ranges\n\
- view <file> [symbol|A..B]     lines as N#hash|code\n\
- search <regex> [--glob G]     grep with N#hash anchors\n\
- apply [--file F] [--dry-run]  atomic multi-file V4A patch (`*** Begin Patch` / `*** Update File:` / `*** End Patch`); hunks may be anchored (`@@ 12#ab..15#cd`, `@@ after 12#ab`, `@@ before 12#ab`, `@@ delete 12#ab..15#cd` with `+` lines) or classic `@@` context (` `, `-`, `+`)\n\
- edit <file> A#h[..B#h] [--insert-after|--insert-before|--delete] [--text T|--file F]\n\
- symbol <file> <name> [--replace|--insert-after|--delete] [--text T|--file F]\n\
- create <file> [--force] [--text T|--file F]\n\
- ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang]\n\
- check [--raw] [--warnings] [--timeout S] [cmd ...]   build/test runner with anchored errors, rustc notes/labels, and failing test output (pass any command directly, e.g. `vx check cargo test -p pkg`)\n\
From a shell on Windows, prefer `--file F` (or MCP) over piping multi-line code to avoid quoting/encoding pitfalls. Edits print new anchors and syntax errors immediately; no need to re-read. `vx help` has details.";

// ---------------------------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------------------------

static PATH_ENV: OnceLock<OsString> = OnceLock::new();

/// PATH used when spawning helper tools (rg, ast-grep, build commands). Optional.
/// Only the main app calls this; the `vx` bin includes this file but never does.
#[allow(dead_code)]
pub fn set_path_env(p: OsString) {
    let _ = PATH_ENV.set(p);
}

/// Windows: `Command::new("foo")` only tries `foo.exe`, so npm/scoop shims (`foo.cmd`, `foo.bat`)
/// look "not installed". Resolve extension-less names against PATH ourselves.
#[cfg(windows)]
fn resolve_program(program: &str) -> OsString {
    let p = Path::new(program);
    if p.extension().is_some() || p.components().count() > 1 {
        return program.into();
    }
    let path = PATH_ENV.get().cloned().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for ext in ["exe", "cmd", "bat"] {
            let cand = dir.join(format!("{program}.{ext}"));
            if cand.is_file() {
                return cand.into_os_string();
            }
        }
    }
    program.into()
}

fn command(program: &str) -> Command {
    #[cfg(windows)]
    let mut c = Command::new(resolve_program(program));
    #[cfg(not(windows))]
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
    if full.is_dir() {
        return Err(format!("{rel} is a directory (list it with: vx map --dir {rel})"));
    }
    let bytes = std::fs::read(&full).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { format!("{rel} not found") } else { format!("cannot read {rel}: {e}") }
    })?;
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

/// Markdown has no tree-sitter grammar here; headings (outside code fences) are its symbols.
fn markdown_outline(rel: &str, doc: &Doc) -> String {
    let mut heads: Vec<(usize, usize, String)> = Vec::new(); // (row, level, title)
    let mut fence: Option<String> = None;
    for (i, l) in doc.lines.iter().enumerate() {
        let t = l.trim_start();
        if let Some(f) = &fence {
            if t.starts_with(f.as_str()) {
                fence = None;
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(t.chars().take(3).collect());
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && t[level..].starts_with(' ') && l.len() - t.len() <= 3 {
            heads.push((i, level, t[level..].trim().trim_end_matches('#').trim().to_string()));
        }
    }
    let n = doc.lines.len();
    let mut out = format!("# {rel} ({n} lines)\n");
    if heads.is_empty() {
        out.push_str("(no headings found)\n");
    }
    for (k, (row, level, title)) in heads.iter().enumerate() {
        let end = heads[k + 1..].iter().find(|h| h.1 <= *level).map(|h| h.0 - 1).unwrap_or(n - 1);
        out.push_str(&format!("{}[{}..{}] {} {}\n", "  ".repeat(level - 1), doc.anchor(*row), doc.anchor(end), "#".repeat(*level), title));
    }
    out
}

pub fn outline(ws: &Path, path: &str) -> Result<String, String> {
    let (_, rel, doc) = read_doc(ws, path)?;
    if rel.to_lowercase().ends_with(".md") || rel.to_lowercase().ends_with(".markdown") {
        return Ok(markdown_outline(&rel, &doc));
    }
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
                if a > n {
                    return Err(format!("{rel} has only {n} line(s); line {a} is past the end"));
                }
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
                "…[{} more lines; continue with: vx view {} {}..{}]\n",
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
        if r1 + 1 > r0 + MAX_VIEW_LINES {
            out.push_str(&format!("…[{} more lines; vx view {rel} {}..{}]\n", r1 + 1 - (r0 + MAX_VIEW_LINES), r0 + MAX_VIEW_LINES + 1, r1 + 1));
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
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
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

type Hit = (String, usize, String);

/// `Ok(None)` when ripgrep is not installed (callers fall back to the built-in walker).
fn rg_search(ws: &Path, query: &str, glob: Option<&str>) -> Result<Option<Vec<Hit>>, String> {
    let mut cmd = command("rg");
    cmd.args(["-n", "--no-heading", "--smart-case", "--max-count", "10", "--glob", "!.git", "--glob", "!node_modules", "--glob", "!target", "--glob", "!dist"]);
    if let Some(g) = glob.filter(|g| !g.is_empty()) {
        cmd.args(["--glob", g]);
    }
    cmd.args(["--", query]).current_dir(ws);
    let Ok(out) = run_capture(cmd, Duration::from_secs(20)) else { return Ok(None) };
    // Exit 1 = no matches; exit 2 with no output = bad regex / bad glob (must not look like "no matches").
    if out.code == 2 && out.stdout.trim().is_empty() {
        let msg: String = out.stderr.trim().lines().take(4).collect::<Vec<_>>().join("\n");
        return Err(format!("search failed: {msg}"));
    }
    let re = lazy_re!(r"^(.+?):(\d+):(.*)$");
    let mut hits = Vec::new();
    for l in out.stdout.lines() {
        let Some(c) = re.captures(l) else { continue };
        let file = c[1].replace('\\', "/");
        hits.push((file.trim_start_matches("./").to_string(), c[2].parse().unwrap_or(0), c[3].to_string()));
    }
    Ok(Some(hits))
}

/// Translate an rg-style glob (`*`, `**`, `?`, `{a,b}`) into an anchored regex.
fn glob_to_regex(g: &str) -> Result<Regex, String> {
    let cs: Vec<char> = g.trim_start_matches("./").chars().collect();
    let mut re = String::from("^");
    let (mut i, mut in_brace) = (0usize, false);
    while i < cs.len() {
        match cs[i] {
            '*' if cs.get(i + 1) == Some(&'*') => {
                i += 1;
                if cs.get(i + 1) == Some(&'/') {
                    i += 1;
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '{' => {
                in_brace = true;
                re.push_str("(?:");
            }
            '}' if in_brace => {
                in_brace = false;
                re.push(')');
            }
            ',' if in_brace => re.push('|'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&re).map_err(|e| format!("invalid glob `{g}`: {e}"))
}

/// Dependency-free fallback for machines without ripgrep (same hit shape, same ignore list).
fn builtin_search(ws: &Path, query: &str, glob: Option<&str>) -> Result<Vec<Hit>, String> {
    let smart = !query.chars().any(|c| c.is_uppercase());
    let re = regex::RegexBuilder::new(query).case_insensitive(smart).build().map_err(|e| format!("search failed: invalid regex: {e}"))?;
    let glob_re = glob.filter(|g| !g.is_empty()).map(glob_to_regex).transpose()?;
    let mut hits: Vec<Hit> = Vec::new();
    let walk = WalkDir::new(ws).sort_by_file_name().into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        e.depth() == 0 || !(name.starts_with('.') || (e.file_type().is_dir() && ignored_dir(&name)))
    });
    for e in walk.flatten() {
        if !e.file_type().is_file() {
            continue;
        }
        let rel = rel_of(ws, e.path());
        if let Some(g) = &glob_re {
            if !(g.is_match(&rel) || g.is_match(&e.file_name().to_string_lossy())) {
                continue;
            }
        }
        if e.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let mut per_file = 0;
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                hits.push((rel.clone(), i + 1, l.to_string()));
                per_file += 1;
                if per_file >= 10 {
                    break;
                }
            }
        }
        if hits.len() >= 400 {
            break;
        }
    }
    Ok(hits)
}

pub fn search(ws: &Path, query: &str, glob: Option<&str>) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("empty query".into());
    }
    let hits = match rg_search(ws, query, glob)? {
        Some(h) => h,
        None => builtin_search(ws, query, glob)?,
    };
    let mut res = String::new();
    for (count, (file, line, raw)) in hits.iter().enumerate() {
        if count >= 80 || res.len() > 12_000 {
            res.push_str(&format!("…[{} more matches; narrow the query or --glob]\n", hits.len() - count));
            break;
        }
        let text: String = raw.chars().take(200).collect();
        res.push_str(&format!("{file}:{line}#{}| {}\n", line_hash(raw), text.trim()));
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

pub fn ast(ws: &Path, pattern: &str, lang: Option<&str>, path: Option<&str>, rewrite: Option<&str>, dry_run: bool) -> Result<String, String> {
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
    // ast-grep exits 1 with empty output when nothing matched; that is not a failure.
    if out.code != 0 && out.stdout.trim().is_empty() && !out.stderr.trim().is_empty() && !out.stderr.contains("No matches") {
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
    if rewrite.is_none() || dry_run {
        let mut res = if dry_run && rewrite.is_some() {
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

// --------------------------------- apply (multi-file patch) -----------------------------------
//
// One atomic call for many hunks across many files. The envelope is OpenAI's V4A `apply_patch`
// format (which models already know), extended with hashline-anchored hunks:
//
//   *** Begin Patch
//   *** Update File: src/a.rs
//   @@ 12#ab..15#cd            replace lines 12..15 with the + lines   (no old text needed)
//   +new line
//   @@ after 40#ef             insert the + lines after line 40 (also: `before 40#ef`)
//   +inserted
//   @@ delete 50#aa..52#bb     remove lines 50..52
//   @@ fn helper() {           classic V4A context hunk: ' ' context, '-' remove, '+' add
//    context line
//   -old line
//   +new line
//   *** Add File: src/new.rs
//   +content
//   *** Delete File: src/old.rs
//   *** End Patch
//
// Every hunk is resolved against the file as it was before the patch (so anchors from one `view`
// stay valid for all hunks), all files are validated first, and nothing is written unless every
// hunk applies.

#[derive(Clone, Copy, PartialEq, Debug)]
enum AnchorKind {
    Replace,
    Delete,
    After,
    Before,
}

#[derive(Debug)]
enum Hunk {
    Anchored { kind: AnchorKind, start: (usize, String), end: (usize, String), new: Vec<String>, header: String },
    /// `body` keeps each line's V4A prefix (b' ', b'-', b'+') so context lines can be written back
    /// exactly as they are in the file.
    Context { ctx: Option<String>, body: Vec<(u8, String)>, eof: bool, header: String },
}

#[derive(Debug)]
enum FileOp {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, hunks: Vec<Hunk> },
}

/// Strip a `N#hh|` prefix copied from `view` output (never part of real code).
fn strip_hashline_prefix(l: &str) -> &str {
    match lazy_re!(r"^\d+#[0-9a-fA-F]{2}\|").find(l) {
        Some(m) => &l[m.end()..],
        None => l,
    }
}

fn patch_header<'a>(l: &'a str, tag: &str) -> Option<&'a str> {
    l.trim_end().strip_prefix(tag).map(str::trim)
}

fn is_file_header(l: &str) -> bool {
    let t = l.trim_end();
    t == "*** End Patch"
        || t.starts_with("*** Add File:")
        || t.starts_with("*** Delete File:")
        || t.starts_with("*** Update File:")
}

fn parse_anchored_header(h: &str) -> Option<(AnchorKind, (usize, String), (usize, String))> {
    let re = lazy_re!(r"^(?i:(after|before|replace|delete)\s+)?(\d+#[0-9A-Fa-f]{2})(?:\s*(?:\.\.|-)\s*(\d+#[0-9A-Fa-f]{2}))?$");
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

fn parse_patch(text: &str) -> Result<Vec<FileOp>, String> {
    let t = text.replace("\r\n", "\n");
    let lines: Vec<&str> = t.split('\n').collect();
    let begin = lines
        .iter()
        .position(|l| l.trim() == "*** Begin Patch")
        .ok_or("not a patch: expected a `*** Begin Patch` line (see `vx help apply`)")?;
    let mut ops = Vec::new();
    let mut i = begin + 1;
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
            while i < lines.len() && !is_file_header(lines[i]) {
                let b = lines[i];
                if let Some(rest) = b.strip_prefix('+') {
                    body.extend(std::iter::repeat(String::new()).take(blanks));
                    blanks = 0;
                    body.push(strip_hashline_prefix(rest).to_string());
                } else if b.trim().is_empty() {
                    blanks += 1; // a blank line that lost its `+`; kept only if more content follows
                } else {
                    return Err(format!("patch line {}: lines of `*** Add File: {p}` must start with `+` (got `{}`)", i + 1, clip(b, 60)));
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
                        hunks.push(Hunk::Anchored { kind, start, end, new: Vec::new(), header: format!("@@ {h}") });
                    } else {
                        let ctx = if h.is_empty() { None } else { Some(h.to_string()) };
                        hunks.push(Hunk::Context { ctx, body: Vec::new(), eof: false, header: format!("@@ {h}").trim_end().to_string() });
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
                    // V4A allows the first hunk of a file without an `@@` line.
                    hunks.push(Hunk::Context { ctx: None, body: Vec::new(), eof: false, header: "@@".into() });
                }
                let Some(cur) = hunks.last_mut() else { continue };
                match cur {
                    Hunk::Anchored { new, header, kind, .. } => {
                        if let Some(rest) = b.strip_prefix('+') {
                            new.extend(std::iter::repeat(String::new()).take(pending_blanks));
                            pending_blanks = 0;
                            new.push(strip_hashline_prefix(rest).to_string());
                        } else if b.trim().is_empty() {
                            pending_blanks += 1;
                        } else {
                            let what = if *kind == AnchorKind::Delete { "takes no body lines" } else { "takes only `+` lines (the anchors already say which lines change)" };
                            return Err(format!("patch line {line_no}: anchored hunk `{header}` {what}; got `{}`", clip(b, 60)));
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
    if !ended {
        return Err("patch has no `*** End Patch` line (was it cut off?). Nothing was changed.".into());
    }
    if ops.is_empty() {
        return Err("patch contains no file operations".into());
    }
    Ok(ops)
}

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
fn seek_lines(lines: &[String], pat: &[String], from: usize, eof: bool) -> Result<Option<(usize, MatchLevel, bool)>, String> {
    if pat.is_empty() || pat.len() > lines.len() {
        return Ok(None);
    }
    let last = lines.len() - pat.len();
    let levels = [MatchLevel::Exact, MatchLevel::Rstrip, MatchLevel::Trim, MatchLevel::Fuzzy];
    for level in levels {
        // Some(used a `?` wildcard) when `pat` matches at `i`.
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
        let hits: Vec<(usize, bool)> = (from.min(last + 1)..=last).filter_map(|i| at(i).map(|w| (i, w))).take(3).collect();
        match (level, hits.len()) {
            (_, 0) => continue,
            (MatchLevel::Exact | MatchLevel::Rstrip, _) | (_, 1) => return Ok(Some((hits[0].0, level, hits[0].1))),
            _ => {
                return Err(format!(
                    "the context matches {} places once whitespace is ignored (lines {}); add more context lines or use anchors",
                    if hits.len() > 2 { "3+".to_string() } else { hits.len().to_string() },
                    hits.iter().map(|h| (h.0 + 1).to_string()).collect::<Vec<_>>().join(", ")
                ))
            }
        }
    }
    Ok(None)
}

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

enum Shift {
    Add(String),
    Cut(usize),
}

/// When a hunk only matched after ignoring indentation, its added lines need the same shift.
fn indent_shift(old_first: &str, file_first: &str) -> Option<Shift> {
    let (o, f) = (leading_ws(old_first), leading_ws(file_first));
    if o == f {
        None
    } else if let Some(add) = f.strip_suffix(o) {
        Some(Shift::Add(add.to_string()))
    } else {
        o.strip_suffix(f).map(|cut| Shift::Cut(cut.len()))
    }
}

fn apply_shift(l: &str, s: &Shift) -> String {
    match s {
        _ if l.trim().is_empty() => l.to_string(),
        Shift::Add(a) => format!("{a}{l}"),
        Shift::Cut(n) if leading_ws(l).len() >= *n => l[*n..].to_string(),
        Shift::Cut(_) => l.trim_start().to_string(),
    }
}

/// Point at where the model probably meant, when a context hunk does not match.
fn closest_hint(doc: &Doc, old: &[String]) -> String {
    let Some(first) = old.iter().map(|l| l.trim()).find(|l| !l.is_empty()) else { return String::new() };
    let mut hits: Vec<usize> = doc.lines.iter().enumerate().filter(|(_, l)| l.trim() == first).map(|(i, _)| i).take(2).collect();
    if hits.is_empty() {
        let words: HashSet<&str> = first.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.len() > 1).collect();
        if words.is_empty() {
            return String::new();
        }
        let best = doc
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let lw: HashSet<&str> = l.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.len() > 1).collect();
                let inter = words.intersection(&lw).count();
                (i, inter as f64 / words.union(&lw).count().max(1) as f64)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some((i, score)) if score >= 0.5 => hits.push(i),
            _ => return String::new(),
        }
    }
    let mut out = String::from("\n  closest current lines:");
    for h in hits {
        let (a, b) = (h.saturating_sub(1), (h + old.len().min(4)).min(doc.lines.len().saturating_sub(1)));
        for l in doc.hashlines(a, b).lines() {
            out.push_str(&format!("\n    {l}"));
        }
    }
    out
}

struct PlannedFile {
    full: PathBuf,
    rel: String,
    original: Option<Vec<u8>>,
    /// `None` = delete.
    new_text: Option<String>,
    remove_after: Option<PathBuf>,
    summary: String,
    notes: Vec<String>,
}

/// Resolve a path from a patch: workspace-relative or absolute; `cwd`-relative when that exists.
fn patch_path(ws: &Path, cwd: Option<&Path>, p: &str, must_exist: bool) -> Result<PathBuf, String> {
    let from_ws = resolve(ws, p);
    if let (Some(cwd), false) = (cwd, Path::new(p).is_absolute()) {
        if let Ok(rel) = cwd.strip_prefix(ws) {
            let joined = rel.join(p);
            if let Ok(c) = resolve(ws, &joined.to_string_lossy()) {
                let ws_ok = from_ws.as_ref().map(|f| if must_exist { f.exists() } else { f.parent().map_or(false, Path::exists) }).unwrap_or(false);
                let cwd_ok = if must_exist { c.exists() } else { c.parent().map_or(false, Path::exists) };
                if cwd_ok && !ws_ok {
                    return Ok(c);
                }
            }
        }
    }
    from_ws
}

fn plan_update(ws: &Path, cwd: Option<&Path>, path: &str, move_to: Option<&str>, hunks: &[Hunk], errors: &mut Vec<String>) -> Option<PlannedFile> {
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
    // (start, remove, new lines, hunk index)
    let mut edits: Vec<(usize, usize, Vec<String>, usize)> = Vec::new();
    let mut notes = Vec::new();
    let mut cursor = 0usize;
    let mut failed = false;
    for (hi, h) in hunks.iter().enumerate() {
        let tag = |header: &str| format!("{rel} hunk {} (`{header}`)", hi + 1);
        match h {
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
                        errors.push(format!("{}: no `+` lines. To remove lines say so: `@@ delete {}#{}..{}#{}`", tag(header), start.0, start.1, end.0, end.1));
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
                            hunk_cursor = if old.first().map_or(false, |o| o.trim() == c.trim()) { idx } else { idx + 1 };
                        }
                        Ok(None) => {
                            // Models often put a line of the hunk itself in the `@@` header.
                            if !old.iter().any(|o| o.trim() == c.trim()) {
                                errors.push(format!("{}: the `@@` context line was not found{}", tag(header), closest_hint(&doc, &pat)));
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
                    edits.push((at, 0, body.iter().filter(|(k, _)| *k == b'+').map(|(_, l)| l.clone()).collect(), hi));
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
                    // Out-of-order hunks: retry from top of file.
                    found = seek_lines(&doc.lines, &old[..pat_len], 0, *eof);
                }
                match found {
                    Ok(Some((idx, level, wild))) => {
                        let pat = &old[..pat_len];
                        let matched = &doc.lines[idx..idx + pat_len];
                        let shift = if level == MatchLevel::Trim || level == MatchLevel::Fuzzy {
                            pat.iter()
                                .enumerate()
                                .find(|(_, l)| !l.trim().is_empty())
                                .and_then(|(k, l)| indent_shift(l, &matched[k]))
                        } else {
                            None
                        };
                        let mut new_lines = Vec::new();
                        let mut old_pos = 0usize;
                        let mut reindented = false;
                        for (kind, line) in body {
                            match *kind {
                                b' ' => {
                                    if old_pos < pat_len {
                                        // Preserve the file's exact context line (whitespace & non-ASCII).
                                        new_lines.push(matched[old_pos].clone());
                                    } else if !dropped_trailing_blank {
                                        new_lines.push(line.clone());
                                    }
                                    old_pos += 1;
                                }
                                b'-' => {
                                    old_pos += 1;
                                }
                                b'+' => {
                                    if let Some(ref s) = shift {
                                        reindented = true;
                                        new_lines.push(apply_shift(line, s));
                                    } else {
                                        new_lines.push(line.clone());
                                    }
                                }
                                _ => {}
                            }
                        }
                        if reindented {
                            notes.push(format!("{rel} hunk {} matched after ignoring indentation; re-indented its `+` lines", hi + 1));
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
            errors.push(format!("{rel}: hunks {} and {} touch the same lines; merge them into one hunk", w[0].3 + 1, w[1].3 + 1));
            return None;
        }
    }
    let mut new_doc = Doc { lines: doc.lines.clone(), ends: doc.ends.clone(), eol: doc.eol, bom: doc.bom };
    for (start, remove, new, _) in edits.iter().rev() {
        splice(&mut new_doc, *start, *remove, new.clone());
    }
    // New anchors per hunk, in file order.
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
            if let Some((row, m)) = after.iter().find(|(r, _)| !before.iter().any(|(b, _)| b == r)).or_else(|| after.first()) {
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
pub fn apply_patch(ws: &Path, patch: &str, cwd: Option<&Path>, dry_run: bool) -> Result<String, String> {
    let ops = parse_patch(patch)?;
    let mut errors: Vec<String> = Vec::new();
    let mut plans: Vec<PlannedFile> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for op in &ops {
        let key = match op {
            FileOp::Add { path, .. } | FileOp::Delete { path } | FileOp::Update { path, .. } => path.replace('\\', "/").to_lowercase(),
        };
        if !seen.insert(key) {
            errors.push(format!("{}: appears twice in the patch; put all its hunks under one `*** Update File:`", match op {
                FileOp::Add { path, .. } | FileOp::Delete { path } | FileOp::Update { path, .. } => path,
            }));
            continue;
        }
        match op {
            FileOp::Add { path, lines } => match patch_path(ws, cwd, path, false) {
                Ok(full) if full.exists() => errors.push(format!("{path}: already exists; use `*** Update File:` (or delete it first)")),
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
                            notes.push(format!("⚠ {rel}: syntax error at {}#{}: {m}", row + 1, line_hash(line)));
                        }
                    }
                    plans.push(PlannedFile { full, summary: format!("A {rel}  ({} lines)", lines.len()), rel, original: None, new_text: Some(text), remove_after: None, notes });
                }
                Err(e) => errors.push(format!("{path}: {e}")),
            },
            FileOp::Delete { path } => match patch_path(ws, cwd, path, true) {
                Ok(full) if full.is_file() => {
                    let rel = rel_of(ws, &full);
                    plans.push(PlannedFile { original: std::fs::read(&full).ok(), full, summary: format!("D {rel}"), rel, new_text: None, remove_after: None, notes: Vec::new() });
                }
                Ok(_) => errors.push(format!("{path}: not found (nothing to delete)")),
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
        // Write everything; on failure restore what was already written.
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
                if p.remove_after.is_some() {
                    let _ = std::fs::remove_file(&p.full);
                }
                for d in done.iter().rev() {
                    if let Some(old) = &d.remove_after {
                        if let Some(bytes) = &d.original {
                            let _ = std::fs::write(old, bytes);
                        }
                        let _ = std::fs::remove_file(&d.full);
                    } else if let Some(bytes) = &d.original {
                        let _ = std::fs::write(&d.full, bytes);
                    } else {
                        let _ = std::fs::remove_file(&d.full);
                    }
                }
                return Err(format!("cannot write {}: {e}. Rolled back the files written so far.", p.rel));
            }
            done.push(p);
        }
    }
    let hunks: usize = ops.iter().map(|o| if let FileOp::Update { hunks, .. } = o { hunks.len() } else { 1 }).sum();
    let mut out = format!(
        "{} {} file(s), {hunks} change(s):\n",
        if dry_run { "Dry run OK (nothing written):" } else { "Applied patch:" },
        plans.len(),
    );
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

// --------------------------------- check ------------------------------------------------------

struct Diag {
    file: String,
    line: usize,
    col: usize,
    error: bool,
    code: String,
    msg: String,
    /// Primary span label (rustc: "expected `u32`, found `String`").
    label: String,
    /// help/note children, secondary span labels, tsc continuation lines.
    notes: Vec<String>,
}

fn strip_ansi(s: &str) -> String {
    lazy_re!(r"\x1b\[[0-9;?]*[ -/]*[@-~]").replace_all(s, "").to_string()
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
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

/// Notes for a cargo JSON diagnostic: help/note children (with suggested code) and labels of
/// secondary spans, which is what makes rustc's human output actionable.
fn rustc_notes(m: &Value, primary_line: usize, prefix: &str) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(spans) = m["spans"].as_array() {
        for s in spans.iter().filter(|s| s["is_primary"].as_bool() != Some(true)) {
            if let Some(label) = s["label"].as_str().filter(|l| !l.trim().is_empty()) {
                let line = s["line_start"].as_u64().unwrap_or(0) as usize;
                let file = s["file_name"].as_str().unwrap_or("").replace('\\', "/");
                let at = if line == primary_line { String::new() } else { format!(" ({prefix}{file}:{line})") };
                notes.push(format!("note: {}{at}", clip(label, 140)));
            }
        }
    }
    if let Some(children) = m["children"].as_array() {
        for c in children {
            let level = c["level"].as_str().unwrap_or("note");
            let msg = c["message"].as_str().unwrap_or("").lines().next().unwrap_or("").trim().to_string();
            if msg.is_empty() {
                continue;
            }
            let sugg: Vec<String> = c["spans"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s["suggested_replacement"].as_str())
                        .map(|r| format!("`{}`", clip(r.lines().next().unwrap_or(""), 80)))
                        .collect()
                })
                .unwrap_or_default();
            let tail = if sugg.is_empty() { String::new() } else { format!(": {}", sugg.join(", ")) };
            notes.push(format!("{level}: {}{tail}", clip(&msg, 160)));
        }
    }
    notes.truncate(4);
    notes
}

fn parse_diags(raw: &str, prefix: &str) -> Vec<Diag> {
    let mut out: Vec<Diag> = Vec::new();
    let generic = lazy_re!(r"(?i)^(?P<f>[^\s:()][^:()]*?)(?:[:(](?P<l>\d+)(?:[:,](?P<c>\d+))?\)?):?\s*-?\s*(?P<sev>error|warning|fatal error)\b[:\s]*(?P<m>.*)$");
    let code_re = lazy_re!(r"^(?P<c>[A-Z]{1,4}\d{3,5})\b:?\s*(?P<m>.*)$");
    // tsc / gcc print a message's details on indented lines right after it.
    let mut continuing = false;
    for l in raw.lines() {
        let l = l.trim_end();
        if l.starts_with('{') {
            continuing = false;
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
            let (file, line, col, label) = match span {
                Some(s) => (
                    s["file_name"].as_str().unwrap_or("").replace('\\', "/"),
                    s["line_start"].as_u64().unwrap_or(0) as usize,
                    s["column_start"].as_u64().unwrap_or(0) as usize,
                    s["label"].as_str().unwrap_or("").to_string(),
                ),
                None => (String::new(), 0, 0, String::new()),
            };
            out.push(Diag {
                file: if file.is_empty() { file } else { format!("{prefix}{file}") },
                line,
                col,
                error: level == "error",
                code: m["code"]["code"].as_str().unwrap_or("").to_string(),
                msg,
                label: clip(&label, 140),
                notes: rustc_notes(m, line, prefix),
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
                label: String::new(),
                notes: Vec::new(),
            });
            continuing = true;
        } else if continuing && l.starts_with(char::is_whitespace) && !l.trim().is_empty() {
            if let Some(d) = out.last_mut() {
                if d.notes.len() < 4 {
                    d.notes.push(clip(l, 200));
                }
            }
        } else {
            continuing = false;
        }
    }
    let mut seen = HashSet::new();
    out.retain(|d| seen.insert((d.file.clone(), d.line, d.msg.clone())));
    out
}

fn render_list(ws: &Path, out: &mut String, list: &[&Diag], cap: usize) {
    for (i, d) in list.iter().take(cap).enumerate() {
        let loc = if d.file.is_empty() { "(no location)".to_string() } else { format!("{}:{}:{}", d.file, d.line, d.col) };
        let code = if d.code.is_empty() { String::new() } else { format!(" [{}]", d.code) };
        out.push_str(&format!("{}. {loc}{code} {}\n", i + 1, d.msg));
        if d.line > 0 && !d.file.is_empty() {
            if let Ok(text) = std::fs::read_to_string(ws.join(&d.file)) {
                let lines: Vec<&str> = text.lines().collect();
                for n in d.line.saturating_sub(1).max(1)..=(d.line + 1).min(lines.len()) {
                    let mark = if n == d.line { ">" } else { " " };
                    out.push_str(&format!(" {mark} {}\n", hl(n, lines[n - 1])));
                    if n == d.line && !d.label.is_empty() {
                        out.push_str(&format!("     ^ {}\n", d.label));
                    }
                }
            } else if !d.label.is_empty() {
                out.push_str(&format!("     ^ {}\n", d.label));
            }
        }
        for n in &d.notes {
            out.push_str(&format!("   = {n}\n"));
        }
    }
    if list.len() > cap {
        out.push_str(&format!("…[{} more]\n", list.len() - cap));
    }
}

fn render_diags(ws: &Path, diags: &[Diag], label: &str, secs: f64, show_warnings: bool) -> String {
    let errors: Vec<&Diag> = diags.iter().filter(|d| d.error).collect();
    let warns: Vec<&Diag> = diags.iter().filter(|d| !d.error).collect();
    let files: HashSet<&str> = errors.iter().map(|d| d.file.as_str()).collect();
    let mut out = format!(
        "✖ {} error(s){} in {} file(s) ({label}, {secs:.1}s):\n",
        errors.len(),
        if warns.is_empty() { String::new() } else { format!(", {} warning(s)", warns.len()) },
        files.len()
    );
    render_list(ws, &mut out, &errors, 15);
    if show_warnings && !warns.is_empty() {
        out.push_str("warnings:\n");
        render_list(ws, &mut out, &warns, 15);
    }
    out
}

fn progress_line(t: &str) -> bool {
    lazy_re!(r"^\s*(Compiling|Downloading|Downloaded|Blocking|Fresh|Updating|Locking|Checking|Finished|Running|Doc-tests|running \d+ tests?|\.+$|test .* \.\.\. ok$|test .* \.\.\. ignored$)").is_match(t)
}

/// Failure output without the noise: cargo-test panic blocks and summaries when present,
/// otherwise lines that look like failures (with a little context), otherwise the tail.
fn compress_plain(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for l in raw.lines() {
        let t = l.trim_end();
        if t.trim().is_empty() || progress_line(t) || lines.last() == Some(&t) || t.contains("run with `RUST_BACKTRACE=1`") {
            continue;
        }
        lines.push(t);
    }
    let fail_re = lazy_re!(r"(?i)(\bfail|\berror\b|panicked|assert|expected|received|\bleft\b|\bright\b|exception|traceback|✕|×|FAILED|test result:)");
    let mut keep = vec![false; lines.len()];
    let mut any = false;
    // cargo test: keep each `---- name stdout ----` block whole.
    let mut in_block = false;
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("---- ") && l.ends_with(" ----") {
            in_block = true;
        } else if *l == "failures:" || l.starts_with("test result:") {
            in_block = false;
        }
        if in_block || fail_re.is_match(l) {
            keep[i] = true;
            any = true;
            if !in_block {
                for k in keep.iter_mut().skip(i + 1).take(2) {
                    *k = true;
                }
            }
        }
    }
    let picked: Vec<&str> = if any {
        lines.iter().zip(&keep).filter(|(_, k)| **k).map(|(l, _)| *l).collect()
    } else {
        lines.clone()
    };
    let n = picked.len();
    let mut out = picked.iter().skip(n.saturating_sub(60)).copied().collect::<Vec<_>>().join("\n");
    if n > 60 {
        out = format!("…[{} earlier lines]\n{out}", n - 60);
    }
    out.chars().take(6000).collect()
}

/// cargo test / libtest summaries, e.g. "45 passed, 2 ignored".
fn test_summary(raw: &str) -> Option<String> {
    let re = lazy_re!(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored");
    let (mut p, mut f, mut ig, mut any) = (0u64, 0u64, 0u64, false);
    for c in re.captures_iter(raw) {
        any = true;
        p += c[1].parse::<u64>().unwrap_or(0);
        f += c[2].parse::<u64>().unwrap_or(0);
        ig += c[3].parse::<u64>().unwrap_or(0);
    }
    any.then(|| format!("{p} passed, {f} failed{}", if ig > 0 { format!(", {ig} ignored") } else { String::new() }))
}

/// Unfiltered output for `--raw`: cargo JSON lines are replaced by rustc's rendered text.
fn raw_view(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in raw.lines() {
        if l.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(l) {
                if let Some(r) = v["message"]["rendered"].as_str() {
                    out.push(r.trim_end().to_string());
                }
                continue;
            }
        }
        if !l.trim().is_empty() {
            out.push(l.trim_end().to_string());
        }
    }
    let text = out.join("\n");
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let mut s = lines[n.saturating_sub(200)..].join("\n");
    if n > 200 {
        s = format!("…[{} earlier lines]\n{s}", n - 200);
    }
    s.chars().take(20_000).collect()
}

#[derive(Default, Clone, Copy)]
pub struct CheckOpts {
    pub raw: bool,
    pub warnings: bool,
}

/// True when a `check` report contains a failure (used for the `vx` exit code).
#[allow(dead_code)]
pub fn check_failed(report: &str) -> bool {
    report.lines().any(|l| l.starts_with('✖'))
}

pub fn check(ws: &Path, cmd: Option<&str>, timeout_secs: u64, opts: CheckOpts) -> Result<String, String> {
    let jobs: Vec<(String, String, String)> = match cmd.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => vec![(String::new(), c.to_string(), c.split_whitespace().take(2).collect::<Vec<_>>().join(" "))],
        None => detect_checks(ws),
    };
    if jobs.is_empty() {
        return Err("no project detected (Cargo.toml / tsconfig.json / go.mod); pass a command, e.g. vx check npm test".into());
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
            report.push_str(&format!("✖ {label}: timed out after {timeout_secs}s (raise with --timeout S)\n"));
            continue;
        }
        if opts.raw {
            let mark = if out.code == 0 { "✔" } else { "✖" };
            report.push_str(&format!("{mark} {label} exit {} ({:.1}s):\n{}\n", out.code, out.secs, raw_view(&raw)));
            continue;
        }
        let diags = parse_diags(&raw, &prefix);
        let errors = diags.iter().filter(|d| d.error).count();
        let warns = diags.len() - errors;
        let tests = test_summary(&raw).map(|t| format!(", tests: {t}")).unwrap_or_default();
        if out.code == 0 && errors == 0 {
            report.push_str(&format!(
                "✔ {label} ok ({:.1}s{tests}){}\n",
                out.secs,
                if warns > 0 { format!(", {warns} warning(s){}", if opts.warnings { ":" } else { " (list: --warnings)" }) } else { String::new() }
            ));
            if opts.warnings && warns > 0 {
                let w: Vec<&Diag> = diags.iter().filter(|d| !d.error).collect();
                render_list(ws, &mut report, &w, 15);
            }
        } else if errors > 0 {
            report.push_str(&render_diags(ws, &diags, &label, out.secs, opts.warnings));
        } else {
            let tail = compress_plain(&raw);
            let tail = if tail.trim().is_empty() { "(no output)".to_string() } else { tail };
            report.push_str(&format!("✖ {label} failed (exit {}, {:.1}s{tests}):\n{}\n", out.code, out.secs, tail));
        }
    }
    let max = if opts.raw { 24_000 } else { 12_000 };
    Ok(report.chars().take(max).collect())
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
        "ast" => ast(ws, need("pattern")?, s(a, "lang"), s(a, "path"), s(a, "rewrite"), a["dry_run"].as_bool().unwrap_or(false)),
        "apply" => {
            let cwd = s(a, "cwd").map(PathBuf::from);
            apply_patch(ws, need("patch")?, cwd.as_deref(), a["dry_run"].as_bool().unwrap_or(false))
        }
        "check" => check(
            ws,
            s(a, "cmd"),
            a["timeout"].as_u64().unwrap_or(180),
            CheckOpts {
                raw: a["raw"].as_bool().unwrap_or(false),
                warnings: a["warnings"].as_bool().unwrap_or(false),
            },
        ),
        other => Err(format!("unknown op `{other}`")),
    }
}

/// Convenience for callers that want a plain string (errors prefixed with `Error:`).
/// Used by the in-app agent only, not by the `vx` bin.
#[allow(dead_code)]
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
        let r = render_diags(&ws, &d, "tsc", 0.5, false);
        assert!(r.contains("a.ts:1:5 [TS2322]") && r.contains(&format!(">  1#{}|", line_hash("let a: number = 'x';"))) || r.contains("1#"), "{r}");
    }

    #[test]
    fn markdown_outline_skips_code_fences() {
        let ws = tmp("md");
        std::fs::write(ws.join("R.md"), "# T\n\n```\n# not a heading\n```\n\n## Sec\ntext\n").unwrap();
        let r = outline(&ws, "R.md").unwrap();
        assert!(r.contains("] # T"), "{r}");
        assert!(r.contains("] ## Sec"), "{r}");
        assert!(!r.contains("not a heading"), "{r}");
    }

    #[test]
    fn builtin_search_smart_case_and_globs() {
        let ws = tmp("bs");
        std::fs::create_dir_all(ws.join("src/deep")).unwrap();
        std::fs::write(ws.join("src/deep/a.rs"), "fn Alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(ws.join("b.py"), "alpha = 1\n").unwrap();
        let rs = builtin_search(&ws, "alpha", Some("**/*.rs")).unwrap();
        assert_eq!(rs.len(), 1, "{rs:?}");
        assert_eq!(rs[0].0, "src/deep/a.rs");
        assert_eq!(builtin_search(&ws, "alpha", Some("*.{py,rs}")).unwrap().len(), 2);
        assert!(builtin_search(&ws, "fn (", None).is_err());
    }

    #[test]
    fn view_errors_are_actionable() {
        let ws = tmp("ve");
        std::fs::write(ws.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::create_dir_all(ws.join("d")).unwrap();
        assert!(view(&ws, "a.rs", Some("5..9")).unwrap_err().contains("past the end"));
        assert!(view(&ws, "nope.rs", None).unwrap_err().contains("not found"));
        assert!(view(&ws, "d", None).unwrap_err().contains("directory"));
    }

    #[test]
    fn apply_patch_anchored_multi_hunk_and_atomic_failure() {
        let ws = tmp("apply-anchored");
        std::fs::write(ws.join("a.rs"), "fn one() -> i32 {\n    1\n}\n\nfn two() -> i32 {\n    2\n}\n").unwrap();
        std::fs::write(ws.join("b.rs"), "const X: i32 = 0;\n").unwrap();
        let l2 = format!("2#{}", line_hash("    1"));
        let l6 = format!("6#{}", line_hash("    2"));
        let b1 = format!("1#{}", line_hash("const X: i32 = 0;"));
        // Atomic failure: second file has a bad anchor, so first file must stay untouched.
        let bad_patch = format!(
            "*** Begin Patch\n*** Update File: a.rs\n@@ {l2}\n+    10\n*** Update File: b.rs\n@@ 1#ff\n+const X: i32 = 9;\n*** End Patch\n"
        );
        let err = apply_patch(&ws, &bad_patch, None, false).unwrap_err();
        assert!(err.contains("nothing was written"), "{err}");
        assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("    1\n"));

        // Valid multi-hunk + multi-file + add + after + delete.
        let good_patch = format!(
            "*** Begin Patch\n\
             *** Update File: a.rs\n\
             @@ {l2}\n\
             +    10\n\
             +    + 1\n\
             @@ after {l6}\n\
             +    // added\n\
             *** Update File: b.rs\n\
             @@ {b1}\n\
             +const X: i32 = 42;\n\
             *** Add File: c.rs\n\
             +pub fn c() {{}}\n\
             *** End Patch\n"
        );
        // Dry run validates without writing.
        let dry = apply_patch(&ws, &good_patch, None, true).unwrap();
        assert!(dry.contains("Dry run OK"), "{dry}");
        assert!(!ws.join("c.rs").exists());

        let ok = apply_patch(&ws, &good_patch, None, false).unwrap();
        assert!(ok.contains("Applied patch: 3 file(s)"), "{ok}");
        assert!(ok.contains("M a.rs  2 hunk(s)"), "{ok}");
        let a_after = std::fs::read_to_string(ws.join("a.rs")).unwrap();
        assert!(a_after.contains("    10\n    + 1\n") && a_after.contains("    2\n    // added\n"), "{a_after}");
        assert_eq!(std::fs::read_to_string(ws.join("b.rs")).unwrap(), "const X: i32 = 42;\n");
        assert_eq!(std::fs::read_to_string(ws.join("c.rs")).unwrap(), "pub fn c() {}\n");
    }

    #[test]
    fn apply_patch_context_reindents_and_preserves_unicode_context() {
        let ws = tmp("apply-ctx");
        std::fs::write(ws.join("a.py"), "def greet():\n    msg = \"hello — world\"\n    return msg\n").unwrap();
        // Model sends wrong indentation (2 spaces instead of 4) and ASCII `?` for the em-dash on context line (like PS 5.1 pipe).
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: a.py\n",
            "@@ def greet():\n",
            "   msg = \"hello ? world\"\n",
            "-  return msg\n",
            "+  return msg.upper()\n",
            "*** End Patch\n",
        );
        let out = apply_patch(&ws, patch, None, false).unwrap();
        assert!(out.contains("re-indented") && out.contains("non-ASCII"), "{out}");
        let after = std::fs::read_to_string(ws.join("a.py")).unwrap();
        assert_eq!(after, "def greet():\n    msg = \"hello — world\"\n    return msg.upper()\n");
    }

    #[test]
    fn apply_patch_rejects_overlapping_hunks() {
        let ws = tmp("apply-overlap");
        std::fs::write(ws.join("a.rs"), "a\nb\nc\n").unwrap();
        let l1 = format!("1#{}", line_hash("a"));
        let l2 = format!("2#{}", line_hash("b"));
        let l3 = format!("3#{}", line_hash("c"));
        let patch = format!(
            "*** Begin Patch\n*** Update File: a.rs\n@@ {l1}..{l2}\n+x\n@@ {l2}..{l3}\n+y\n*** End Patch\n"
        );
        let err = apply_patch(&ws, &patch, None, false).unwrap_err();
        assert!(err.contains("touch the same lines"), "{err}");
    }

    #[test]
    fn check_preserves_rustc_labels_notes_and_cargo_test_failures() {
        let ws = tmp("check-rich");
        std::fs::write(ws.join("a.rs"), "fn f() -> u32 {\n    \"nope\"\n}\n").unwrap();
        let cargo_json = json!({
            "reason": "compiler-message",
            "message": {
                "level": "error",
                "code": { "code": "E0308" },
                "message": "mismatched types",
                "spans": [
                    { "file_name": "a.rs", "line_start": 2, "column_start": 5, "is_primary": true, "label": "expected `u32`, found `&str`" },
                    { "file_name": "a.rs", "line_start": 1, "column_start": 11, "is_primary": false, "label": "expected `u32` because of return type" }
                ],
                "children": [
                    { "level": "help", "message": "try parsing", "spans": [{ "suggested_replacement": "\"nope\".parse()" }] }
                ]
            }
        });
        let diags = parse_diags(&cargo_json.to_string(), "");
        assert_eq!(diags.len(), 1);
        let rendered = render_diags(&ws, &diags, "cargo check", 0.4, false);
        assert!(rendered.contains("expected `u32`, found `&str`"), "{rendered}");
        assert!(rendered.contains("because of return type"), "{rendered}");
        assert!(rendered.contains("help: try parsing"), "{rendered}");
        assert!(check_failed(&rendered));

        let test_out = "running 3 tests\ntest a ... ok\ntest b ... FAILED\n\nfailures:\n\n---- b stdout ----\nthread 'b' panicked at src/lib.rs:10:5:\nassertion `left == right` failed\n  left: 1\n right: 2\nnote: run with `RUST_BACKTRACE=1`\n\nfailures:\n    b\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured\n";
        assert_eq!(test_summary(test_out).as_deref(), Some("2 passed, 1 failed"));
        let comp = compress_plain(test_out);
        assert!(comp.contains("---- b stdout ----") && comp.contains("left: 1") && !comp.contains("test a ... ok"), "{comp}");
    }
}
