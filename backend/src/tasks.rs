//! Run-task discovery: scans the workspace (root + shallow sub-packages) for runnable
//! tasks across ecosystems, classifies and ranks them, and merges in user-defined tasks
//! from `.vireo/tasks.json`.

use crate::state::{is_ignored_dir_name, resolve_safe_path, AppState};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_DIRS: usize = 300;
const MAX_DEPTH: usize = 2;
const MAX_PER_SOURCE: usize = 25;
const MAX_TASKS: usize = 200;
const CUSTOM_FILE: &str = ".vireo/tasks.json";
const CUSTOM_PREFIX: &str = "custom:";

#[derive(Serialize, Clone, Default)]
pub struct TaskItem {
    /// Stable id (`pkg:frontend:dev`, `custom:my-task`, ...). Used for pins, history and tab reuse.
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    /// Working directory relative to the workspace root ("" = root).
    pub cwd: String,
    /// run | build | test | check | other
    pub category: String,
    /// Higher is more likely to be the primary run target.
    pub score: i32,
    pub custom: bool,
    pub env: BTreeMap<String, String>,
    pub depends_on: Vec<String>,
}

#[derive(Serialize)]
pub struct TasksRes {
    pub tasks: Vec<TaskItem>,
    pub default_task: Option<String>,
}

struct Dir {
    abs: PathBuf,
    rel: String,
}

impl Dir {
    fn label(&self, fallback: &str) -> String {
        if self.rel.is_empty() {
            fallback.to_string()
        } else {
            self.rel.rsplit('/').next().unwrap_or(&self.rel).to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Classification & scoring
// ---------------------------------------------------------------------------

fn classify(name: &str, command: &str) -> &'static str {
    const TEST: &[&str] = &[
        "test", "tests", "pytest", "vitest", "jest", "e2e", "cypress", "playwright", "spec",
    ];
    const CHECK: &[&str] = &[
        "check", "lint", "typecheck", "tsc", "clippy", "fmt", "format", "eslint", "prettier",
    ];
    const RUN: &[&str] = &[
        "dev", "start", "serve", "watch", "run", "up", "preview", "launch", "runserver",
    ];
    const BUILD: &[&str] = &["build", "compile", "bundle", "package", "release", "dist"];

    let tokens: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let has = |set: &[&str]| tokens.iter().any(|t| set.contains(&t.as_str()));
    if has(TEST) {
        "test"
    } else if has(CHECK) {
        "check"
    } else if has(RUN) {
        "run"
    } else if has(BUILD) {
        "build"
    } else {
        let c = command.to_lowercase();
        if c.starts_with("cargo run") || c.contains("compose up") {
            "run"
        } else {
            "other"
        }
    }
}

fn base_score(category: &str, rel: &str) -> i32 {
    let intent = match category {
        "run" => 80,
        "build" => 50,
        "test" => 40,
        "check" => 20,
        _ => 10,
    };
    // Tasks at the workspace root are more likely to be the "main" thing.
    intent + if rel.is_empty() { 10 } else { 0 }
}

fn make_task(
    id: String,
    name: String,
    command: String,
    source: &str,
    dir: &Dir,
    category: Option<&str>,
    bonus: i32,
) -> TaskItem {
    let category = category
        .map(str::to_string)
        .unwrap_or_else(|| classify(&name, &command).to_string());
    let score = base_score(&category, &dir.rel) + bonus;
    TaskItem {
        id,
        name,
        command,
        source: source.to_string(),
        cwd: dir.rel.clone(),
        category,
        score,
        ..Default::default()
    }
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '/' | '@'))
}

// ---------------------------------------------------------------------------
// Directory scan
// ---------------------------------------------------------------------------

