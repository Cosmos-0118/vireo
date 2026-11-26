//! Tree-sitter language support, symbol extraction, outline, repo map, and symbol ops.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node, Parser};

use super::doc::{line_hash, read_doc_smart, splice, text_to_lines, Doc};
use super::edit::finish_edit;
use super::util::{command, find_path_candidates, ignored_dir, missing_path_err, rel_of, resolve, run_capture, with_note};
use super::view::{show_line, MAX_VIEW_BYTES, MAX_VIEW_LINES};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Ts,
    Tsx,
    Js,
    Py,
    Go,
    Json,
    CSharp,
    Java,
    C,
    Cpp,
    Ruby,
    Php,
    Kotlin,
    Bash,
}

impl Lang {
    /// Every language with a built-in grammar, in discriminant order.
    pub const ALL: [Lang; 15] = [
        Lang::Rust,
        Lang::Ts,
        Lang::Tsx,
        Lang::Js,
        Lang::Py,
        Lang::Go,
        Lang::Json,
        Lang::CSharp,
        Lang::Java,
        Lang::C,
        Lang::Cpp,
        Lang::Ruby,
        Lang::Php,
        Lang::Kotlin,
        Lang::Bash,
    ];
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
        "cs" => Lang::CSharp,
        "java" => Lang::Java,
        "c" => Lang::C,
        // `.h` is ambiguous; the C++ grammar is a superset that also reads plain C headers.
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h" | "ipp" => Lang::Cpp,
        "rb" | "rake" | "gemspec" => Lang::Ruby,
        "php" | "phtml" => Lang::Php,
        "kt" | "kts" => Lang::Kotlin,
        "sh" | "bash" | "zsh" => Lang::Bash,
        _ => return None,
    })
}

pub(crate) fn ts_language(l: Lang) -> Language {
    match l {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Js => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Py => tree_sitter_python::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Json => tree_sitter_json::LANGUAGE.into(),
        Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
    }
}

fn parse_tree(l: Lang, src: &str) -> Option<tree_sitter::Tree> {
    // One parser per language per thread: allocating + configuring a C parser for every file
    // dominated repo_map on large repos.
    thread_local! {
        static PARSERS: std::cell::RefCell<[Option<Parser>; Lang::ALL.len()]> = const { std::cell::RefCell::new([const { None }; Lang::ALL.len()]) };
    }
    PARSERS.with(|cell| {
        let mut parsers = cell.borrow_mut();
        let slot = &mut parsers[l as usize];
        if slot.is_none() {
            let mut p = Parser::new();
            p.set_language(&ts_language(l)).ok()?;
            *slot = Some(p);
        }
        slot.as_mut()?.parse(src, None)
    })
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

/// 0-based row of the last line of the top-level import block (`use`, `import`, `#include`,
/// `using`, ...), or of the package/namespace header when a file has no imports yet. `None` when
/// the language has no parser or the file has neither.
pub(crate) fn import_end_row(lang: Option<Lang>, src: &str) -> Option<usize> {
    let lang = lang?;
    let tree = parse_tree(lang, src)?;
    let root = tree.root_node();
    let (imports, headers): (&[&str], &[&str]) = match lang {
        Lang::Rust => (&["use_declaration", "extern_crate_declaration"], &[]),
        Lang::Ts | Lang::Tsx | Lang::Js => (&["import_statement"], &[]),
        Lang::Py => (&["import_statement", "import_from_statement", "future_import_statement"], &[]),
        Lang::Go => (&["import_declaration"], &["package_clause"]),
        Lang::Java => (&["import_declaration"], &["package_declaration"]),
        Lang::Kotlin => (&["import_list", "import_header", "import"], &["package_header"]),
        Lang::CSharp => (&["using_directive"], &[]),
        Lang::C | Lang::Cpp => (&["preproc_include"], &[]),
        Lang::Php => (&["namespace_use_declaration"], &["namespace_definition", "php_tag"]),
        Lang::Ruby | Lang::Bash | Lang::Json => (&[], &[]),
    };
    let (mut last_import, mut last_header) = (None, None);
    let mut c = root.walk();
    for ch in root.children(&mut c) {
        let k = ch.kind();
        if imports.contains(&k) {
            last_import = Some(rows(ch).1);
        } else if headers.contains(&k) && ch.named_child_count() <= 1 {
            last_header = Some(rows(ch).1);
        }
    }
    last_import.or(last_header)
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
    // Skip attribute-only lines (`[Fact]`, `@Override`) so the signature shows the declaration itself.
    let text = ntext(n, src);
    let mut lines = text.lines().map(str::trim);
    let mut first = lines.next().unwrap_or("");
    while (first.starts_with('[') && first.ends_with(']')) || (first.starts_with('@') && (first.ends_with(')') || !first.contains(' '))) {
        match lines.next() {
            Some(next) if !next.is_empty() => first = next,
            _ => break,
        }
    }
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

/// Which node kinds make up the outline of a declaration-oriented language handled by [`Walk::generic`].
struct Spec {
    /// Declarations that are listed and whose body is walked for members.
    containers: &'static [&'static str],
    /// Declarations that are listed but not entered.
    leaves: &'static [&'static str],
    /// Wrappers that are walked through without being listed (templates, `#ifdef`, `extern "C"`, ...).
    transparent: &'static [&'static str],
    /// Sibling comment kinds that belong to the declaration below them.
    docs: &'static [&'static str],
}

