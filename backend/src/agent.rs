use crate::checkpoint::TurnTracker;
use crate::state::{background_command, is_ignored_dir_name, resolve_safe_path, AppState};
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use walkdir::WalkDir;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentChatReq {
    pub provider: String, // "openai" | "anthropic" | "gemini" | "ollama" | "antigravity-cli" | "claude-cli" | "codex-cli" | "gemini-cli" | "opencode-cli" | "cursor-cli" | "copilot-cli" | "amp-cli" | "qwen-cli"
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub auto_checkpoint: Option<bool>,
    /// CLI providers only: "ask" | "auto_edit" (default) | "full_access".
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Claude CLI only: Vireo permission rules the user chose to "always allow" in this thread
    /// (see `cli_agent::permission_rule`).
    #[serde(default)]
    pub allowed_rules: Option<Vec<String>>,
}

const SYSTEM_PROMPT: &str = r#"You are Vireo Agent, a concise, practical AI software engineer working directly inside the user's local repository.
You have token-efficient tools to inspect and modify files and run commands:
- repo_map(query?, dir?) ranked symbol map; outline(path) symbols with [start#hash..end#hash] ranges
- read_file(path, start_line?, end_line?, symbol?) returns lines as `N#hash|code`
- search_code(query) returns `file:line#hash| text`
- edit_file(path, start, end?, new_text) replaces lines by their `N#hash` anchors (copy them from read_file/outline/search/verify output); no old text needed. Use op=insert_after/insert_before/delete for those cases.
- symbol_edit(path, symbol, mode, new_code?) reads or changes a whole function/struct/class by name
- ast(pattern, lang?, rewrite?) structural search / multi-file codemod (ast-grep)
- verify(command?) runs build/tests and returns only deduplicated errors with anchors
- write_file(path, content) for new files; run_command(command) for anything else

Guidelines:
- Orient with repo_map/outline, then read only the needed symbol or range, not whole files.
- Edits report new syntax errors immediately; you do not need to re-read after editing. If an edit is rejected for stale anchors, use the fresh snippet it returns.
- Make minimal, clean, well-tested changes; run verify after changing code.
- After making edits, summarize what changed and mention any commands or tests to run."#;

fn openai_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List files in the workspace or a relative subdirectory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dir": { "type": "string", "description": "Optional relative directory path." }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "repo_map",
                "description": "Ranked map of source files and symbol signatures. Pass query to focus; far cheaper than reading files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Keywords (symbol or file names)." },
                        "dir": { "type": "string", "description": "Optional subdirectory." }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "outline",
                "description": "Symbols of a source file as [start#hash..end#hash] ranges (usable as edit_file anchors).",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read lines as `N#hash|code` (optionally a line range or one symbol). Large files are capped; prefer outline + symbol/range.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start_line": { "type": "integer" },
                        "end_line": { "type": "integer" },
                        "symbol": { "type": "string", "description": "Read just this function/class, e.g. `run_loop` or `Type::method`." }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Regex search across the workspace; results are `file:line#hash| text`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "glob": { "type": "string", "description": "Optional file glob, e.g. *.rs" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a file in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Edit by line anchors: start/end are `N#hash` from read_file/outline/search/verify output; the lines start..end are replaced by new_text (no old text needed; stale line numbers are re-located automatically). op: replace (default), insert_after, insert_before, delete. Fallback: old_text+new_text exact substring replace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start": { "type": "string", "description": "Anchor like 189#a4." },
                        "end": { "type": "string", "description": "Optional end anchor (inclusive); defaults to start." },
                        "new_text": { "type": "string", "description": "Replacement code without the N#hash| prefixes." },
                        "op": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"] },
                        "old_text": { "type": "string", "description": "Fallback only." }
                    },
                    "required": ["path", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "symbol_edit",
                "description": "Read or change a whole function/struct/class/interface by name (range includes its doc comments/attributes). mode: read (default), replace, insert_after, delete.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "symbol": { "type": "string", "description": "Name, or Container::name for methods." },
                        "mode": { "type": "string", "enum": ["read", "replace", "insert_after", "delete"] },
                        "new_code": { "type": "string" }
                    },
                    "required": ["path", "symbol"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ast",
                "description": "Structural code search (ast-grep patterns like `foo($A, $B)`); with rewrite it applies a codemod across files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "lang": { "type": "string", "description": "e.g. rust, ts, tsx, python, go" },
                        "rewrite": { "type": "string" },
                        "path": { "type": "string" }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "verify",
                "description": "Run the project's build/type-check (auto-detected) or a given command and return only deduplicated errors with line anchors.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string" } }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace root (30s timeout).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"]
                }
            }
        }
    ])
}