fn collect_dirs(ws: &Path) -> Vec<Dir> {
    let mut out = vec![Dir {
        abs: ws.to_path_buf(),
        rel: String::new(),
    }];
    let mut frontier = vec![0usize];
    for depth in 1..=MAX_DEPTH {
        let mut next = Vec::new();
        for idx in frontier {
            let (parent_abs, parent_rel) = (out[idx].abs.clone(), out[idx].rel.clone());
            let Ok(rd) = std::fs::read_dir(&parent_abs) else {
                continue;
            };
            let mut names: Vec<String> = rd
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.') && !is_ignored_dir_name(n))
                .filter(|n| !matches!(n.as_str(), "vendor" | "out" | "coverage" | "site-packages"))
                .collect();
            names.sort();
            for n in names {
                if out.len() >= MAX_DIRS {
                    return out;
                }
                let rel = if parent_rel.is_empty() {
                    n.clone()
                } else {
                    format!("{}/{}", parent_rel, n)
                };
                out.push(Dir {
                    abs: parent_abs.join(&n),
                    rel,
                });
                if depth < MAX_DEPTH {
                    next.push(out.len() - 1);
                }
            }
        }
        frontier = next;
    }
    out
}

/// True when `file` exists in `dir` or any ancestor up to (and including) the workspace root.
fn walk_up_has(ws: &Path, rel: &str, files: &[&str]) -> Option<String> {
    let mut parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    loop {
        let mut p = ws.to_path_buf();
        for part in &parts {
            p.push(part);
        }
        for f in files {
            if p.join(f).is_file() {
                return Some((*f).to_string());
            }
        }
        if parts.pop().is_none() {
            return None;
        }
    }
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// Extractors
// ---------------------------------------------------------------------------

fn node_pm(ws: &Path, dir: &Dir, pkg: &serde_json::Value) -> &'static str {
    if let Some(pm) = pkg.get("packageManager").and_then(|v| v.as_str()) {
        for name in ["pnpm", "yarn", "bun", "npm"] {
            if pm.starts_with(name) {
                return name;
            }
        }
    }
    match walk_up_has(
        ws,
        &dir.rel,
        &["bun.lock", "bun.lockb", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"],
    )
    .as_deref()
    {
        Some("bun.lock") | Some("bun.lockb") => "bun",
        Some("pnpm-lock.yaml") => "pnpm",
        Some("yarn.lock") => "yarn",
        _ => "npm",
    }
}

fn is_lifecycle(name: &str, scripts: &serde_json::Map<String, serde_json::Value>) -> bool {
    if matches!(
        name,
        "install" | "preinstall" | "postinstall" | "prepare" | "prepublish" | "prepublishOnly" | "prepack" | "postpack" | "publish"
    ) {
        return true;
    }
    for prefix in ["pre", "post"] {
        if let Some(base) = name.strip_prefix(prefix) {
            if !base.is_empty() && scripts.contains_key(base) {
                return true;
            }
        }
    }
    false
}

fn extract_node(ws: &Path, dir: &Dir, out: &mut Vec<TaskItem>) {
    let manifest = if dir.abs.join("package.json").is_file() {
        ("package.json", "package.json")
    } else {
        return;
    };
    let Some(raw) = read(&dir.abs.join(manifest.0)) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
        return;
    };
    let pm = node_pm(ws, dir, &json);
    let label = dir.label(pm);
    let source = if dir.rel.is_empty() {
        "package.json".to_string()
    } else {
        format!("{}/package.json", dir.rel)
    };
    for (name, _) in scripts.iter().take(MAX_PER_SOURCE * 2) {
        if !is_plain_name(name) || is_lifecycle(name, scripts) {
            continue;
        }
        let command = match pm {
            "yarn" => format!("yarn {}", name),
            other => format!("{} run {}", other, name),
        };
        out.push(make_task(
            format!("pkg:{}:{}", dir.rel, name),
            format!("{}: {}", label, name),
            command,
            &source,
            dir,
            None,
            0,
        ));
    }
}

fn extract_deno(dir: &Dir, out: &mut Vec<TaskItem>) {
    for f in ["deno.json", "deno.jsonc"] {
        let Some(raw) = read(&dir.abs.join(f)) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&raw)) else {
            continue;
        };
        if let Some(tasks) = json.get("tasks").and_then(|t| t.as_object()) {
            let label = dir.label("deno");
            for (name, _) in tasks.iter().take(MAX_PER_SOURCE) {
                if is_plain_name(name) {
                    out.push(make_task(
                        format!("deno:{}:{}", dir.rel, name),
                        format!("{}: {}", label, name),
                        format!("deno task {}", name),
                        f,
                        dir,
                        None,
                        0,
                    ));
                }
            }
        }
        return;
    }
}

