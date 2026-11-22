//! Tree-sitter language support, symbol extraction, outline, repo map, and symbol ops.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};
use walkdir::WalkDir;

use super::doc::{hl, line_hash, read_doc, read_doc_smart, splice, text_to_lines, Doc};
use super::edit::finish_edit;
use super::util::{find_path_candidates, ignored_dir, missing_path_err, rel_of, resolve};
use super::view::MAX_VIEW_LINES;

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
pub(crate) fn syntax_errors(lang: Option<Lang>, src: &str) -> Option<Vec<(usize, String)>> {
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

pub(crate) fn qualified(s: &Sym) -> String {
    match &s.container {
        Some(c) => format!("{c}::{}", s.name),
        None => s.name.clone(),
    }
}

pub(crate) fn find_symbol<'a>(syms: &'a [Sym], query: &str) -> Result<&'a Sym, String> {
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

pub fn outline(ws: &Path, path: &str, cwd: Option<&Path>) -> Result<String, String> {
    let (_, rel, doc, note) = read_doc_smart(ws, path, cwd)?;
    let prefix = note.map(|n| format!("{n}\n")).unwrap_or_default();
    if rel.to_lowercase().ends_with(".md") || rel.to_lowercase().ends_with(".markdown") {
        return Ok(format!("{prefix}{}", markdown_outline(&rel, &doc)));
    }
    let Some(lang) = lang_for(&rel).filter(|l| *l != Lang::Json) else {
        return Err(format!("no outline support for {rel}; use view"));
    };
    let src = doc.to_text();
    let syms = symbols(lang, &src);
    let mut out = format!("{prefix}# {rel} ({} lines)\n", doc.lines.len());
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

// --------------------------------- repo map ---------------------------------------------------

pub fn repo_map(ws: &Path, query: Option<&str>, dir: Option<&str>) -> Result<String, String> {
    let mut dir_note: Option<String> = None;
    let root = match dir.filter(|d| !d.trim().is_empty()) {
        Some(d) => {
            let r = resolve(ws, d)?;
            if r.exists() {
                r
            } else {
                let rel_attempt = rel_of(ws, &r);
                let cands = find_path_candidates(ws, &rel_attempt, false, true);
                if cands.len() == 1 {
                    dir_note = Some(format!("(resolved dir `{rel_attempt}` -> `{}`)\n", cands[0]));
                    resolve(ws, &cands[0])?
                } else {
                    return Err(missing_path_err(ws, &rel_attempt, false, true));
                }
            }
        }
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
    let mut out = dir_note.unwrap_or_default();
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