const CSHARP: Spec = Spec {
    containers: &["namespace_declaration", "file_scoped_namespace_declaration", "class_declaration", "struct_declaration", "interface_declaration", "record_declaration", "record_struct_declaration"],
    leaves: &[
        "method_declaration", "constructor_declaration", "destructor_declaration", "property_declaration", "enum_declaration", "delegate_declaration",
        "local_function_statement", "event_declaration", "event_field_declaration", "indexer_declaration", "operator_declaration", "conversion_operator_declaration",
    ],
    // C# 9 top-level programs wrap every statement (and local function) in `global_statement`.
    transparent: &["global_statement"],
    docs: &["comment"],
};

const JAVA: Spec = Spec {
    containers: &["class_declaration", "interface_declaration", "enum_declaration", "record_declaration", "annotation_type_declaration"],
    leaves: &["method_declaration", "constructor_declaration", "compact_constructor_declaration"],
    transparent: &["enum_body_declarations"],
    docs: &["line_comment", "block_comment"],
};

const C_LANG: Spec = Spec {
    containers: &[],
    leaves: &["function_definition", "struct_specifier", "union_specifier", "enum_specifier", "type_definition", "preproc_function_def"],
    transparent: &["preproc_if", "preproc_ifdef", "preproc_else", "preproc_elif", "linkage_specification", "declaration_list", "declaration"],
    docs: &["comment"],
};

const CPP: Spec = Spec {
    containers: &["namespace_definition", "class_specifier", "struct_specifier", "union_specifier"],
    leaves: &["function_definition", "enum_specifier", "type_definition", "alias_declaration", "preproc_function_def"],
    transparent: &["template_declaration", "preproc_if", "preproc_ifdef", "preproc_else", "preproc_elif", "linkage_specification", "declaration_list", "declaration"],
    docs: &["comment"],
};

const RUBY: Spec = Spec {
    containers: &["module", "class", "singleton_class"],
    leaves: &["method", "singleton_method"],
    transparent: &[],
    docs: &["comment"],
};

const PHP: Spec = Spec {
    containers: &["class_declaration", "interface_declaration", "trait_declaration", "enum_declaration"],
    leaves: &["function_definition", "method_declaration"],
    transparent: &["namespace_definition", "compound_statement"],
    docs: &["comment"],
};

const KOTLIN: Spec = Spec {
    containers: &["class_declaration", "object_declaration", "companion_object"],
    leaves: &["function_declaration", "property_declaration", "type_alias"],
    transparent: &[],
    docs: &["block_comment", "line_comment", "multiline_comment", "comment"],
};