/// Keys declared under a TOML `[header]` section (flat key = value lines only).
fn toml_section_keys(raw: &str, header: &str) -> Vec<String> {
    let key_re = Regex::new(r#"^\s*([A-Za-z0-9_.-]+|"[^"]+")\s*="#).unwrap();
    let mut inside = false;
    let mut keys = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == format!("[{}]", header);
            continue;
        }
        if inside {
            if let Some(c) = key_re.captures(line) {
                keys.push(c[1].trim_matches('"').to_string());
            }
        }
    }
    keys
}

fn toml_string_in_section(raw: &str, header: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"^\s*{}\s*=\s*"([^"]*)""#, regex::escape(key))).ok()?;
    let mut inside = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == format!("[{}]", header);
            continue;
        }
        if inside {
            if let Some(c) = re.captures(line) {
                return Some(c[1].to_string());
            }
        }
    }
    None
}

fn extract_cargo(dir: &Dir, out: &mut Vec<TaskItem>) {
    let Some(raw) = read(&dir.abs.join("Cargo.toml")) else {
        return;
    };
    let has_package = raw.lines().any(|l| l.trim() == "[package]");
    let has_workspace = raw.lines().any(|l| l.trim() == "[workspace]");
    if !has_package && !has_workspace {
        return;
    }
    let source = if dir.rel.is_empty() {
        "Cargo.toml".to_string()
    } else {
        format!("{}/Cargo.toml", dir.rel)
    };
    let label = dir.label("cargo");
    let id = |n: &str| format!("cargo:{}:{}", dir.rel, n);

    // Binary targets: [[bin]] entries, src/main.rs, src/bin/*.
    let mut bins: Vec<String> = Vec::new();
    if has_package {
        let pkg_name = toml_string_in_section(&raw, "package", "name");
        let mut in_bin = false;
        let name_re = Regex::new(r#"^\s*name\s*=\s*"([^"]*)""#).unwrap();
        for line in raw.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_bin = t == "[[bin]]";
                continue;
            }
            if in_bin {
                if let Some(c) = name_re.captures(line) {
                    bins.push(c[1].to_string());
                }
            }
        }
        if dir.abs.join("src/main.rs").is_file() {
            if let Some(n) = pkg_name {
                bins.push(n);
            }
        }
        if let Ok(rd) = std::fs::read_dir(dir.abs.join("src/bin")) {
            for e in rd.flatten() {
                let p = e.path();
                let name = if p.is_dir() && p.join("main.rs").is_file() {
                    p.file_name().and_then(|s| s.to_str()).map(str::to_string)
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
                } else {
                    None
                };
                bins.extend(name);
            }
        }
        bins.sort();
        bins.dedup();
    }
    let default_run = toml_string_in_section(&raw, "package", "default-run");

    if bins.len() == 1 || default_run.is_some() {
        out.push(make_task(
            id("run"),
            format!("{}: cargo run", label),
            "cargo run".into(),
            &source,
            dir,
            Some("run"),
            0,
        ));
    }
    if bins.len() > 1 {
        for b in bins.iter().take(8) {
            out.push(make_task(
                id(&format!("run:{}", b)),
                format!("{}: cargo run --bin {}", label, b),
                format!("cargo run --bin {}", b),
                &source,
                dir,
                Some("run"),
                if default_run.as_deref() == Some(b) { 5 } else { 0 },
            ));
        }
    }
    for (n, cmd, cat) in [
        ("build", "cargo build", "build"),
        ("check", "cargo check", "check"),
        ("test", "cargo test", "test"),
        ("clippy", "cargo clippy", "check"),
    ] {
        out.push(make_task(
            id(n),
            format!("{}: {}", label, cmd),
            cmd.into(),
            &source,
            dir,
            Some(cat),
            0,
        ));
    }
}

