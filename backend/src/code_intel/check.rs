//! Build/test runner with deduplicated anchored diagnostics, rustc notes/labels, and failing test output.

use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::doc::hl;
use super::util::{ignored_dir, run_capture, shell_command};

pub(crate) struct Diag {
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

pub(crate) fn clip(s: &str, max: usize) -> String {
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

pub(crate) fn parse_diags(raw: &str, prefix: &str) -> Vec<Diag> {
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

pub(crate) fn render_diags(ws: &Path, diags: &[Diag], label: &str, secs: f64, show_warnings: bool) -> String {
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
pub(crate) fn compress_plain(raw: &str) -> String {
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
pub(crate) fn test_summary(raw: &str) -> Option<String> {
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
