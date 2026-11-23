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
    // rustc's human format (plain `cargo build` / `cargo test`): a header line, then `--> file:l:c`,
    // then a source snippet whose `^^^ label` line carries the explanation.
    let rustc_head = lazy_re!(r"^(?P<lvl>error|warning)(?:\[(?P<c>[A-Za-z0-9]+)\])?: (?P<m>.+)$");
    let rustc_arrow = lazy_re!(r"^\s*--> (?P<f>[^:\n]+):(?P<l>\d+):(?P<c>\d+)\s*$");
    let rustc_label = lazy_re!(r"^\s*(?:\d+\s*)?\|\s*[\s^~\-]*\^[\^~\-]*\s*(?P<t>.*)$");
    let mut pending: Option<(bool, String, String)> = None;
    let mut want_label = false;
    // tsc / gcc print a message's details on indented lines right after it.
    let mut continuing = false;
    for l in raw.lines() {
        let l = l.trim_end();
        if let Some(c) = rustc_head.captures(l) {
            continuing = false;
            want_label = false;
            pending = Some((&c["lvl"] == "error", c.name("c").map_or(String::new(), |m| m.as_str().to_string()), c["m"].to_string()));
            continue;
        }
        if let Some((error, code, msg)) = pending.take() {
            if let Some(c) = rustc_arrow.captures(l) {
                out.push(Diag {
                    file: format!("{prefix}{}", c["f"].trim().replace('\\', "/").trim_start_matches("./")),
                    line: c["l"].parse().unwrap_or(0),
                    col: c["c"].parse().unwrap_or(0),
                    error,
                    code,
                    msg,
                    label: String::new(),
                    notes: Vec::new(),
                });
                want_label = true;
                continue;
            }
            // No location (e.g. `error: could not compile ...`): not a located diagnostic.
        }
        if want_label {
            if let Some(c) = rustc_label.captures(l) {
                if let Some(d) = out.last_mut() {
                    d.label = clip(c["t"].trim(), 140);
                }
                want_label = false;
                continue;
            }
        }
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
    // Long lines that repeat (per-crate build-script warnings, env dumps) are shown once with a
    // count; crate names/versions are ignored when comparing so `warning: foo@1.0: X` and
    // `warning: bar@2.0: X` collapse.
    let mut reps: Vec<usize> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let prefix_re = lazy_re!(r"^(?:cargo:warning=|warning: [\w.\-]+@[\w.+\-]+: )");
    for l in raw.lines() {
        let t = l.trim_end();
        if t.trim().is_empty() || progress_line(t) || lines.last() == Some(&t) || t.contains("run with `RUST_BACKTRACE=1`") {
            continue;
        }
        if t.trim().chars().count() >= 24 {
            let key = prefix_re.replace(t.trim(), "").into_owned();
            if let Some(&idx) = seen.get(&key) {
                reps[idx] += 1;
                continue;
            }
            seen.insert(key, lines.len());
        }
        lines.push(t);
        reps.push(1);
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
        // A `warning:` header (or its snippet) merely mentioning "expected"/"error" isn't a failure.
        let warn_header = l.starts_with("warning");
        if in_block || (!warn_header && fail_re.is_match(l)) {
            keep[i] = true;
            any = true;
            if !in_block {
                // A compiler error's snippet and `^^^ label` follow it; other hits need less.
                let ctx = if l.starts_with("error") { 5 } else { 2 };
                for k in keep.iter_mut().skip(i + 1).take(ctx) {
                    *k = true;
                }
            }
        }
    }
    let fmt = |i: usize| if reps[i] > 1 { format!("{}  (×{})", lines[i], reps[i]) } else { lines[i].to_string() };
    let picked: Vec<String> = (0..lines.len()).filter(|&i| !any || keep[i]).map(fmt).collect();
    let n = picked.len();
    // Root causes are at the top, summaries at the bottom: keep both ends, drop the middle.
    let out = if n > 60 {
        format!(
            "{}\n…[{} lines omitted]\n{}",
            picked[..20].join("\n"),
            n - 60,
            picked[n - 40..].join("\n")
        )
    } else {
        picked.join("\n")
    };
    out.chars().take(6000).collect()
}