fn extract_python(ws: &Path, dir: &Dir, out: &mut Vec<TaskItem>) {
    let pyproject = read(&dir.abs.join("pyproject.toml"));
    let has_pytest_ini = dir.abs.join("pytest.ini").is_file();
    let has_manage = dir.abs.join("manage.py").is_file();
    if pyproject.is_none() && !has_pytest_ini && !has_manage {
        return;
    }
    let prefix = match walk_up_has(ws, &dir.rel, &["uv.lock", "poetry.lock", "Pipfile.lock"]).as_deref() {
        Some("uv.lock") => "uv run ",
        Some("poetry.lock") => "poetry run ",
        Some("Pipfile.lock") => "pipenv run ",
        _ => "",
    };
    let source = if pyproject.is_some() { "pyproject.toml" } else { "python" };
    let label = dir.label("python");
    let id = |n: &str| format!("py:{}:{}", dir.rel, n);

    if has_manage {
        out.push(make_task(
            id("runserver"),
            format!("{}: manage.py runserver", label),
            format!("{}python manage.py runserver", prefix),
            "manage.py",
            dir,
            Some("run"),
            0,
        ));
    }
    if let Some(raw) = &pyproject {
        let mut scripts = toml_section_keys(raw, "project.scripts");
        scripts.extend(toml_section_keys(raw, "tool.poetry.scripts"));
        scripts.extend(toml_section_keys(raw, "tool.poe.tasks"));
        let poe = !toml_section_keys(raw, "tool.poe.tasks").is_empty();
        let n_poe = toml_section_keys(raw, "tool.poe.tasks");
        for s in scripts.iter().take(MAX_PER_SOURCE) {
            if !is_plain_name(s) {
                continue;
            }
            let command = if poe && n_poe.contains(s) {
                format!("{}poe {}", prefix, s)
            } else {
                format!("{}{}", prefix, s)
            };
            out.push(make_task(
                id(s),
                format!("{}: {}", label, s),
                command,
                "pyproject.toml",
                dir,
                None,
                0,
            ));
        }
    }
    out.push(make_task(
        id("pytest"),
        if dir.rel.is_empty() { "pytest".into() } else { format!("{}: pytest", label) },
        format!("{}pytest", prefix),
        source,
        dir,
        Some("test"),
        0,
    ));
}

fn extract_go(dir: &Dir, out: &mut Vec<TaskItem>) {
    if !dir.abs.join("go.mod").is_file() {
        return;
    }
    let label = dir.label("go");
    let id = |n: &str| format!("go:{}:{}", dir.rel, n);
    if dir.abs.join("main.go").is_file() {
        out.push(make_task(id("run"), format!("{}: go run .", label), "go run .".into(), "go.mod", dir, Some("run"), 0));
    }
    if let Ok(rd) = std::fs::read_dir(dir.abs.join("cmd")) {
        for e in rd.flatten().take(8) {
            if e.path().join("main.go").is_file() {
                if let Some(n) = e.file_name().to_str() {
                    out.push(make_task(
                        id(&format!("run:{}", n)),
                        format!("{}: go run ./cmd/{}", label, n),
                        format!("go run ./cmd/{}", n),
                        "go.mod",
                        dir,
                        Some("run"),
                        0,
                    ));
                }
            }
        }
    }
    out.push(make_task(id("build"), format!("{}: go build", label), "go build ./...".into(), "go.mod", dir, Some("build"), 0));
    out.push(make_task(id("test"), format!("{}: go test", label), "go test ./...".into(), "go.mod", dir, Some("test"), 0));
}