const BASH: Spec = Spec {
    containers: &[],
    leaves: &["function_definition"],
    transparent: &[],
    docs: &["comment"],
};

/// Name of a declaration plus the scope it names explicitly (`Foo` for C++ `Foo::bar`).
fn declarator_name(mut cur: Node, src: &[u8]) -> (String, Option<String>) {
    loop {
        match cur.kind() {
            "qualified_identifier" => {
                let scope = cur.child_by_field_name("scope").map(|s| strip_generics(ntext(s, src).trim_end_matches("::"))).filter(|s| !s.is_empty());
                return match cur.child_by_field_name("name") {
                    Some(inner) => {
                        let (name, inner_scope) = declarator_name(inner, src);
                        (name, inner_scope.or(scope))
                    }
                    None => (ntext(cur, src).to_string(), None),
                };
            }
            k if k.ends_with("_declarator") => match cur.child_by_field_name("declarator").or_else(|| cur.named_child(0)) {
                Some(d) => cur = d,
                None => return (String::new(), None),
            },
            _ => return (ntext(cur, src).trim().to_string(), None),
        }
    }
}

fn decl_name(n: Node, src: &[u8]) -> (String, Option<String>) {
    if let Some(nm) = n.child_by_field_name("name") {
        return declarator_name(nm, src);
    }
    if let Some(d) = n.child_by_field_name("declarator") {
        return declarator_name(d, src);
    }
    match n.kind() {
        "companion_object" => return ("Companion".into(), None),
        // `typealias Name = Type`: the grammar labels the alias name `type`.
        "type_alias" => return (field_text(n, "type", src), None),
        _ => {}
    }
    // Kotlin: the name is an unlabelled child (`val x` wraps it in `variable_declaration`).
    let mut c = n.walk();
    for ch in n.named_children(&mut c) {
        match ch.kind() {
            "identifier" | "simple_identifier" | "type_identifier" => return (ntext(ch, src).to_string(), None),
            "variable_declaration" => return decl_name(ch, src),
            _ => {}
        }
    }
    (String::new(), None)
}

fn body_of(n: Node) -> Node {
    if let Some(b) = n.child_by_field_name("body") {
        return b;
    }
    let mut c = n.walk();
    let found = n.named_children(&mut c).find(|ch| matches!(ch.kind(), "class_body" | "enum_class_body" | "body_statement"));
    found.unwrap_or(n)
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

    /// Table-driven walker for the C family and other declaration-oriented grammars.
    fn generic(&mut self, parent: Node, container: Option<&str>, depth: usize, spec: &Spec) {
        let mut c = parent.walk();
        for n in parent.named_children(&mut c) {
            let kind = n.kind();
            if spec.transparent.contains(&kind) {
                self.generic(n, container, depth, spec);
                continue;
            }
            let is_container = spec.containers.contains(&kind);
            if !is_container && !spec.leaves.contains(&kind) {
                continue;
            }
            // `struct Foo;` / `class Foo;` forward declarations and `struct Foo x;` uses carry no body.
            if kind.ends_with("_specifier") && n.child_by_field_name("body").is_none() {
                continue;
            }
            let (name, scope) = decl_name(n, self.src);
            if name.is_empty() {
                // Anonymous namespace / `class << self`: members belong to the enclosing scope.
                if is_container {
                    self.generic(body_of(n), container, depth, spec);
                }
                continue;
            }
            let owner = scope.as_deref().or(container);
            self.push(name.clone(), n, n, owner, depth, spec.docs);
            if is_container {
                self.generic(body_of(n), Some(&name), depth + 1, spec);
            }
        }
    }

    /// A C# 9 top-level program (`Program.cs` with no class) has no declarations to list, so
    /// give its executable statements one anchored `<top-level>` range.
    fn csharp_top_level(&mut self, root: Node) {
        let mut c = root.walk();
        let mut span: Option<(usize, usize)> = None;
        for n in root.named_children(&mut c) {
            if n.kind() != "global_statement" || n.named_child(0).is_some_and(|s| s.kind() == "local_function_statement") {
                continue;
            }
            let (s, e) = rows(n);
            span = Some(span.map_or((s, e), |(s0, e0)| (s0.min(s), e0.max(e))));
        }
        let Some((start, end)) = span else { return };
        let pos = self.out.iter().position(|s| s.start > start).unwrap_or(self.out.len());
        self.out.insert(
            pos,
            Sym { name: "<top-level>".into(), kind: "top-level".into(), container: None, start, end, sig: "<top-level statements>".into(), depth: 0 },
        );
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
        Lang::CSharp => {
            w.generic(root, None, 0, &CSHARP);
            w.csharp_top_level(root);
        }
        Lang::Java => w.generic(root, None, 0, &JAVA),
        Lang::C => w.generic(root, None, 0, &C_LANG),
        Lang::Cpp => w.generic(root, None, 0, &CPP),
        Lang::Ruby => w.generic(root, None, 0, &RUBY),
        Lang::Php => w.generic(root, None, 0, &PHP),
        Lang::Kotlin => w.generic(root, None, 0, &KOTLIN),
        Lang::Bash => w.generic(root, None, 0, &BASH),
    }
    w.out
}