/// Environment problems that look like code failures: said first, so tail-trimming can't hide them.
fn env_hint(raw: &str) -> Option<&'static str> {
    let l = raw.to_ascii_lowercase();
    if (l.contains("cl.exe") || l.contains("link.exe")) && (l.contains("not found") || l.contains("not be found")) {
        return Some("hint: MSVC build tools are not on PATH for this shell (environment problem, not a code error). Run from a VS Developer shell or source the repo's msvc-env.ps1 (Enter-MsvcEnv), then retry.");
    }
    if l.contains("command not found") || l.contains("is not recognized as an internal or external command") || l.contains("program not found") {
        return Some("hint: a required program is missing or not on PATH (environment problem, not a code error).");
    }
    if l.contains("os error 32") || l.contains("lnk1104") || l.contains("being used by another process") || l.contains("text file busy") {
        return Some("hint: a file is locked by a running process, likely the app or a previous build (environment problem, not a code error). Stop that process, or use a check that doesn't relink (e.g. `cargo check`).");
    }
    if l.contains("eaddrinuse") || l.contains("os error 10048") || l.contains("address already in use") {
        return Some("hint: a TCP port is already in use by another process (environment problem, not a code error).");
    }
    if l.contains("failed to download") || l.contains("enotfound") || l.contains("could not resolve host") || l.contains("unable to update registry") || l.contains("network is unreachable") {
        return Some("hint: the network or package registry is unreachable (environment problem, not a code error). Retry, or run offline if dependencies are already cached.");
    }
    if l.contains("eperm") || l.contains("eacces") || l.contains("access is denied") || l.contains("permission denied") {
        return Some("hint: a permission error blocked the command (environment problem, not a code error). Check file ownership and that no process holds the file.");
    }
    None
}

/// What a passing custom command printed, so a silent pass is stated rather than implied.
fn quiet_note(raw: &str) -> String {
    let out: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with('{') && !progress_line(t))
        .collect();
    match out.last() {
        None => ", exit 0, no output".to_string(),
        Some(last) => format!(", exit 0, {} output line(s), last: {}", out.len(), clip(last, 80)),
    }
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
    if any && p == 0 && f == 0 {
        return Some("0 tests ran ⚠ (check the test name/filter)".to_string());
    }
    if !any && lazy_re!(r"(?i)no tests (ran|found|collected)|collected 0 items|Tests:\s+0 total").is_match(raw) {
        return Some("0 tests ran ⚠ (check the test name/filter)".to_string());
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

/// Label for a user-supplied command: the full command line, shortened so long ones stay readable.
fn short_label(cmd: &str) -> String {
    let one_line = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 60 {
        one_line
    } else {
        format!("{}…", one_line.chars().take(59).collect::<String>())
    }
}

pub fn check(ws: &Path, cmd: Option<&str>, timeout_secs: u64, opts: CheckOpts) -> Result<String, String> {
    let jobs: Vec<(String, String, String)> = match cmd.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => vec![(String::new(), c.to_string(), short_label(c))],
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
            let quiet = if cmd.is_some() && diags.is_empty() && tests.is_empty() { quiet_note(&raw) } else { String::new() };
            report.push_str(&format!(
                "✔ {label} ok ({:.1}s{tests}{quiet}){}\n",
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
            let hint = env_hint(&raw).map(|h| format!("{h}\n")).unwrap_or_default();
            report.push_str(&format!("✖ {label} failed (exit {}, {:.1}s{tests}):\n{hint}{}\n", out.code, out.secs, tail));
        }
    }
    let max = if opts.raw { 24_000 } else { 12_000 };
    Ok(report.chars().take(max).collect())
}