fn extract_make_like(dir: &Dir, out: &mut Vec<TaskItem>) {
    let label = dir.label("make");
    let name_re_make = Regex::new(r"(?m)^([A-Za-z0-9][A-Za-z0-9_-]*)\s*:([^=]|$)").unwrap();
    let name_re_just = Regex::new(r"(?m)^([A-Za-z_][A-Za-z0-9_-]*)\s*:([^=]|$)").unwrap();
    for (file, tool, re) in [
        ("Makefile", "make", &name_re_make),
        ("makefile", "make", &name_re_make),
        ("GNUmakefile", "make", &name_re_make),
        ("justfile", "just", &name_re_just),
        ("Justfile", "just", &name_re_just),
    ] {
        if let Some(raw) = read(&dir.abs.join(file)) {
            let mut seen = HashSet::new();
            for c in re.captures_iter(&raw).take(MAX_PER_SOURCE * 2) {
                let n = c[1].to_string();
                if n.starts_with('.') || !seen.insert(n.clone()) || seen.len() > MAX_PER_SOURCE {
                    continue;
                }
                out.push(make_task(
                    format!("{}:{}:{}", tool, dir.rel, n),
                    format!("{}: {} {}", label, tool, n),
                    format!("{} {}", tool, n),
                    file,
                    dir,
                    None,
                    0,
                ));
            }
            return;
        }
    }
    for file in ["Taskfile.yml", "Taskfile.yaml"] {
        if let Some(raw) = read(&dir.abs.join(file)) {
            let key_re = Regex::new(r"^  ([A-Za-z0-9_:-]+):").unwrap();
            let mut inside = false;
            let mut count = 0;
            for line in raw.lines() {
                if line.starts_with("tasks:") {
                    inside = true;
                    continue;
                }
                if inside && !line.starts_with(' ') && !line.trim().is_empty() && !line.starts_with('#') {
                    inside = false;
                }
                if inside {
                    if let Some(c) = key_re.captures(line) {
                        if count >= MAX_PER_SOURCE {
                            break;
                        }
                        count += 1;
                        out.push(make_task(
                            format!("task:{}:{}", dir.rel, &c[1]),
                            format!("{}: task {}", label, &c[1]),
                            format!("task {}", &c[1]),
                            file,
                            dir,
                            None,
                            0,
                        ));
                    }
                }
            }
            return;
        }
    }
}

fn extract_compose(dir: &Dir, out: &mut Vec<TaskItem>) {
    for f in ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"] {
        if dir.abs.join(f).is_file() {
            let label = dir.label("docker");
            out.push(make_task(
                format!("compose:{}:up", dir.rel),
                format!("{}: docker compose up", label),
                "docker compose up".into(),
                f,
                dir,
                Some("run"),
                if dir.rel.is_empty() { 40 } else { 0 },
            ));
            out.push(make_task(
                format!("compose:{}:up-build", dir.rel),
                format!("{}: docker compose up --build", label),
                "docker compose up --build".into(),
                f,
                dir,
                Some("run"),
                if dir.rel.is_empty() { 30 } else { 0 },
            ));
            return;
        }
    }
}

/// Top-level launcher scripts such as `dev.ps1`, `run.sh`, `build-and-run.ps1` or `<repo>.ps1`.
fn extract_launchers(ws: &Path, dir: &Dir, out: &mut Vec<TaskItem>) {
    if !dir.rel.is_empty() {
        return;
    }
    let repo = ws
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let re = Regex::new(r"^(run|dev|start|serve|launch|build-and-run)([-_.][\w.-]*)?\.(ps1|sh|bat|cmd)$").unwrap();
    let Ok(rd) = std::fs::read_dir(&dir.abs) else {
        return;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let lower = name.to_lowercase();
        let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
        let ext = lower.rsplit('.').next().unwrap_or("");
        if !(re.is_match(&lower) || (!repo.is_empty() && stem == repo && matches!(ext, "ps1" | "sh" | "bat" | "cmd"))) {
            continue;
        }
        let command = match ext {
            "ps1" if cfg!(windows) => format!("./{}", name),
            "ps1" => format!("pwsh ./{}", name),
            "sh" => format!("bash ./{}", name),
            _ if cfg!(windows) => format!("./{}", name),
            _ => continue,
        };
        // `build-and-run` style scripts are the closest thing to a one-click full launch.
        let bonus = if lower.starts_with("build-and-run") { 70 } else { 60 };
        out.push(make_task(
            format!("script::{}", name),
            name.clone(),
            command,
            "root script",
            dir,
            Some("run"),
            bonus,
        ));
    }
}

fn strip_jsonc(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    Regex::new(r",(\s*[}\]])").unwrap().replace_all(&out, "$1").into_owned()
}

