//! Version-bearing file discovery and lossless in-place version bumping.
//!
//! Every manifest is described by the byte ranges of its version strings, so a bump splices
//! only those bytes: line endings, indentation, key order and comments stay exactly as written.

use super::semver::Ver;
use super::Config;
use crate::code_intel::util::{ignored_dir, rel_of};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Cargo,
    CargoLock,
    Npm,
    NpmLock,
    Tauri,
    Deno,
    Pyproject,
    PyModule,
    Msbuild,
    Nuspec,
    Nbgv,
    Maven,
    Gradle,
    GradleProps,
    Inno,
    GoVersion,
    Custom,
}

impl Kind {
    pub fn stack(self) -> &'static str {
        match self {
            Kind::Cargo | Kind::CargoLock => "Rust",
            Kind::Npm | Kind::NpmLock | Kind::Deno => "Node/TS",
            Kind::Tauri => "Tauri",
            Kind::Pyproject | Kind::PyModule => "Python",
            Kind::Msbuild | Kind::Nuspec | Kind::Nbgv => ".NET",
            Kind::Maven | Kind::Gradle | Kind::GradleProps => "Java/Kotlin",
            Kind::Inno => "Installer",
            Kind::GoVersion => "Go",
            Kind::Custom => "Custom",
        }
    }
}

/// How the new version is written into a span.
#[derive(Clone, Copy, Debug)]
pub enum Style {
    /// `1.2.3-rc.1`
    Full,
    /// `1.2.3` (no prerelease: MSBuild `VersionPrefix`)
    Core,
    /// `1.2.3` or `1.2.3.0`, matching the part count already there (`AssemblyVersion`)
    Asm,
    /// `1.2-rc.1` (Nerdbank.GitVersioning `version.json`)
    Nbgv,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

#[derive(Clone, Serialize)]
pub struct Manifest {
    pub path: String,
    pub kind: Kind,
    pub stack: &'static str,
    pub version: Option<String>,
    pub note: Option<String>,
    /// Crate name (Cargo.toml), used to keep `Cargo.lock` in step.
    #[serde(skip)]
    pub name: Option<String>,
    #[serde(skip)]
    pub spans: Vec<Span>,
    #[serde(skip)]
    pub text: String,
}

#[derive(Serialize)]
pub struct Change {
    pub line: usize,
    pub before: String,
    pub after: String,
}

#[derive(Serialize)]
pub struct FileChange {
    pub path: String,
    pub kind: Kind,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<Change>,
    #[serde(skip)]
    pub old: String,
    #[serde(skip)]
    pub new: String,
}

// ---------------------------------------------------------------------------------------------
// Scanners: text -> spans
// ---------------------------------------------------------------------------------------------

struct Json<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Json<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }

    /// Inner range of a string literal (cursor on the opening quote).
    fn string(&mut self) -> Option<Range<usize>> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        let start = self.i + 1;
        let mut j = start;
        while j < self.b.len() {
            match self.b[j] {
                b'\\' => j += 2,
                b'"' => {
                    self.i = j + 1;
                    return Some(start..j);
                }
                _ => j += 1,
            }
        }
        None
    }

    fn skip_value(&mut self) -> Option<()> {
        self.ws();
        match *self.b.get(self.i)? {
            b'"' => self.string().map(|_| ()),
            open @ (b'{' | b'[') => {
                let close = if open == b'{' { b'}' } else { b']' };
                self.i += 1;
                loop {
                    self.ws();
                    if *self.b.get(self.i)? == close {
                        self.i += 1;
                        return Some(());
                    }
                    if open == b'{' {
                        self.string()?;
                        self.ws();
                        if *self.b.get(self.i)? != b':' {
                            return None;
                        }
                        self.i += 1;
                    }
                    self.skip_value()?;
                    self.ws();
                    if *self.b.get(self.i)? == b',' {
                        self.i += 1;
                    }
                }
            }
            _ => {
                while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']') {
                    self.i += 1;
                }
                Some(())
            }
        }
    }

    /// Range of the string value at `path` inside the object at the cursor.
    fn find(&mut self, path: &[&str]) -> Option<Range<usize>> {
        self.ws();
        if *self.b.get(self.i)? != b'{' {
            return None;
        }
        self.i += 1;
        loop {
            self.ws();
            if *self.b.get(self.i)? == b'}' {
                return None;
            }
            let key = self.string()?;
            self.ws();
            if *self.b.get(self.i)? != b':' {
                return None;
            }
            self.i += 1;
            self.ws();
            if &self.b[key.clone()] == path[0].as_bytes() {
                return if path.len() == 1 { self.string() } else { self.find(&path[1..]) };
            }
            self.skip_value()?;
            self.ws();
            if *self.b.get(self.i)? == b',' {
                self.i += 1;
            }
        }
    }
}