fn anthropic_tools_schema() -> Value {
    let items = openai_tools_schema();
    let arr: Vec<Value> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let f = &t["function"];
            json!({
                "name": f["name"],
                "description": f["description"],
                "input_schema": f["parameters"],
            })
        })
        .collect();
    Value::Array(arr)
}

fn gemini_tools_schema() -> Value {
    let items = openai_tools_schema();
    let decls: Vec<Value> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let f = &t["function"];
            json!({
                "name": f["name"],
                "description": f["description"],
                "parameters": f["parameters"],
            })
        })
        .collect();
    json!([{ "function_declarations": decls }])
}

async fn exec_tool(
    state: &AppState,
    ws: &Path,
    name: &str,
    args: &Value,
    turn: &mut TurnTracker,
) -> String {
    let is_mutation = matches!(name, "write_file" | "edit_file" | "run_command" | "symbol_edit")
        || (name == "ast" && args.get("rewrite").and_then(|v| v.as_str()).map_or(false, |r| !r.is_empty()));
    if is_mutation {
        // Silent pre-turn snapshot; the checkpoint is only shown at the end if files changed.
        turn.begin(state).await;
        turn.note_tool(name, args);
    }

    match name {
        "list_files" => {
            let rel = args.get("dir").and_then(|v| v.as_str()).unwrap_or("");
            let base = if rel.is_empty() {
                ws.to_path_buf()
            } else {
                match resolve_safe_path(ws, rel) {
                    Ok(p) => p,
                    Err(e) => return format!("Error: {}", e),
                }
            };
            let ws_owned = ws.to_path_buf();
            tokio::task::spawn_blocking(move || {
                let mut out = Vec::new();
                for e in WalkDir::new(&base)
                    .max_depth(4)
                    .into_iter()
                    .filter_entry(|ent| {
                        if ent.depth() == 0 {
                            return true;
                        }
                        let n = ent.file_name().to_string_lossy();
                        !(ent.file_type().is_dir() && is_ignored_dir_name(&n))
                    })
                    .flatten()
                {
                    if e.file_type().is_file() {
                        if let Ok(r) = e.path().strip_prefix(&ws_owned) {
                            out.push(r.to_string_lossy().replace('\\', "/"));
                            if out.len() >= 200 {
                                break;
                            }
                        }
                    }
                }
                out.sort();
                out.join("\n")
            })
            .await
            .unwrap_or_else(|_| "Failed to list files".to_string())
        }
        "repo_map" | "outline" | "read_file" | "search_code" | "ast" | "verify" | "symbol_edit" | "edit_file"
            if name != "edit_file" || args.get("old_text").and_then(|v| v.as_str()).is_none() =>
        {
            let (op, a) = match name {
                "repo_map" => ("map", args.clone()),
                "outline" => ("outline", args.clone()),
                "search_code" => ("search", json!({ "query": args["query"], "glob": args["glob"] })),
                "ast" => ("ast", args.clone()),
                "verify" => ("check", json!({ "cmd": args["command"] })),
                "symbol_edit" => (
                    "symbol",
                    json!({ "path": args["path"], "symbol": args["symbol"], "mode": args["mode"], "new_code": args["new_code"] }),
                ),
                "edit_file" => ("edit", args.clone()),
                _ => {
                    // read_file
                    let target = if let Some(sym) = args.get("symbol").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                        Some(sym.to_string())
                    } else {
                        let st = args.get("start_line").and_then(|v| v.as_u64());
                        let en = args.get("end_line").and_then(|v| v.as_u64());
                        match (st, en) {
                            (None, None) => None,
                            (s, e) => Some(format!("{}..{}", s.unwrap_or(1), e.map(|e| e.to_string()).unwrap_or_default())),
                        }
                    };
                    ("view", json!({ "path": args["path"], "target": target }))
                }
            };
            let ws_owned = ws.to_path_buf();
            let op_owned = op.to_string();
            let res = tokio::task::spawn_blocking(move || crate::code_intel::run_op_string(&ws_owned, &op_owned, &a))
                .await
                .unwrap_or_else(|_| "Error: tool crashed".to_string());
            if matches!(name, "edit_file" | "symbol_edit" | "ast") && !res.starts_with("Error") {
                let touched = args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                state.notify_fs_changed(vec![if touched.is_empty() { "ast".to_string() } else { touched }]);
            }
            res
        }
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let full = match resolve_safe_path(ws, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            if let Some(parent) = full.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            match tokio::fs::write(&full, content.as_bytes()).await {
                Ok(_) => {
                    state.notify_fs_changed(vec![path.to_string()]);
                    format!("Wrote {} ({} bytes)", path, content.len())
                }
                Err(e) => format!("Error writing {}: {}", path, e),
            }
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
            let new_text = args.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
            let full = match resolve_safe_path(ws, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            let current = match tokio::fs::read_to_string(&full).await {
                Ok(c) => c,
                Err(e) => return format!("Error reading {}: {}", path, e),
            };
            if !current.contains(old_text) {
                return format!("Error: old_text not found in {}", path);
            }
            let updated = current.replacen(old_text, new_text, 1);
            match tokio::fs::write(&full, updated.as_bytes()).await {
                Ok(_) => {
                    state.notify_fs_changed(vec![path.to_string()]);
                    format!("Edited {} successfully", path)
                }
                Err(e) => format!("Error writing {}: {}", path, e),
            }
        }
        "run_command" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.trim().is_empty() {
                return "Empty command".to_string();
            }
            let fut = if cfg!(windows) {
                background_command("powershell.exe")
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd])
                    .env("PATH", crate::pty::augmented_path_env())
                    .current_dir(ws)
                    .output()
            } else {
                background_command("sh")
                    .args(["-lc", cmd])
                    .env("PATH", crate::pty::augmented_path_env())
                    .current_dir(ws)
                    .output()
            };
            match tokio::time::timeout(Duration::from_secs(30), fut).await {
                Ok(Ok(o)) => {
                    state.notify_fs_changed(vec!["cmd".to_string()]);
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let combined = format!(
                        "exit={}\n{}\n{}",
                        o.status.code().unwrap_or(-1),
                        stdout,
                        stderr
                    );
                    combined.chars().take(12_000).collect()
                }
                Ok(Err(e)) => format!("Failed to run command: {}", e),
                Err(_) => "Command timed out after 30s".to_string(),
            }
        }
        other => format!("Unknown tool: {}", other),
    }
}