fn extract_vscode(ws: &Path, out: &mut Vec<TaskItem>) {
    let Some(raw) = read(&ws.join(".vscode/tasks.json")) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&raw)) else {
        return;
    };
    let Some(tasks) = json.get("tasks").and_then(|t| t.as_array()) else {
        return;
    };
    let skip_re = Regex::new(r"\$\{(input|command|config|env):").unwrap();
    for t in tasks.iter().take(MAX_PER_SOURCE) {
        let (Some(label), Some(cmd)) = (
            t.get("label").and_then(|v| v.as_str()),
            t.get("command").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let mut command = cmd.to_string();
        if let Some(args) = t.get("args").and_then(|a| a.as_array()) {
            for a in args.iter().filter_map(|a| a.as_str()) {
                command.push(' ');
                if a.contains(' ') {
                    command.push_str(&format!("\"{}\"", a));
                } else {
                    command.push_str(a);
                }
            }
        }
        command = command.replace("${workspaceFolder}", ".");
        if skip_re.is_match(&command) {
            continue;
        }
        let cwd = t
            .pointer("/options/cwd")
            .and_then(|v| v.as_str())
            .map(|c| c.replace("${workspaceFolder}", "").trim_matches(['/', '\\']).replace('\\', "/"))
            .filter(|c| !c.contains("${") && !c.contains(".."))
            .unwrap_or_default();
        let group = t
            .get("group")
            .and_then(|g| g.as_str().or_else(|| g.get("kind").and_then(|k| k.as_str())));
        let category = match group {
            Some("build") => Some("build"),
            Some("test") => Some("test"),
            _ => None,
        };
        let dir = Dir { abs: ws.join(&cwd), rel: cwd };
        let mut item = make_task(
            format!("vscode::{}", label),
            label.to_string(),
            command,
            ".vscode/tasks.json",
            &dir,
            category,
            0,
        );
        item.score = base_score(&item.category, "") - 5;
        out.push(item);
    }
}

// ---------------------------------------------------------------------------
// Custom tasks (.vireo/tasks.json)
// ---------------------------------------------------------------------------

fn load_custom(ws: &Path) -> (Vec<TaskItem>, Option<String>) {
    let Some(raw) = read(&ws.join(CUSTOM_FILE)) else {
        return (Vec::new(), None);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&raw)) else {
        return (Vec::new(), None);
    };
    let Some(arr) = json.get("tasks").and_then(|t| t.as_array()) else {
        return (Vec::new(), None);
    };
    let raw_ids: HashSet<String> = arr
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let map_id = |id: &str| {
        if raw_ids.contains(id) {
            format!("{}{}", CUSTOM_PREFIX, id)
        } else {
            id.to_string()
        }
    };
    let mut tasks = Vec::new();
    for t in arr {
        let Some(id) = t.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
        let command = t.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let depends_on: Vec<String> = t
            .get("dependsOn")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|d| d.as_str()).map(&map_id).collect())
            .unwrap_or_default();
        if command.trim().is_empty() && depends_on.is_empty() {
            continue;
        }
        let cwd = t
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace("${workspaceFolder}", "")
            .replace('\\', "/")
            .trim_matches('/')
            .to_string();
        let category = t
            .get("category")
            .and_then(|v| v.as_str())
            .filter(|c| matches!(*c, "run" | "build" | "test" | "check" | "other"))
            .map(str::to_string)
            .unwrap_or_else(|| classify(&name, &command).to_string());
        let env = t
            .get("env")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        tasks.push(TaskItem {
            id: format!("{}{}", CUSTOM_PREFIX, id),
            name,
            command,
            source: CUSTOM_FILE.to_string(),
            cwd,
            score: base_score(&category, "") + 200,
            category,
            custom: true,
            env,
            depends_on,
        });
    }
    let default = json
        .get("defaultTask")
        .and_then(|v| v.as_str())
        .map(&map_id);
    (tasks, default)
}