pub(crate) fn qualified(s: &Sym) -> String {
    match &s.container {
        Some(c) => format!("{c}::{}", s.name),
        None => s.name.clone(),
    }
}

/// `Foo:45` / `Foo@45` pins a symbol to the one starting at (or enclosing) line 45.
fn split_line_pin(q: &str) -> (&str, Option<usize>) {
    if let Some((head, tail)) = q.rsplit_once([':', '@']) {
        if !head.is_empty() && !head.ends_with(':') && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = tail.parse() {
                return (head.trim(), Some(n));
            }
        }
    }
    (q, None)
}

pub(crate) fn find_symbol<'a>(syms: &'a [Sym], query: &str) -> Result<&'a Sym, String> {
    let (query_name, line_pin) = split_line_pin(query.trim());
    let mut q = query_name;
    let mut want_impl = false;
    for kw in [
        "pub(crate) ", "pub ", "async ", "export ", "default ", "fn ", "func ", "def ", "function ", "class ",
        "struct ", "enum ", "trait ", "interface ", "type ", "const ", "let ", "mod ", "abstract ",
        "public ", "private ", "protected ", "internal ", "static ", "final ", "namespace ", "record ", "module ",
        "object ", "fun ", "sealed ", "override ", "virtual ",
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
    if let Some(l) = line_pin {
        let exact: Vec<&Sym> = m.iter().copied().filter(|s| s.start + 1 == l).collect();
        m = if !exact.is_empty() {
            exact
        } else {
            let mut inner: Vec<&Sym> = m.iter().copied().filter(|s| s.start < l && l <= s.end + 1).collect();
            inner.sort_by_key(|s| s.end - s.start);
            inner.into_iter().take(1).collect()
        };
        if m.is_empty() {
            return Err(format!("no symbol `{name}` starts at or encloses line {l} (run outline to list symbols)"));
        }
    }
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
            "symbol `{query}` is ambiguous; pin one by start line (e.g. `{name}:{}`) or qualify it as Container::name. Candidates: {}",
            m[0].start + 1,
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

/// How many leading doc-comment / attribute / decorator lines of the symbol range `r0..=r1` a
/// replacement should leave alone: the symbol range starts at its `///` docs and `#[..]` attributes,
/// so replacing it with just the signature and body would silently delete them. Zero when `new`
/// itself starts with a comment or attribute (it restates them), or the range has none.
pub(crate) fn keep_leading_docs(lines: &[String], r0: usize, r1: usize, new: &[String]) -> usize {
    let leading = |l: &str| {
        let t = l.trim_start();
        t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || t.starts_with("#[") || t.starts_with("#!") || t.starts_with('@') || t.starts_with('#')
    };
    if new.iter().find(|l| !l.trim().is_empty()).is_some_and(|l| leading(l)) {
        return 0;
    }
    let mut k = 0;
    while r0 + k < r1 && lines.get(r0 + k).is_some_and(|l| leading(l)) {
        k += 1;
    }
    k
}

/// Read or change a whole symbol (function, struct, class, ...) by name.
pub fn symbol_op(
    ws: &Path,
    path: &str,
    symbol: &str,
    mode: &str,
    new_code: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let (full, rel, doc, note) = read_doc_smart(ws, path, cwd)?;
    symbol_op_doc(full, rel, doc, symbol, mode, new_code).map(|out| with_note(note, out))
}

fn symbol_op_doc(
    full: PathBuf,
    rel: String,
    mut doc: Doc,
    symbol: &str,
    mode: &str,
    new_code: &str,
) -> Result<String, String> {
    let lang = lang_for(&rel).filter(|l| *l != Lang::Json).ok_or_else(|| format!("no symbol support for {rel}"))?;
    let text = doc.to_text();
    let syms = symbols(lang, &text);
    let s = find_symbol(&syms, symbol)?.clone();
    let (r0, r1) = (s.start, s.end);
    if matches!(mode, "" | "read") {
        let mut out = format!("# {rel} {} lines {}..{}\n", qualified(&s), r0 + 1, r1 + 1);
        for i in r0..=r1.min(r0 + MAX_VIEW_LINES - 1) {
            let line = show_line(&doc, i);
            if i > r0 && out.len() + line.len() > MAX_VIEW_BYTES {
                out.push_str(&format!("…[{} more lines; continue with {rel}:{}..{}]\n", r1 + 1 - i, i + 1, r1 + 1));
                return Ok(out);
            }
            out.push_str(&line);
            out.push('\n');
        }
        if r1 + 1 > r0 + MAX_VIEW_LINES {
            out.push_str(&format!("…[{} more lines; continue with {rel}:{}..{}]\n", r1 + 1 - (r0 + MAX_VIEW_LINES), r0 + MAX_VIEW_LINES + 1, r1 + 1));
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
            let kept = keep_leading_docs(&doc.lines, r0, r1, &new_lines);
            let (k, removed) = (new_lines.len(), r1 - r0 + 1 - kept);
            splice(&mut doc, r0 + kept, removed, new_lines);
            let note = if kept > 0 { format!(" Kept {kept} leading doc/attribute line(s) your new_code did not restate.") } else { String::new() };
            finish_edit(&full, &rel, &doc, Some(lang), before, Some((r0 + kept, r0 + kept + k - 1)), format!("Replaced {} in {rel} (lines {}..{}, +{k} -{removed}).{note}", qualified(&s), r0 + kept + 1, r1 + 1))
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

/// Unique top-level symbol names of a file: `impl Foo` next to `struct Foo` and bare `mod foo;`
/// declarations add nothing to a map line (and would double-count rank).
fn top_names(syms: &[Sym]) -> Vec<&str> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for s in syms.iter().filter(|s| s.depth == 0) {
        let sig = s.sig.trim();
        let bare_mod = sig.ends_with(';') && (sig.starts_with("mod ") || sig.contains(" mod "));
        if !bare_mod && seen.insert(s.name.as_str()) {
            out.push(s.name.as_str());
        }
    }
    out
}

pub(crate) fn is_test_path(rel: &str) -> bool {
    let l = rel.to_lowercase();
    let name = l.rsplit('/').next().unwrap_or(&l);
    name == "tests.rs"
        || name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || l.contains("/__tests__/")
        || l.contains("/tests/")
}

/// Workspace-relative files with uncommitted changes (modified, staged or untracked). Cached on the
/// git index mtime plus a short TTL so repeated `map` calls in a long-lived `vx mcp` stay free.
fn dirty_files(ws: &Path) -> HashSet<String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant, SystemTime};
    type Cache = Option<(PathBuf, Option<SystemTime>, Instant, HashSet<String>)>;
    static CACHE: Mutex<Cache> = Mutex::new(None);
    let git_dir = ws.join(".git");
    if !git_dir.exists() {
        return HashSet::new();
    }
    let stamp = std::fs::metadata(git_dir.join("index")).and_then(|m| m.modified()).ok();
    if let Ok(g) = CACHE.lock() {
        if let Some((p, st, at, set)) = g.as_ref() {
            if p == ws && *st == stamp && at.elapsed() < Duration::from_secs(3) {
                return set.clone();
            }
        }
    }
    let mut cmd = command("git");
    cmd.current_dir(ws).args(["-c", "core.quotepath=off", "status", "--porcelain=v1", "-uall"]);
    let set: HashSet<String> = match run_capture(cmd, Duration::from_secs(5)) {
        Ok(o) if o.code == 0 => o
            .stdout
            .lines()
            .filter(|l| l.len() > 3)
            .map(|l| {
                let p = &l[3..];
                p.rsplit_once(" -> ").map_or(p, |(_, new)| new).trim_matches('"').to_string()
            })
            .collect(),
        _ => HashSet::new(),
    };
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((ws.to_path_buf(), stamp, Instant::now(), set.clone()));
    }
    set
}