#[derive(Debug, Deserialize)]
pub struct ModelsQuery {
    /// Ollama base URL as stored in the UI (with or without the trailing `/v1`).
    pub ollama_base: Option<String>,
}

#[derive(Serialize)]
struct LocalModels {
    running: bool,
    models: Vec<String>,
}

#[derive(Default)]
struct AgyModelsCache {
    models: Vec<String>,
    fetching: bool,
    fetched_at: Option<Instant>,
}

static AGY_MODELS_CACHE: OnceLock<Mutex<AgyModelsCache>> = OnceLock::new();

fn cached_antigravity_models() -> Vec<String> {
    let cache = AGY_MODELS_CACHE.get_or_init(|| Mutex::new(AgyModelsCache::default()));
    let (models, should_fetch) = {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        let stale = guard
            .fetched_at
            .map(|t| t.elapsed() > Duration::from_secs(600))
            .unwrap_or(true);
        let should_fetch = !guard.fetching && stale;
        if should_fetch {
            guard.fetching = true;
        }
        (guard.models.clone(), should_fetch)
    };

    if should_fetch {
        tokio::spawn(async move {
            let exe = tokio::task::spawn_blocking(|| crate::pty::resolve_cli_tool("agy"))
                .await
                .ok()
                .flatten();
            let fetched = match exe {
                Some(path) => {
                    let mut cmd = background_command(&path.to_string_lossy());
                    cmd.arg("models")
                        .env("PATH", crate::pty::augmented_path_env())
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null());
                    match tokio::time::timeout(Duration::from_secs(25), cmd.output()).await {
                        Ok(Ok(out)) if out.status.success() => {
                            let text = String::from_utf8_lossy(&out.stdout);
                            let parsed: Vec<String> = text
                                .lines()
                                .filter_map(|line| {
                                    let (slug, _) = line.split_once('\t')?;
                                    let slug = slug.trim();
                                    (!slug.is_empty()).then(|| slug.to_string())
                                })
                                .collect();
                            Some(parsed)
                        }
                        _ => None,
                    }
                }
                None => Some(Vec::new()),
            };
            let cache = AGY_MODELS_CACHE.get_or_init(|| Mutex::new(AgyModelsCache::default()));
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(list) = fetched {
                if !list.is_empty() {
                    guard.models = list;
                }
            }
            guard.fetching = false;
            guard.fetched_at = Some(Instant::now());
        });
    }

    models
}