fn json_find(text: &str, path: &[&str]) -> Option<Range<usize>> {
    let start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    Json { b: text.as_bytes(), i: start }.find(path)
}

/// First `version = "x"` line inside each of `sections`. Also returns `name` of the first section.
fn toml_spans(text: &str, sections: &[&str]) -> (Vec<Span>, Option<String>) {
    let ver = lazy_re!(r#"^\s*version\s*=\s*["']([^"']*)["']"#);
    let name_re = lazy_re!(r#"^\s*name\s*=\s*["']([^"']*)["']"#);
    let mut spans = Vec::new();
    let mut name = None;
    let (mut section, mut done) = (String::new(), HashSet::new());
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.trim_matches(|c| c == '[' || c == ']').trim().to_string();
        } else if sections.contains(&section.as_str()) {
            if name.is_none() && section == sections[0] {
                name = name_re.captures(line).map(|c| c[1].to_string());
            }
            if !done.contains(&section) {
                if let Some(m) = ver.captures(line).and_then(|c| c.get(1)) {
                    spans.push(Span { range: off + m.start()..off + m.end(), style: Style::Full });
                    done.insert(section.clone());
                }
            }
        }
        off += line.len();
    }
    (spans, name)
}

/// `[[package]]` blocks of a Cargo.lock for workspace members (no `source`) named in `names`.
fn lock_spans(text: &str, names: &HashSet<String>) -> Vec<Span> {
    let name_re = lazy_re!(r#"^name\s*=\s*"([^"]*)""#);
    let ver_re = lazy_re!(r#"^version\s*=\s*"([^"]*)""#);
    let mut out = Vec::new();
    let (mut in_pkg, mut hit, mut sourced, mut span): (bool, bool, bool, Option<Range<usize>>) =
        (false, false, false, None);
    let flush = |hit: bool, sourced: bool, span: &mut Option<Range<usize>>, out: &mut Vec<Span>| {
        if let (true, false, Some(r)) = (hit, sourced, span.take()) {
            out.push(Span { range: r, style: Style::Full });
        }
        *span = None;
    };
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end();
        if t == "[[package]]" {
            flush(hit, sourced, &mut span, &mut out);
            (in_pkg, hit, sourced) = (true, false, false);
        } else if t.starts_with('[') {
            flush(hit, sourced, &mut span, &mut out);
            in_pkg = false;
        } else if in_pkg {
            if let Some(c) = name_re.captures(t) {
                hit = names.contains(&c[1]);
            } else if let Some(m) = ver_re.captures(t).and_then(|c| c.get(1)) {
                span = Some(off + m.start()..off + m.end());
            } else if t.starts_with("source") {
                sourced = true;
            }
        }
        off += line.len();
    }
    flush(hit, sourced, &mut span, &mut out);
    out
}

/// Same length as `text`, with `<!-- ... -->` blanked so offsets still line up.
fn mask_xml_comments(text: &str) -> String {
    let re = lazy_re!(r"(?s)<!--.*?-->");
    let mut out = text.to_string();
    for m in re.find_iter(text) {
        let blank: String = text[m.range()].chars().map(|c| if c == '\n' { '\n' } else { ' ' }).collect();
        out.replace_range(m.range(), &blank);
    }
    out
}

fn regex_spans(re: &Regex, text: &str, style: Style) -> Vec<Span> {
    re.captures_iter(text)
        .filter_map(|c| c.get(1))
        .filter(|m| !m.as_str().contains("$("))
        .map(|m| Span { range: m.range(), style })
        .collect()
}

fn msbuild_spans(text: &str) -> Vec<Span> {
    let masked = mask_xml_comments(text);
    let mut spans = Vec::new();
    for (tag, style) in [
        ("Version", Style::Full),
        ("VersionPrefix", Style::Core),
        ("PackageVersion", Style::Full),
        ("AssemblyVersion", Style::Asm),
        ("FileVersion", Style::Asm),
        ("InformationalVersion", Style::Full),
    ] {
        let re = Regex::new(&format!(r"<{tag}>\s*([^<\s][^<]*?)\s*</{tag}>")).unwrap();
        spans.extend(
            regex_spans(&re, &masked, style)
                .into_iter()
                .filter(|s| !masked[s.range.clone()].contains(['$', '+'])),
        );
    }
    spans.sort_by_key(|s| s.range.start);
    spans
}

/// Project-level `<version>` of a pom.xml (not the parent's, a dependency's or a plugin's).
fn maven_spans(text: &str) -> Vec<Span> {
    let masked = mask_xml_comments(text);
    let tag = lazy_re!(r"<(/?)([A-Za-z_][\w.\-:]*)[^<>]*?(/?)>");
    let mut stack: Vec<String> = Vec::new();
    for c in tag.captures_iter(&masked) {
        let whole = c.get(0).unwrap();
        let (closing, name, selfclosing) = (!c[1].is_empty(), c[2].to_string(), !c[3].is_empty());
        if closing {
            stack.pop();
        } else if !selfclosing {
            if name == "version" && stack.len() == 1 {
                let end = masked[whole.end()..].find('<').map_or(masked.len(), |i| whole.end() + i);
                let value = masked[whole.end()..end].trim();
                if value.is_empty() || value.contains("${") {
                    return Vec::new();
                }
                let lead = masked[whole.end()..end].len() - masked[whole.end()..end].trim_start().len();
                return vec![Span { range: whole.end() + lead..whole.end() + lead + value.len(), style: Style::Full }];
            }
            stack.push(name);
        }
    }
    Vec::new()
}

fn spans_for(kind: Kind, name: &str, text: &str) -> (Vec<Span>, Option<String>) {
    let full = |r: Range<usize>| Span { range: r, style: Style::Full };
    let json = |path: &[&str]| json_find(text, path).map(full).into_iter().collect::<Vec<_>>();
    let spans = match kind {
        Kind::Cargo => return toml_spans(text, &["package", "workspace.package"]),
        Kind::Pyproject => return toml_spans(text, &["project", "tool.poetry"]),
        Kind::Npm | Kind::Deno => json(&["version"]),
        Kind::NpmLock => {
            let mut v = json(&["version"]);
            v.extend(json(&["packages", "", "version"]));
            v
        }
        Kind::Tauri => {
            let v = json(&["version"]);
            if v.is_empty() { json(&["package", "version"]) } else { v }
        }
        Kind::Nbgv => json_find(text, &["version"]).map(|r| Span { range: r, style: Style::Nbgv }).into_iter().collect(),
        Kind::PyModule => regex_spans(lazy_re!(r#"(?m)^__version__\s*(?::\s*\w+\s*)?=\s*["']([^"']+)["']"#), text, Style::Full),
        Kind::Msbuild => msbuild_spans(text),
        Kind::Nuspec => regex_spans(lazy_re!(r"(?s)<version>\s*([^<$\s]+)\s*</version>"), &mask_xml_comments(text), Style::Full)
            .into_iter()
            .take(1)
            .collect(),
        Kind::Maven => maven_spans(text),
        Kind::Gradle => regex_spans(lazy_re!(r#"(?m)^version\s*=\s*["']([^"'$]+)["']"#), text, Style::Full),
        Kind::GradleProps => regex_spans(lazy_re!(r"(?m)^version\s*=\s*([^\s#]+)"), text, Style::Full),
        Kind::Inno => regex_spans(lazy_re!(r#"(?m)^\s*#define\s+(?:My)?App(?:lication)?Version\s+"([^"]+)""#), text, Style::Full)
            .into_iter()
            // `0.0.0` is the "set me from the command line" sentinel, not a real version.
            .filter(|s| &text[s.range.clone()] != "0.0.0")
            .collect(),
        Kind::GoVersion => regex_spans(lazy_re!(r#"(?m)^\s*(?:const\s+|var\s+)?[Vv]ersion\s*=\s*"v?(\d+\.\d+\.\d+[^"]*)""#), text, Style::Full),
        Kind::CargoLock | Kind::Custom => Vec::new(),
    };
    let _ = name;
    let note = (kind == Kind::Msbuild && spans.is_empty() && (text.contains("MinVer") || text.contains("Nerdbank.GitVersioning")))
        .then(|| "tag-driven version (MinVer/Nerdbank.GitVersioning): nothing to edit".to_string());
    (spans, note)
}

fn classify(name: &str) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "cargo.toml" => Kind::Cargo,
        "cargo.lock" => Kind::CargoLock,
        "package.json" => Kind::Npm,
        "package-lock.json" => Kind::NpmLock,
        "tauri.conf.json" => Kind::Tauri,
        "deno.json" | "jsr.json" => Kind::Deno,
        "pyproject.toml" => Kind::Pyproject,
        "__init__.py" | "__about__.py" | "version.py" => Kind::PyModule,
        "directory.build.props" => Kind::Msbuild,
        "version.json" => Kind::Nbgv,
        "pom.xml" => Kind::Maven,
        "build.gradle" | "build.gradle.kts" => Kind::Gradle,
        "gradle.properties" => Kind::GradleProps,
        "version.go" => Kind::GoVersion,
        _ if lower.ends_with(".csproj") || lower.ends_with(".fsproj") || lower.ends_with(".vbproj") => Kind::Msbuild,
        _ if lower.ends_with(".nuspec") => Kind::Nuspec,
        _ if lower.ends_with(".iss") => Kind::Inno,
        _ => return None,
    })
}

fn cargo_inherits_workspace_version(text: &str) -> bool {
    let re = lazy_re!(r#"^\s*version(?:\s*\.\s*workspace\s*=\s*true|\s*=\s*\{[^}]*\bworkspace\s*=\s*true\b)"#);
    let mut in_pkg = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[package]";
        } else if in_pkg && re.is_match(t) {
            return true;
        }
    }
    false
}

fn make(path: String, kind: Kind, text: String) -> Option<Manifest> {
    let base = path.rsplit('/').next().unwrap_or(&path).to_string();
    let (spans, extra) = spans_for(kind, &base, &text);
    let (spans, name) = if kind == Kind::Cargo { (spans, extra.clone()) } else { (spans, None) };
    let note = if kind == Kind::Cargo {
        (spans.is_empty() && name.is_some() && cargo_inherits_workspace_version(&text))
            .then(|| "inherits workspace.package.version".to_string())
    } else {
        extra
    };
    if spans.is_empty() && note.is_none() && kind != Kind::CargoLock {
        return None;
    }
    let version = spans.first().map(|s| text[s.range.clone()].to_string());
    Some(Manifest { stack: kind.stack(), path, kind, version, note, name, spans, text })
}

/// Walk the workspace (≤ 5 levels, honoring .gitignore) and describe every version-bearing file.
pub fn scan(ws: &Path, cfg: &Config) -> Vec<Manifest> {
    let walk = ignore::WalkBuilder::new(ws)
        .max_depth(Some(5))
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| {
            !(e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) && ignored_dir(&e.file_name().to_string_lossy()))
        })
        .build();
    let mut found: BTreeMap<String, Manifest> = BTreeMap::new();
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(kind) = entry.file_name().to_str().and_then(classify) else { continue };
        let rel = rel_of(ws, entry.path());
        if cfg.is_excluded(&rel) || entry.metadata().map_or(true, |m| m.len() > 8 * 1024 * 1024) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        if let Some(m) = make(rel.clone(), kind, text) {
            found.insert(rel, m);
        }
    }
    for f in &cfg.files {
        let rel = f.path.replace('\\', "/");
        let (Ok(re), Ok(text)) = (Regex::new(&f.pattern), std::fs::read_to_string(ws.join(&rel))) else { continue };
        let spans = regex_spans(&re, &text, Style::Full);
        if spans.is_empty() {
            continue;
        }
        let version = Some(text[spans[0].range.clone()].to_string());
        found.insert(rel.clone(), Manifest { path: rel, kind: Kind::Custom, stack: "Custom", version, note: None, name: None, spans, text });
    }
    let mut all: Vec<Manifest> = found.into_values().collect();
    // Cargo.lock: show the version its workspace members carry.
    let crates: Vec<(String, String)> =
        all.iter().filter(|m| m.kind == Kind::Cargo).filter_map(|m| Some((m.path.clone(), m.name.clone()?))).collect();
    for lock in all.iter_mut().filter(|m| m.kind == Kind::CargoLock) {
        let names = lock_members(&lock.path, &crates);
        lock.spans = lock_spans(&lock.text, &names);
        lock.version = lock.spans.first().map(|s| lock.text[s.range.clone()].to_string());
    }
    all.retain(|m| m.kind != Kind::CargoLock || !m.spans.is_empty());
    all
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn within(dir: &str, path: &str) -> bool {
    dir.is_empty() || dir_of(path) == dir || dir_of(path).starts_with(&format!("{dir}/"))
}

/// Crate names whose nearest `Cargo.lock` is `lock_path`.
fn lock_members(lock_path: &str, crates: &[(String, String)]) -> HashSet<String> {
    let dir = dir_of(lock_path);
    crates.iter().filter(|(p, _)| within(dir, p)).map(|(_, n)| n.clone()).collect()
}

// ---------------------------------------------------------------------------------------------
// Bumping
// ---------------------------------------------------------------------------------------------

pub fn render(style: Style, old: &str, new: &Ver) -> String {
    match style {
        Style::Full => new.to_string(),
        Style::Core => new.core(),
        Style::Asm if old.split('.').count() == 4 => format!("{}.0", new.core()),
        Style::Asm => new.core(),
        Style::Nbgv => format!("{}.{}{}", new.major, new.minor, new.pre.as_ref().map(|p| format!("-{p}")).unwrap_or_default()),
    }
}

fn line_at(text: &str, pos: usize) -> (usize, &str) {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    (text[..pos].matches('\n').count() + 1, text[start..end].trim())
}

fn splice(m: &Manifest, spans: &[Span], new: &Ver) -> Option<FileChange> {
    let mut spans: Vec<&Span> = spans.iter().collect();
    spans.sort_by_key(|s| s.range.start);
    let (mut out, mut last, mut delta) = (String::with_capacity(m.text.len()), 0usize, 0isize);
    let mut places: Vec<(usize, usize)> = Vec::new(); // (old offset, new offset)
    for s in &spans {
        let old = &m.text[s.range.clone()];
        let rendered = render(s.style, old, new);
        out.push_str(&m.text[last..s.range.start]);
        places.push((s.range.start, (s.range.start as isize + delta) as usize));
        out.push_str(&rendered);
        delta += rendered.len() as isize - old.len() as isize;
        last = s.range.end;
    }
    out.push_str(&m.text[last..]);
    if out == m.text {
        return None;
    }
    let changes = places
        .iter()
        .map(|&(o, n)| Change { line: line_at(&m.text, o).0, before: line_at(&m.text, o).1.to_string(), after: line_at(&out, n).1.to_string() })
        .filter(|c| c.before != c.after)
        .collect();
    Some(FileChange {
        path: m.path.clone(),
        kind: m.kind,
        from: m.version.clone(),
        to: new.to_string(),
        changes,
        old: m.text.clone(),
        new: out,
    })
}

/// Changes needed to move the selected manifests (all when `only` is empty) to `new`.
/// Lockfiles follow their manifest: `Cargo.lock` for the bumped crates, `package-lock.json`
/// next to a bumped `package.json`.
pub fn plan(manifests: &[Manifest], only: &[String], new: &Ver) -> Vec<FileChange> {
    let only: Vec<String> = only.iter().map(|p| p.replace('\\', "/").trim_start_matches("./").to_string()).collect();
    let picked = |m: &Manifest| only.is_empty() || only.iter().any(|o| o == &m.path);
    let chosen: Vec<&Manifest> =
        manifests.iter().filter(|m| !matches!(m.kind, Kind::CargoLock | Kind::NpmLock) && picked(m)).collect();
    let crates: Vec<(String, String)> = chosen
        .iter()
        .filter(|m| m.kind == Kind::Cargo)
        .filter_map(|m| Some((m.path.clone(), m.name.clone()?)))
        .collect();
    let all_crates: Vec<(String, String)> = manifests
        .iter()
        .filter(|m| m.kind == Kind::Cargo)
        .filter_map(|m| Some((m.path.clone(), m.name.clone()?)))
        .collect();
    let ws_bumped_dirs: Vec<&str> = chosen
        .iter()
        .filter(|m| m.kind == Kind::Cargo && m.text.contains("[workspace.package]"))
        .map(|m| dir_of(&m.path))
        .collect();
    let mut out: Vec<FileChange> = chosen.iter().filter_map(|m| splice(m, &m.spans, new)).collect();
    for m in manifests {
        let spans = match m.kind {
            Kind::CargoLock => {
                // Only crates whose nearest lock is this one, and only those being bumped
                // (including workspace members that inherit `workspace.package.version`).
                let owned = lock_members(&m.path, &all_crates);
                let mut names: HashSet<String> =
                    crates.iter().filter(|(_, n)| owned.contains(n)).map(|(_, n)| n.clone()).collect();
                for member in manifests.iter().filter(|x| {
                    x.kind == Kind::Cargo
                        && x.spans.is_empty()
                        && x.note.as_deref() == Some("inherits workspace.package.version")
                        && ws_bumped_dirs.iter().any(|d| within(d, &x.path))
                }) {
                    if let Some(n) = &member.name {
                        if owned.contains(n) {
                            names.insert(n.clone());
                        }
                    }
                }
                lock_spans(&m.text, &names)
            }
            Kind::NpmLock => {
                let sibling = format!("{}{}package.json", dir_of(&m.path), if dir_of(&m.path).is_empty() { "" } else { "/" });
                if chosen.iter().any(|c| c.path == sibling) { m.spans.clone() } else { Vec::new() }
            }
            _ => continue,
        };
        if let Some(c) = splice(m, &spans, new) {
            out.push(c);
        }
    }
    out
}

/// Writes every change, restoring the already-written files if one write fails.
pub fn write_all(ws: &Path, changes: &[FileChange]) -> Result<(), String> {
    let mut done: Vec<&FileChange> = Vec::new();
    for c in changes {
        let p: PathBuf = ws.join(&c.path);
        if let Err(e) = std::fs::write(&p, &c.new) {
            for d in done {
                let _ = std::fs::write(ws.join(&d.path), &d.old);
            }
            return Err(format!("could not write {}: {e}. Nothing was changed.", c.path));
        }
        done.push(c);
    }
    Ok(())
}

pub fn restore(ws: &Path, changes: &[FileChange]) {
    for c in changes {
        let _ = std::fs::write(ws.join(&c.path), &c.old);
    }
}