fn discover(ws: &Path) -> TasksRes {
    let mut auto = Vec::new();
    for dir in collect_dirs(ws) {
        extract_launchers(ws, &dir, &mut auto);
        extract_node(ws, &dir, &mut auto);
        extract_deno(&dir, &mut auto);
        extract_cargo(&dir, &mut auto);
        extract_python(ws, &dir, &mut auto);
        extract_go(&dir, &mut auto);
        extract_make_like(&dir, &mut auto);
        extract_compose(&dir, &mut auto);
    }
    extract_vscode(ws, &mut auto);

    let (custom, default_task) = load_custom(ws);
    let mut seen = HashSet::new();
    let mut tasks: Vec<TaskItem> = custom
        .into_iter()
        .chain(auto)
        .filter(|t| seen.insert(t.id.clone()))
        .collect();
    // Stable sort keeps discovery order (root first) among equal scores.
    tasks.sort_by(|a, b| b.score.cmp(&a.score));
    tasks.truncate(MAX_TASKS);
    TasksRes { tasks, default_task }
}

pub async fn get_tasks(State(state): State<AppState>) -> Json<TasksRes> {
    let ws = state.workspace_path().await;
    let res = tokio::task::spawn_blocking(move || discover(&ws))
        .await
        .unwrap_or(TasksRes {
            tasks: Vec::new(),
            default_task: None,
        });
    Json(res)
}

#[derive(Deserialize)]
pub struct SaveTaskReq {
    pub id: Option<String>,
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
    pub category: Option<String>,
    pub env: Option<BTreeMap<String, String>>,
    /// Also make this the project's default task.
    pub make_default: Option<bool>,
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn read_custom_json(ws: &Path) -> serde_json::Value {
    read(&ws.join(CUSTOM_FILE))
        .and_then(|r| serde_json::from_str(&strip_jsonc(&r)).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({ "version": 1, "tasks": [] }))
}

async fn write_custom_json(
    state: &AppState,
    ws: &Path,
    json: &serde_json::Value,
) -> Result<(), (StatusCode, String)> {
    let full = resolve_safe_path(ws, CUSTOM_FILE).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let body = serde_json::to_string_pretty(json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tokio::fs::write(&full, format!("{}\n", body))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.notify_fs_changed(vec![CUSTOM_FILE.to_string()]);
    Ok(())
}

pub async fn save_custom_task(
    State(state): State<AppState>,
    Json(req): Json<SaveTaskReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let name = req.name.trim();
    let command = req.command.trim();
    if name.is_empty() || command.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Name and command are required".into()));
    }
    let cwd = req.cwd.as_deref().unwrap_or("").trim().replace('\\', "/");
    let cwd = cwd.trim_matches('/').to_string();
    if cwd.split('/').any(|p| p == "..") || Path::new(&cwd).is_absolute() {
        return Err((StatusCode::BAD_REQUEST, "Working directory must be inside the workspace".into()));
    }

    let ws = state.workspace_path().await;
    let mut json = read_custom_json(&ws);
    let obj = json.as_object_mut().unwrap();
    obj.entry("version").or_insert(serde_json::json!(1));
    let tasks = obj
        .entry("tasks")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or((StatusCode::CONFLICT, ".vireo/tasks.json has an invalid `tasks` field".to_string()))?;

    let existing_id = req
        .id
        .as_deref()
        .map(|i| i.strip_prefix(CUSTOM_PREFIX).unwrap_or(i).to_string())
        .filter(|i| !i.is_empty());
    let id = match existing_id {
        Some(id) => id,
        None => {
            let base = match slugify(name) {
                s if s.is_empty() => "task".to_string(),
                s => s,
            };
            let used: HashSet<&str> = tasks
                .iter()
                .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
                .collect();
            let mut candidate = base.clone();
            let mut n = 2;
            while used.contains(candidate.as_str()) {
                candidate = format!("{}-{}", base, n);
                n += 1;
            }
            candidate
        }
    };

    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), id.clone().into());
    entry.insert("name".into(), name.into());
    entry.insert("command".into(), command.into());
    if !cwd.is_empty() {
        entry.insert("cwd".into(), cwd.into());
    }
    if let Some(c) = req
        .category
        .as_deref()
        .filter(|c| matches!(*c, "run" | "build" | "test" | "check" | "other"))
    {
        entry.insert("category".into(), c.into());
    }
    if let Some(env) = req.env.filter(|e| !e.is_empty()) {
        entry.insert("env".into(), serde_json::to_value(env).unwrap_or_default());
    }
    let entry = serde_json::Value::Object(entry);
    match tasks
        .iter_mut()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        Some(slot) => *slot = entry,
        None => tasks.push(entry),
    }
    if req.make_default.unwrap_or(false) {
        obj.insert("defaultTask".into(), id.clone().into());
    }

    write_custom_json(&state, &ws, &json).await?;
    Ok(Json(serde_json::json!({ "ok": true, "id": format!("{}{}", CUSTOM_PREFIX, id) })))
}