/// Models actually available on this machine: what the local Ollama daemon has pulled, the models
/// the user's Codex install lists, and cached models from Antigravity CLI (`agy models`).
pub async fn list_models(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ModelsQuery>,
) -> Json<Value> {
    let base = q
        .ollama_base
        .as_deref()
        .map(|b| b.trim().trim_end_matches('/').trim_end_matches("/v1").to_string())
        .filter(|b| b.starts_with("http://") || b.starts_with("https://"))
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());

    let probe = async {
        let resp = state
            .http_client
            .get(format!("{base}/api/tags"))
            .timeout(Duration::from_millis(800))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        let body: Value = resp.json().await.ok()?;
        let models = body["models"]
            .as_array()?
            .iter()
            .filter_map(|m| m["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        Some(models)
    };
    let ollama = match probe.await {
        Some(models) => LocalModels { running: true, models },
        None => LocalModels { running: false, models: vec![] },
    };

    let codex = tokio::task::spawn_blocking(|| {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let raw = std::fs::read_to_string(Path::new(&home).join(".codex").join("models_cache.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        Some(
            v["models"]
                .as_array()?
                .iter()
                .filter(|m| m["visibility"].as_str() == Some("list"))
                .filter_map(|m| m["slug"].as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        )
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    let antigravity = cached_antigravity_models();

    Json(json!({ "ollama": ollama, "codex": codex, "antigravity": antigravity }))
}

pub(crate) async fn send_sse(tx: &mpsc::Sender<Result<Event, Infallible>>, event: &str, payload: Value) {
    let _ = tx
        .send(Ok(Event::default()
            .event(event)
            .data(payload.to_string())))
        .await;
}

pub async fn chat(
    State(state): State<AppState>,
    Json(req): Json<AgentChatReq>,
) -> impl IntoResponse {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        let ws = state.workspace_path().await;
        let auto_cp = req.auto_checkpoint.unwrap_or(true);
        let mut turn = TurnTracker::new(auto_cp);

        let provider = req.provider.to_lowercase();
        let api_key = req.api_key.clone().unwrap_or_default();

        // Installed agent CLIs use their own login; no API key involved.
        if let Some(kind) = crate::cli_agent::CliKind::from_provider(&provider) {
            crate::cli_agent::run(&state, &ws, &req, kind, auto_cp, &tx).await;
            send_sse(&tx, "done", json!({})).await;
            return;
        }

        if provider != "ollama" && api_key.trim().is_empty() {
            send_sse(
                &tx,
                "error",
                json!({
                    "message": "No API key is set. Open settings (the gear icon in the Agent panel) to add an OpenAI, Anthropic, Gemini, OpenRouter, or local Ollama model, or pick an installed, logged-in CLI (Antigravity, Claude Code, Codex, Gemini CLI, OpenCode, Cursor CLI, GitHub Copilot, Amp, or Qwen Code) without an API key."
                }),
            )
            .await;
            send_sse(&tx, "done", json!({})).await;
            return;
        }

        match provider.as_str() {
            "anthropic" => {
                run_anthropic_loop(
                    &state,
                    &ws,
                    &req,
                    &api_key,
                    &mut turn,
                    &tx,
                )
                .await;
            }
            "gemini" => {
                run_gemini_loop(
                    &state,
                    &ws,
                    &req,
                    &api_key,
                    &mut turn,
                    &tx,
                )
                .await;
            }
            _ => {
                // openai, openrouter, ollama, lmstudio, custom openai-compatible
                run_openai_loop(
                    &state,
                    &ws,
                    &req,
                    &api_key,
                    &mut turn,
                    &tx,
                )
                .await;
            }
        }

        turn.finish(&state, Some(&tx)).await;
        send_sse(&tx, "done", json!({})).await;
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn run_openai_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let base = req
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(if req.provider == "ollama" {
            "http://127.0.0.1:11434/v1"
        } else {
            "https://api.openai.com/v1"
        })
        .trim_end_matches('/');
    let url = format!("{}/chat/completions", base);

    let mut messages: Vec<Value> = vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
    for m in &req.messages {
        messages.push(json!({"role": m.role, "content": m.content}));
    }

    for _round in 0..12 {
        let mut builder = state.http_client.post(&url).json(&json!({
            "model": req.model,
            "messages": messages,
            "tools": openai_tools_schema(),
            "tool_choice": "auto"
        }));
        if !api_key.trim().is_empty() {
            builder = builder.bearer_auth(api_key.trim());
        }

        let resp = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON ({}): {}", status, e) })).await;
                return;
            }
        };

        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("API error")
                .to_string();
            send_sse(tx, "error", json!({ "message": format!("{}: {}", status, msg) })).await;
            return;
        }

        let msg = &body["choices"][0]["message"];
        if let Some(content) = msg["content"].as_str() {
            if !content.is_empty() {
                send_sse(tx, "text", json!({ "text": content })).await;
            }
        }

        let tool_calls = msg["tool_calls"].as_array();
        if tool_calls.map(|a| a.is_empty()).unwrap_or(true) {
            return;
        }

        messages.push(msg.clone());
        for tc in tool_calls.unwrap() {
            let id = tc["id"].as_str().unwrap_or("call").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));

            send_sse(tx, "tool_call", json!({ "id": id, "name": name, "args": args })).await;
            let result = exec_tool(state, ws, &name, &args, turn).await;
            send_sse(
                tx,
                "tool_result",
                json!({ "id": id, "name": name, "result": result.chars().take(1500).collect::<String>() }),
            )
            .await;

            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }
    }
}

