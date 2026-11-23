//! In-process structural search / rewrite (ast-grep patterns) over the tree-sitter grammars that
//! are already linked for outlines and symbol edits. This replaces shelling out to a separate
//! ~55 MB `ast-grep.exe`; the external CLI is only used for languages not built in.

use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::replacer::TemplateFix;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{Language, Pattern, PatternError};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::symbols::{lang_for, ts_language, Lang};
use super::util::{ignored_dir, rel_of};

/// Files larger than this are skipped (minified bundles, generated code).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
struct SgLang(Lang);

impl SgLang {
    /// Languages where `$` is not a valid identifier character get metavariables rewritten to
    /// this placeholder before parsing (same choice as ast-grep's own language definitions).
    fn expando(self) -> char {
        match self.0 {
            Lang::Rust | Lang::Py | Lang::Go => 'µ',
            Lang::Ts | Lang::Tsx | Lang::Js | Lang::Json => '$',
        }
    }
}

impl Language for SgLang {
    fn kind_to_id(&self, kind: &str) -> u16 {
        self.get_ts_language().id_for_node_kind(kind, true)
    }
    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.get_ts_language()
            .field_id_for_name(field)
            .map(|f| f.get())
    }
    fn expando_char(&self) -> char {
        self.expando()
    }
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        let expando = self.expando();
        if expando == '$' {
            return Cow::Borrowed(query);
        }
        // `$A`, `$$A`, `$$$A` and the anonymous `$$$` become expando-prefixed identifiers.
        let mut out = String::with_capacity(query.len());
        let mut dollars = 0usize;
        for c in query.chars() {
            if c == '$' {
                dollars += 1;
                continue;
            }
            let sigil = if matches!(c, 'A'..='Z' | '_') || dollars == 3 {
                expando
            } else {
                '$'
            };
            out.extend(std::iter::repeat_n(sigil, dollars));
            dollars = 0;
            out.push(c);
        }
        let sigil = if dollars == 3 { expando } else { '$' };
        out.extend(std::iter::repeat_n(sigil, dollars));
        Cow::Owned(out)
    }
    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, *self))
    }
}

impl LanguageExt for SgLang {
    fn get_ts_language(&self) -> TSLanguage {
        ts_language(self.0)
    }
}

const ALL: [Lang; 7] = [
    Lang::Rust,
    Lang::Ts,
    Lang::Tsx,
    Lang::Js,
    Lang::Py,
    Lang::Go,
    Lang::Json,
];

/// Map an ast-grep `-l` name to built-in languages. `None` = not built in (use the external CLI).
pub(crate) fn builtin_langs(name: Option<&str>) -> Option<Vec<Lang>> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Some(ALL.to_vec());
    };
    Some(match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => vec![Lang::Rust],
        "typescript" | "ts" => vec![Lang::Ts],
        "tsx" => vec![Lang::Tsx],
        "javascript" | "js" | "jsx" => vec![Lang::Js],
        "python" | "py" => vec![Lang::Py],
        "go" | "golang" => vec![Lang::Go],
        "json" => vec![Lang::Json],
        _ => return None,
    })
}

/// One match: workspace-relative file, 1-based line, and the full source line it starts on.
pub(crate) type Match = (String, usize, String);

pub(crate) struct Outcome {
    pub matches: Vec<Match>,
    /// Files rewritten, with the number of replacements in each (empty unless rewriting).
    pub rewritten: Vec<(String, usize)>,
}