#[derive(Deserialize)]
pub struct DeleteTaskQuery {
    pub id: String,
}

pub async fn delete_custom_task(
    State(state): State<AppState>,
    Query(q): Query<DeleteTaskQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let id = q.id.strip_prefix(CUSTOM_PREFIX).unwrap_or(&q.id).to_string();
    let ws = state.workspace_path().await;
    let mut json = read_custom_json(&ws);
    let obj = json.as_object_mut().unwrap();
    if let Some(tasks) = obj.get_mut("tasks").and_then(|t| t.as_array_mut()) {
        tasks.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
    }
    if obj.get("defaultTask").and_then(|v| v.as_str()) == Some(id.as_str()) {
        obj.remove("defaultTask");
    }
    write_custom_json(&state, &ws, &json).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("vireo-tasks-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn classifies_by_name() {
        assert_eq!(classify("dev", ""), "run");
        assert_eq!(classify("build-and-run", ""), "run");
        assert_eq!(classify("test:watch", ""), "test");
        assert_eq!(classify("lint", ""), "check");
        assert_eq!(classify("build:prod", ""), "build");
        assert_eq!(classify("clean", ""), "other");
    }

    #[test]
    fn strips_jsonc() {
        let v: serde_json::Value =
            serde_json::from_str(&strip_jsonc("{ // c\n \"a\": \"//x\", /* b */ \"b\": [1,], }")).unwrap();
        assert_eq!(v["a"], "//x");
    }

    #[test]
    fn discovers_monorepo_with_pnpm_and_skips_hooks() {
        let ws = tmp("mono");
        std::fs::write(ws.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::create_dir_all(ws.join("apps/web")).unwrap();
        std::fs::write(
            ws.join("apps/web/package.json"),
            r#"{"scripts":{"dev":"vite","build":"tsc","prebuild":"x","postinstall":"y"}}"#,
        )
        .unwrap();
        let res = discover(&ws);
        let ids: Vec<&str> = res.tasks.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"pkg:apps/web:dev"));
        assert!(!ids.contains(&"pkg:apps/web:prebuild"));
        assert!(!ids.contains(&"pkg:apps/web:postinstall"));
        let dev = res.tasks.iter().find(|t| t.id == "pkg:apps/web:dev").unwrap();
        assert_eq!(dev.command, "pnpm run dev");
        assert_eq!(dev.cwd, "apps/web");
        assert_eq!(res.tasks[0].id, "pkg:apps/web:dev");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn custom_tasks_win_and_resolve_depends_on() {
        let ws = tmp("custom");
        std::fs::create_dir_all(ws.join(".vireo")).unwrap();
        std::fs::write(
            ws.join(".vireo/tasks.json"),
            r#"{"defaultTask":"a","tasks":[{"id":"a","name":"A","command":"echo a","cwd":"${workspaceFolder}/sub"},{"id":"b","name":"B","dependsOn":["a"]}]}"#,
        )
        .unwrap();
        let res = discover(&ws);
        assert_eq!(res.default_task.as_deref(), Some("custom:a"));
        let a = res.tasks.iter().find(|t| t.id == "custom:a").unwrap();
        assert_eq!(a.cwd, "sub");
        let b = res.tasks.iter().find(|t| t.id == "custom:b").unwrap();
        assert_eq!(b.depends_on, vec!["custom:a".to_string()]);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