/// Project and UI files that have no tree-sitter outline but orient a reader (`vx map` lists them
/// below the code files).
fn is_manifest_path(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    if matches!(name.as_str(), "cargo.toml" | "package.json" | "pyproject.toml" | "go.mod") {
        return true;
    }
    matches!(
        name.rsplit_once('.').map(|(_, e)| e),
        Some("csproj" | "fsproj" | "vbproj" | "props" | "targets" | "sln" | "slnx" | "xaml" | "axaml")
    )
}

/// Cheap regex outline of a manifest/markup file: one symbol per notable line.
fn manifest_syms(rel: &str, text: &str) -> Vec<Sym> {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let stem = |p: &str| {
        let f = p.rsplit(['/', '\\']).next().unwrap_or(p);
        f.rsplit_once('.').map_or(f, |(s, _)| s).to_string()
    };
    let msbuild_ref = lazy_re!(r#"<(PackageReference|PackageVersion|ProjectReference)\s[^>]*?(?:Include|Update)="([^"]+)""#);
    let msbuild_prop = lazy_re!(r"<(TargetFrameworks?|OutputType|UseWPF|UseWinUI|UseMaui)>([^<]+)<");
    let sln = lazy_re!(r#"Project\("[^"]*"\)\s*=\s*"([^"]+)"|<Project\s+Path="([^"]+)""#);
    let xaml = lazy_re!(r#"x:Class="([^"]+)""#);
    let single = lazy_re!(r#"(?m)^\s*(?:name\s*=\s*|"name"\s*:\s*|module\s+)"?([^"\s,]+)"?"#);
    let mut out: Vec<Sym> = Vec::new();
    let mk = |row: usize, label: &str, value: String| Sym {
        name: value.clone(),
        kind: "manifest".into(),
        container: None,
        start: row,
        end: row,
        sig: format!("{label} {value}"),
        depth: 0,
    };
    for (row, line) in text.lines().enumerate() {
        if out.len() >= 40 {
            break;
        }
        match ext {
            "csproj" | "fsproj" | "vbproj" | "props" | "targets" => {
                for c in msbuild_ref.captures_iter(line) {
                    let v = if &c[1] == "ProjectReference" { stem(&c[2]) } else { c[2].to_string() };
                    out.push(mk(row, &c[1], v));
                }
                for c in msbuild_prop.captures_iter(line) {
                    out.push(mk(row, &c[1], c[2].trim().to_string()));
                }
            }
            "sln" | "slnx" => {
                for c in sln.captures_iter(line) {
                    out.push(mk(row, "Project", c.get(1).map_or_else(|| stem(&c[2]), |m| m.as_str().to_string())));
                }
            }
            "xaml" | "axaml" => {
                for c in xaml.captures_iter(line) {
                    out.push(mk(row, "x:Class", c[1].rsplit('.').next().unwrap_or(&c[1]).to_string()));
                }
            }
            _ => {
                // `Cargo.toml`/`pyproject.toml` `name = "x"`, `package.json` `"name": "x"`, `go.mod` `module x`.
                if out.is_empty() {
                    if let Some(c) = single.captures(line) {
                        out.push(mk(row, &name, c[1].to_string()));
                    }
                }
            }
        }
    }
    out
}

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
        /// Hashes of every identifier in the file (not the strings: keeps memory flat on big repos).
        idents: HashSet<u64>,
        /// Project/UI manifest (`.csproj`, `.xaml`, `Cargo.toml`, ...): listed below the code files.
        manifest: bool,
    }
    fn hash_ident(s: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }
    let mut entries: Vec<Entry> = Vec::new();
    let ident = lazy_re!(r"[A-Za-z_][A-Za-z0-9_]{2,}");
    let walk = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| {
            e.depth() == 0 || !(e.file_type().is_some_and(|t| t.is_dir()) && ignored_dir(&e.file_name().to_string_lossy()))
        })
        .build();
    for e in walk.flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = rel_of(ws, e.path());
        let lang = lang_for(&rel).filter(|l| *l != Lang::Json);
        if lang.is_none() && !is_manifest_path(&rel) {
            continue;
        }
        if e.metadata().map(|m| m.len() > 300_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let (syms, idents, manifest) = match lang {
            Some(lang) => (symbols(lang, &text), ident.find_iter(&text).map(|m| hash_ident(m.as_str())).collect(), false),
            None => (manifest_syms(&rel, &text), HashSet::new(), true),
        };
        entries.push(Entry { rel, nlines: text.lines().count(), syms, idents, manifest });
        if entries.len() >= 2500 {
            break;
        }
    }
    if entries.is_empty() {
        return Ok(format!(
            "No supported source files found in {} (rs, ts, tsx, js, py, go, cs, java, c/cpp, rb, php, kt, sh).",
            root.display()
        ));
    }
    let tokens: Vec<String> = query
        .unwrap_or("")
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    let demote_tests = !tokens.iter().any(|t| t.contains("test") || t.contains("spec"));
    // Cheap "PageRank": a file matters if other files mention its top-level symbols. Identifiers
    // found in most files (`new`, `read`, ...) carry no signal, and the sum is damped so a file with
    // dozens of symbols doesn't drown one with a single exported component.
    let mut mentions: HashMap<u64, usize> = HashMap::new();
    for e in &entries {
        for id in &e.idents {
            *mentions.entry(*id).or_default() += 1;
        }
    }
    let common_cap = (entries.len() * 2 / 5).max(8);
    let rank: Vec<i64> = entries
        .iter()
        .map(|e| {
            if e.manifest {
                return -1;
            }
            let mut scores: Vec<usize> = top_names(&e.syms)
                .iter()
                .filter(|n| n.len() >= 4)
                .map(|n| {
                    let m = mentions.get(&hash_ident(n)).copied().unwrap_or(1);
                    if m > common_cap { 0 } else { m.saturating_sub(1).min(10) }
                })
                .collect();
            scores.sort_unstable_by(|a, b| b.cmp(a));
            let top: usize = scores.iter().take(3).sum();
            let rest: usize = scores.iter().skip(3).sum();
            let mut f = top as f64 + rest as f64 / (scores.len().max(1) as f64).sqrt();
            if demote_tests && is_test_path(&e.rel) {
                f *= 0.25;
            }
            (f * 4.0) as i64
        })
        .collect();
    let dirty = dirty_files(ws);
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
        let boost = if dirty.contains(&e.rel) { 200 } else { 0 };
        order.push((i, score * 10_000 + rank[i] + boost));
    }
    order.sort_by(|a, b| b.1.cmp(&a.1).then(entries[a.0].rel.cmp(&entries[b.0].rel)));
    // Bare map over a multi-project repo: interleave the big top-level dirs so one language's
    // symbol density can't blank out another subproject.
    if tokens.is_empty() && dir.map_or(true, |d| d.trim().is_empty()) {
        let top = |i: usize| entries[i].rel.split_once('/').map_or(".", |(d, _)| d).to_string();
        let mut groups: Vec<(String, Vec<(usize, i64)>)> = Vec::new();
        for o in order.drain(..) {
            let t = top(o.0);
            match groups.iter_mut().find(|g| g.0 == t) {
                Some(g) => g.1.push(o),
                None => groups.push((t, vec![o])),
            }
        }
        let min_big = (entries.len() * 15 / 100).max(1);
        let (big, small): (Vec<_>, Vec<_>) = groups.into_iter().partition(|g| g.1.len() >= min_big);
        let mut merged: Vec<(usize, i64)> = Vec::new();
        let mut round = 0usize;
        while big.iter().any(|g| round < g.1.len()) {
            for g in &big {
                if let Some(o) = g.1.get(round) {
                    merged.push(*o);
                }
            }
            round += 1;
        }
        let mut rest: Vec<(usize, i64)> = small.into_iter().flat_map(|g| g.1).collect();
        rest.sort_by(|a, b| b.1.cmp(&a.1).then(entries[a.0].rel.cmp(&entries[b.0].rel)));
        order = merged;
        order.extend(rest);
    }
    // A bare `map` is an orientation overview, so keep it short; a query or dir earns more room.
    let narrowed = dir.map_or(false, |d| !d.trim().is_empty());
    let budget = if !tokens.is_empty() { 7000 } else if narrowed { 8000 } else { 4500 };
    let mut out = dir_note.unwrap_or_default();
    if tokens.is_empty() {
        // Directory histogram first: cheap orientation that lets the caller pick a `dir`.
        let pfx = {
            let r = rel_of(ws, &root);
            let r = r.trim_matches('/');
            if r == "." { String::new() } else { r.to_string() }
        };
        let mut by_dir: HashMap<&str, usize> = HashMap::new();
        for e in &entries {
            let inner = if pfx.is_empty() { e.rel.as_str() } else { e.rel.strip_prefix(pfx.as_str()).map_or(e.rel.as_str(), |r| r.trim_start_matches('/')) };
            let top = inner.split_once('/').map_or(".", |(d, _)| d);
            *by_dir.entry(top).or_default() += 1;
        }
        let mut dirs: Vec<(&str, usize)> = by_dir.into_iter().collect();
        dirs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let listed: Vec<String> = dirs.iter().take(8).map(|(d, n)| format!("{d} {n}")).collect();
        let changed: Vec<&str> = order.iter().map(|(i, _)| entries[*i].rel.as_str()).filter(|r| dirty.contains(*r)).collect();
        let changed_note = if changed.is_empty() {
            String::new()
        } else {
            let more = changed.len().saturating_sub(5);
            format!(
                "; {} uncommitted [M]: {}{}",
                changed.len(),
                changed.iter().take(5).copied().collect::<Vec<_>>().join(", "),
                if more > 0 { format!(" +{more}") } else { String::new() }
            )
        };
        out.push_str(&format!("# {} source files ({}){changed_note}; top-ranked below\n", entries.len(), listed.join(", ")));
    }
    let mut shown = 0usize;
    for (i, _) in &order {
        let e = &entries[*i];
        let mark = if dirty.contains(&e.rel) { " [M]" } else { "" };
        let mut block = format!("{} ({}){mark}\n", e.rel, e.nlines);
        if tokens.is_empty() {
            let names = top_names(&e.syms);
            let extra = names.len().saturating_sub(10);
            if !names.is_empty() {
                block = format!("{} ({}){mark}: {}{}\n", e.rel, e.nlines, names.iter().take(10).copied().collect::<Vec<_>>().join(", "), if extra > 0 { format!(" +{extra}") } else { String::new() });
            }
        } else {
            // Source lines are only needed for the few files actually shown: re-read them now.
            let text = std::fs::read_to_string(ws.join(&e.rel)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let anchor = |row: usize| format!("{}#{}", row + 1, line_hash(lines.get(row).copied().unwrap_or("")));
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