/// Search (and optionally rewrite) `target` (a file or directory inside `ws`).
pub(crate) fn run(
    ws: &Path,
    target: &Path,
    langs: &[Lang],
    pattern: &str,
    rewrite: Option<&str>,
) -> Result<Outcome, String> {
    // Compile the pattern once per language; a pattern that parses in no requested language is
    // a user error worth reporting.
    let mut compiled: Vec<(Lang, Pattern, Option<TemplateFix>)> = Vec::new();
    let mut first_err = None;
    for &l in langs {
        match Pattern::try_new(pattern, SgLang(l)) {
            Ok(p) => {
                let fix = match rewrite {
                    Some(r) => Some(
                        TemplateFix::try_new(r, &SgLang(l))
                            .map_err(|e| format!("invalid rewrite: {e}"))?,
                    ),
                    None => None,
                };
                compiled.push((l, p, fix));
            }
            Err(e) => {
                first_err.get_or_insert(e.to_string());
            }
        }
    }
    if compiled.is_empty() {
        return Err(format!(
            "pattern does not parse: {}",
            first_err.unwrap_or_default()
        ));
    }

    let files = collect_files(target, &compiled.iter().map(|c| c.0).collect::<Vec<_>>());
    let results: Mutex<Vec<(String, Vec<(usize, String)>, usize)>> = Mutex::new(Vec::new());
    let write_err: Mutex<Option<String>> = Mutex::new(None);
    // (path, original text) of every file already rewritten, so a later failure can undo them.
    let written: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = files.len().div_ceil(workers).max(1);
    std::thread::scope(|s| {
        for part in files.chunks(chunk) {
            let (compiled, results, write_err, written) = (&compiled, &results, &write_err, &written);
            s.spawn(move || {
                for (path, lang) in part {
                    let Some((_, pat, fix)) = compiled.iter().find(|c| c.0 == *lang) else {
                        continue;
                    };
                    let Ok(src) = std::fs::read_to_string(path) else {
                        continue;
                    };
                    let rel = rel_of(ws, path);
                    match scan_file(&src, *lang, pat, fix.as_ref()) {
                        Some((found, Some(new_src), edits)) => {
                            if write_err.lock().unwrap().is_some() {
                                continue; // a write already failed: stop touching files
                            }
                            if let Err(e) = std::fs::write(path, new_src.as_bytes()) {
                                write_err
                                    .lock()
                                    .unwrap()
                                    .get_or_insert(format!("cannot write {rel}: {e}"));
                                continue;
                            }
                            written.lock().unwrap().push((path.clone(), src.clone()));
                            results.lock().unwrap().push((rel, found, edits));
                        }
                        Some((found, None, _)) => results.lock().unwrap().push((rel, found, 0)),
                        None => {}
                    }
                }
            });
        }
    });
    if let Some(e) = write_err.into_inner().unwrap() {
        let undone = written.into_inner().unwrap();
        let n = undone.len();
        for (p, original) in undone {
            let _ = std::fs::write(p, original.as_bytes());
        }
        return Err(if n > 0 { format!("{e}; rolled back {n} file(s) already rewritten") } else { e });
    }
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Outcome {
        matches: Vec::new(),
        rewritten: Vec::new(),
    };
    for (rel, found, edits) in results {
        if edits > 0 {
            out.rewritten.push((rel.clone(), edits));
        }
        out.matches
            .extend(found.into_iter().map(|(l, t)| (rel.clone(), l, t)));
    }
    Ok(out)
}

/// Matches in one file as `(line, source line)`, plus the rewritten text and edit count when a
/// rewrite applies. `None` when nothing matched.
#[allow(clippy::type_complexity)]
fn scan_file(
    src: &str,
    lang: Lang,
    pat: &Pattern,
    fix: Option<&TemplateFix>,
) -> Option<(Vec<(usize, String)>, Option<String>, usize)> {
    let root = ast_grep_core::AstGrep::new(src, SgLang(lang));
    let node = root.root();
    let lines: Vec<&str> = src.lines().collect();
    let found: Vec<(usize, String)> = node
        .find_all(pat)
        .map(|m| {
            let row = m.start_pos().line();
            (row + 1, lines.get(row).copied().unwrap_or("").to_string())
        })
        .collect();
    if found.is_empty() {
        return None;
    }
    let Some(fix) = fix else {
        return Some((found, None, 0));
    };
    let mut edits = node.replace_all(pat, fix);
    if edits.is_empty() {
        return Some((found, None, 0));
    }
    edits.sort_by_key(|e| e.position);
    let mut bytes = src.as_bytes().to_vec();
    let n = edits.len();
    for e in edits.into_iter().rev() {
        bytes.splice(e.position..e.position + e.deleted_length, e.inserted_text);
    }
    let mut new_src = String::from_utf8(bytes).ok()?;
    if src.contains("\r\n") {
        // Keep CRLF files CRLF: rewrites are templated with bare `\n`.
        new_src = new_src.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Some((found, (new_src != src).then_some(new_src), n))
}

/// Source files under `target` in the requested languages, honoring .gitignore / .ignore and
/// skipping hidden, vendored and build directories.
fn collect_files(target: &Path, langs: &[Lang]) -> Vec<(PathBuf, Lang)> {
    let pick = |p: &Path| {
        let lang = lang_for(&p.to_string_lossy())?;
        langs.contains(&lang).then_some(lang)
    };
    if target.is_file() {
        // An explicitly named file is searched even if ignored, like ast-grep does.
        return pick(target)
            .map(|l| vec![(target.to_path_buf(), l)])
            .unwrap_or_default();
    }
    let mut out = Vec::new();
    let walk = ignore::WalkBuilder::new(target)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && ignored_dir(&e.file_name().to_string_lossy()))
        })
        .build();
    for e in walk.flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if e.metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        if let Some(l) = pick(e.path()) {
            out.push((e.into_path(), l));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