async fn run_anthropic_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let base = req
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://api.anthropic.com")
        .trim_end_matches('/');
    let url = format!("{}/v1/messages", base);

    let mut messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.content}))
        .collect();

    for _round in 0..12 {
        let resp = match state
            .http_client
            .post(&url)
            .header("x-api-key", api_key.trim())
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": req.model,
                "max_tokens": 4096,
                "system": SYSTEM_PROMPT,
                "tools": anthropic_tools_schema(),
                "messages": messages
            }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON: {}", e) })).await;
                return;
            }
        };
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("Anthropic API error");
            send_sse(tx, "error", json!({ "message": format!("{}: {}", status, msg) })).await;
            return;
        }

        let content_blocks = body["content"].as_array().cloned().unwrap_or_default();
        let mut tool_results = Vec::new();

        for block in &content_blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str() {
                        send_sse(tx, "text", json!({ "text": t })).await;
                    }
                }
                Some("tool_use") => {
                    let id = block["id"].as_str().unwrap_or("tool").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    let input = block["input"].clone();
                    send_sse(tx, "tool_call", json!({ "id": id, "name": name, "args": input })).await;
                    let result = exec_tool(state, ws, &name, &input, turn).await;
                    send_sse(
                        tx,
                        "tool_result",
                        json!({ "id": id, "name": name, "result": result.chars().take(1500).collect::<String>() }),
                    )
                    .await;
                    tool_results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": result
                    }));
                }
                _ => {}
            }
        }

        if tool_results.is_empty() {
            return;
        }
        messages.push(json!({ "role": "assistant", "content": content_blocks }));
        messages.push(json!({ "role": "user", "content": tool_results }));
    }
}

async fn run_gemini_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        req.model,
        api_key.trim()
    );

    let mut contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = if m.role == "assistant" { "model" } else { "user" };
            json!({ "role": role, "parts": [{ "text": m.content }] })
        })
        .collect();

    for _round in 0..12 {
        let resp = match state
            .http_client
            .post(&url)
            .json(&json!({
                "system_instruction": { "parts": [{ "text": SYSTEM_PROMPT }] },
                "contents": contents,
                "tools": gemini_tools_schema()
            }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON: {}", e) })).await;
                return;
            }
        };
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("Gemini API error");
            send_sse(tx, "error", json!({ "message": format!("{}: {}", status, msg) })).await;
            return;
        }

        let parts = body["candidates"][0]["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut response_parts = Vec::new();

        for part in &parts {
            if let Some(t) = part["text"].as_str() {
                send_sse(tx, "text", json!({ "text": t })).await;
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or("").to_string();
                let args = fc["args"].clone();
                let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                send_sse(tx, "tool_call", json!({ "id": id, "name": name, "args": args })).await;
                let result = exec_tool(state, ws, &name, &args, turn).await;
                send_sse(
                    tx,
                    "tool_result",
                    json!({ "id": id, "name": name, "result": result.chars().take(1500).collect::<String>() }),
                )
                .await;
                response_parts.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": { "result": result }
                    }
                }));
            }
        }

        if response_parts.is_empty() {
            return;
        }
        contents.push(json!({ "role": "model", "parts": parts }));
        contents.push(json!({ "role": "user", "parts": response_parts }));
    }
}
