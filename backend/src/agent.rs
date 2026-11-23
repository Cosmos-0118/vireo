use crate::agent_session::steer::{
    emit_steer_applied, emit_steer_cancelled, ActiveTurnEntry, SteerDelivery, SteerItem, SteerMode,
};
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
use std::time::Duration;
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
    pub provider: String, // "openai" | "anthropic" | "gemini" | "ollama" | "claude-cli" | "codex-cli" | "copilot-cli"
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
    /// CLI providers: the chat thread this turn belongs to. Sessions are kept per thread.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// CLI providers: the agent's own id for this thread's conversation, to resume it.
    #[serde(default)]
    pub resume_id: Option<String>,
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
                "description": "Edit by line anchors: start/end are `N#hash` from read_file/outline/search/verify output; the lines start..end are replaced by new_text (no old text needed; stale line numbers are re-located automatically). op: replace (default), insert_after, insert_before, delete. start may also be `top`, `bottom` or `imports` (insert new_text after the import block; duplicates skipped) with no anchor. Fallback: old_text+new_text exact substring replace.",
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

/// Models actually available on this machine: what the local Ollama daemon has pulled, the models
/// and the models the user's Codex install lists.
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

    Json(json!({ "ollama": ollama, "codex": codex }))
}

/// Resolve an API base URL, refusing to send a credential over plain HTTP to a non-loopback host.
fn secure_base(base: Option<&str>, default: &str, has_key: bool) -> Result<String, String> {
    let raw = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/');
    let url = reqwest::Url::parse(raw).map_err(|e| format!("Invalid base URL: {e}"))?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let loopback = match url.host() {
                Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            if has_key && !loopback {
                return Err("Refusing to send an API key over unencrypted HTTP; use an https:// base URL.".into());
            }
        }
        other => return Err(format!("Unsupported base URL scheme: {other}")),
    }
    Ok(raw.to_string())
}

#[derive(Debug, Deserialize)]
pub struct ApiModelsReq {
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Live model list from an API provider, so the picker never shows hand-maintained (stale) ids.
pub async fn list_api_models(
    State(state): State<AppState>,
    Json(req): Json<ApiModelsReq>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let bad = |m: String| (axum::http::StatusCode::BAD_GATEWAY, m);
    let key = req.api_key.as_deref().unwrap_or("").trim().to_string();
    let base = req.base_url.as_deref().map(|b| b.trim().trim_end_matches('/')).filter(|b| !b.is_empty());
    let default_base = match req.provider.as_str() {
        "anthropic" => "https://api.anthropic.com",
        _ => "https://api.openai.com/v1",
    };
    let base_checked = secure_base(base, default_base, !key.is_empty()).map_err(bad)?;
    let client = state.http_client.clone();
    let timeout = Duration::from_secs(8);

    let (builder, id_field) = match req.provider.as_str() {
        "anthropic" => (
            client
                .get(format!("{base_checked}/v1/models?limit=100"))
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01"),
            "id",
        ),
        "gemini" => (
            client
                .get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=200")
                .header("x-goog-api-key", &key),
            "name",
        ),
        "openai" => {
            let mut b = client.get(format!("{base_checked}/models"));
            if !key.is_empty() {
                b = b.bearer_auth(&key);
            }
            (b, "id")
        }
        other => return Err(bad(format!("Unsupported provider: {other}"))),
    };
    if req.provider != "openai" && key.is_empty() {
        return Err(bad("Add an API key to load models.".into()));
    }

    let resp = builder
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| bad(format!("Could not reach the provider: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad(format!("Provider returned {}", resp.status())));
    }
    let body: Value = resp.json().await.map_err(|e| bad(e.to_string()))?;
    let items = body["data"].as_array().or_else(|| body["models"].as_array());
    let mut models: Vec<String> = items
        .map(|a| {
            a.iter()
                .filter(|m| {
                    // Gemini lists embedding/vision-only models too.
                    req.provider != "gemini"
                        || m["supportedGenerationMethods"]
                            .as_array()
                            .is_some_and(|g| g.iter().any(|x| x.as_str() == Some("generateContent")))
                })
                .filter_map(|m| m[id_field].as_str())
                .map(|s| s.trim_start_matches("models/").to_string())
                .collect()
        })
        .unwrap_or_default();
    if req.provider == "openai" && base.is_none() {
        // The official endpoint also lists embeddings, audio, image and moderation models.
        models.retain(|m| {
            (m.starts_with("gpt-") || m.starts_with('o') || m.starts_with("chatgpt"))
                && !["embedding", "audio", "realtime", "tts", "transcribe", "image", "moderation", "search", "instruct"]
                    .iter()
                    .any(|x| m.contains(x))
        });
    }
    models.sort();
    models.dedup();
    Ok(Json(json!({ "models": models })))
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
            crate::agent_session::run_turn(&state, &ws, &req, kind, auto_cp, &tx).await;
            send_sse(&tx, "done", json!({})).await;
            return;
        }

        if provider != "ollama" && api_key.trim().is_empty() {
            send_sse(
                &tx,
                "error",
                json!({
                    "message": "No API key is set. Open settings (the gear icon in the Agent panel) to add an OpenAI, Anthropic, Gemini, OpenRouter, or local Ollama model, or pick an installed, logged-in CLI (Claude Code, Codex, or GitHub Copilot) without an API key."
                }),
            )
            .await;
            send_sse(&tx, "done", json!({})).await;
            return;
        }

        let thread_id = req
            .thread_id
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("oneoff-{}", uuid::Uuid::new_v4()));
        let turn_id = uuid::Uuid::new_v4().to_string();
        let active_turn = state.active_turns.register(&thread_id, &turn_id);
        send_sse(&tx, "turn", json!({ "turn_id": turn_id })).await;

        match provider.as_str() {
            "anthropic" => {
                run_anthropic_loop(
                    &state,
                    &ws,
                    &req,
                    &api_key,
                    &mut turn,
                    auto_cp,
                    active_turn.entry(),
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
                    auto_cp,
                    active_turn.entry(),
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
                    auto_cp,
                    active_turn.entry(),
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

async fn await_now_steer(active_turn: &ActiveTurnEntry) {
    loop {
        if active_turn.has_now_pending() {
            return;
        }
        active_turn.notified().await;
    }
}

enum HttpOutcome {
    Response(Result<reqwest::Response, reqwest::Error>),
    Preempted,
    Closed,
}

async fn send_cancellable(
    builder: reqwest::RequestBuilder,
    active_turn: &ActiveTurnEntry,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> HttpOutcome {
    tokio::select! {
        _ = tx.closed() => HttpOutcome::Closed,
        _ = await_now_steer(active_turn) => HttpOutcome::Preempted,
        res = builder.send() => HttpOutcome::Response(res),
    }
}

async fn emit_steers(tx: &mpsc::Sender<Result<Event, Infallible>>, steers: &[SteerItem], base_delivery: SteerDelivery) {
    for item in steers {
        let delivery = if item.mode == SteerMode::Now {
            SteerDelivery::Preempted
        } else {
            base_delivery
        };
        emit_steer_applied(tx, item, delivery).await;
    }
}

async fn cancel_api_steers(tx: &mpsc::Sender<Result<Event, Infallible>>, active_turn: &ActiveTurnEntry, reason: &str) {
    for item in active_turn.force_close_and_drain() {
        emit_steer_cancelled(tx, &item.steer_id, reason).await;
    }
}

fn join_steers(steers: &[SteerItem]) -> String {
    steers
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Appends a user text message to an Anthropic messages array while preserving strict
/// user/assistant role alternation.
fn push_anthropic_user_text(messages: &mut Vec<Value>, text: &str) {
    if let Some(last) = messages.last_mut() {
        if last["role"].as_str() == Some("user") {
            match &mut last["content"] {
                Value::Array(arr) => {
                    arr.push(json!({ "type": "text", "text": text }));
                    return;
                }
                Value::String(s) => {
                    let prev = std::mem::take(s);
                    last["content"] = json!([
                        { "type": "text", "text": prev },
                        { "type": "text", "text": text },
                    ]);
                    return;
                }
                _ => {}
            }
        }
    }
    messages.push(json!({ "role": "user", "content": text }));
}

/// Appends a user text part to a Gemini contents array while preserving strict
/// user/model role alternation.
fn push_gemini_user_text(contents: &mut Vec<Value>, text: &str) {
    if let Some(last) = contents.last_mut() {
        if last["role"].as_str() == Some("user") {
            if let Some(parts) = last["parts"].as_array_mut() {
                parts.push(json!({ "text": text }));
                return;
            }
        }
    }
    contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
}

#[allow(clippy::too_many_arguments)]
async fn run_openai_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    auto_cp: bool,
    active_turn: &ActiveTurnEntry,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let default_base = if req.provider == "ollama" {
        "http://127.0.0.1:11434/v1"
    } else {
        "https://api.openai.com/v1"
    };
    let base = match secure_base(req.base_url.as_deref(), default_base, !api_key.trim().is_empty()) {
        Ok(b) => b,
        Err(message) => {
            cancel_api_steers(tx, active_turn, "turn_failed").await;
            send_sse(tx, "error", json!({ "message": message })).await;
            return;
        }
    };
    let url = format!("{}/chat/completions", base);

    let mut messages: Vec<Value> = vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
    for m in &req.messages {
        messages.push(json!({"role": m.role, "content": m.content}));
    }

    let mut round = 0usize;
    let mut total_rounds = 0usize;
    while round < 12 && total_rounds < 48 {
        round += 1;
        total_rounds += 1;

        let mut builder = state.http_client.post(&url).json(&json!({
            "model": req.model,
            "messages": messages,
            "tools": openai_tools_schema(),
            "tool_choice": "auto"
        }));
        if !api_key.trim().is_empty() {
            builder = builder.bearer_auth(api_key.trim());
        }

        let resp = match send_cancellable(builder, active_turn, tx).await {
            HttpOutcome::Closed => {
                let _ = active_turn.force_close_and_drain();
                return;
            }
            HttpOutcome::Preempted => {
                let steers = active_turn.try_pop_all();
                if !steers.is_empty() {
                    emit_steers(tx, &steers, SteerDelivery::Preempted).await;
                    messages.push(json!({ "role": "user", "content": join_steers(&steers) }));
                    round = 0;
                }
                continue;
            }
            HttpOutcome::Response(Ok(r)) => r,
            HttpOutcome::Response(Err(e)) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON ({}): {}", status, e) })).await;
                return;
            }
        };

        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("API error")
                .to_string();
            cancel_api_steers(tx, active_turn, "turn_failed").await;
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
            if let Some(steers) = active_turn.try_close_drain_all() {
                turn.finish(state, Some(tx)).await;
                *turn = TurnTracker::new(auto_cp);
                messages.push(msg.clone());
                messages.push(json!({ "role": "user", "content": join_steers(&steers) }));
                emit_steers(tx, &steers, SteerDelivery::Continuation).await;
                round = 0;
                continue;
            }
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

        let steers = active_turn.try_pop_all();
        if !steers.is_empty() {
            emit_steers(tx, &steers, SteerDelivery::MidTurn).await;
            messages.push(json!({ "role": "user", "content": join_steers(&steers) }));
        }
    }
    cancel_api_steers(tx, active_turn, "turn_failed").await;
}

#[allow(clippy::too_many_arguments)]
async fn run_anthropic_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    auto_cp: bool,
    active_turn: &ActiveTurnEntry,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let base = match secure_base(req.base_url.as_deref(), "https://api.anthropic.com", true) {
        Ok(b) => b,
        Err(message) => {
            cancel_api_steers(tx, active_turn, "turn_failed").await;
            send_sse(tx, "error", json!({ "message": message })).await;
            return;
        }
    };
    let url = format!("{}/v1/messages", base);

    let mut messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.content}))
        .collect();

    let mut round = 0usize;
    let mut total_rounds = 0usize;
    while round < 12 && total_rounds < 48 {
        round += 1;
        total_rounds += 1;

        let builder = state
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
            }));

        let resp = match send_cancellable(builder, active_turn, tx).await {
            HttpOutcome::Closed => {
                let _ = active_turn.force_close_and_drain();
                return;
            }
            HttpOutcome::Preempted => {
                let steers = active_turn.try_pop_all();
                if !steers.is_empty() {
                    emit_steers(tx, &steers, SteerDelivery::Preempted).await;
                    push_anthropic_user_text(&mut messages, &join_steers(&steers));
                    round = 0;
                }
                continue;
            }
            HttpOutcome::Response(Ok(r)) => r,
            HttpOutcome::Response(Err(e)) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON: {}", e) })).await;
                return;
            }
        };
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("Anthropic API error");
            cancel_api_steers(tx, active_turn, "turn_failed").await;
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
            if let Some(steers) = active_turn.try_close_drain_all() {
                turn.finish(state, Some(tx)).await;
                *turn = TurnTracker::new(auto_cp);
                messages.push(json!({ "role": "assistant", "content": content_blocks }));
                messages.push(json!({ "role": "user", "content": join_steers(&steers) }));
                emit_steers(tx, &steers, SteerDelivery::Continuation).await;
                round = 0;
                continue;
            }
            return;
        }

        let steers = active_turn.try_pop_all();
        if !steers.is_empty() {
            emit_steers(tx, &steers, SteerDelivery::MidTurn).await;
            // Anthropic forbids consecutive `role: "user"` messages: append steered text
            // directly to the same `tool_results` user message content array.
            tool_results.push(json!({ "type": "text", "text": join_steers(&steers) }));
        }
        messages.push(json!({ "role": "assistant", "content": content_blocks }));
        messages.push(json!({ "role": "user", "content": tool_results }));
    }
    cancel_api_steers(tx, active_turn, "turn_failed").await;
}

#[allow(clippy::too_many_arguments)]
async fn run_gemini_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    turn: &mut TurnTracker,
    auto_cp: bool,
    active_turn: &ActiveTurnEntry,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        req.model
    );

    let mut contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = if m.role == "assistant" { "model" } else { "user" };
            json!({ "role": role, "parts": [{ "text": m.content }] })
        })
        .collect();

    let mut round = 0usize;
    let mut total_rounds = 0usize;
    while round < 12 && total_rounds < 48 {
        round += 1;
        total_rounds += 1;

        let builder = state
            .http_client
            .post(&url)
            .header("x-goog-api-key", api_key.trim())
            .json(&json!({
                "system_instruction": { "parts": [{ "text": SYSTEM_PROMPT }] },
                "contents": contents,
                "tools": gemini_tools_schema()
            }));

        let resp = match send_cancellable(builder, active_turn, tx).await {
            HttpOutcome::Closed => {
                let _ = active_turn.force_close_and_drain();
                return;
            }
            HttpOutcome::Preempted => {
                let steers = active_turn.try_pop_all();
                if !steers.is_empty() {
                    emit_steers(tx, &steers, SteerDelivery::Preempted).await;
                    push_gemini_user_text(&mut contents, &join_steers(&steers));
                    round = 0;
                }
                continue;
            }
            HttpOutcome::Response(Ok(r)) => r,
            HttpOutcome::Response(Err(e)) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Request failed: {}", e) })).await;
                return;
            }
        };

        let status = resp.status();
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                cancel_api_steers(tx, active_turn, "turn_failed").await;
                send_sse(tx, "error", json!({ "message": format!("Invalid JSON: {}", e) })).await;
                return;
            }
        };
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("Gemini API error");
            cancel_api_steers(tx, active_turn, "turn_failed").await;
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
            if let Some(steers) = active_turn.try_close_drain_all() {
                turn.finish(state, Some(tx)).await;
                *turn = TurnTracker::new(auto_cp);
                contents.push(json!({ "role": "model", "parts": parts }));
                contents.push(json!({ "role": "user", "parts": [{ "text": join_steers(&steers) }] }));
                emit_steers(tx, &steers, SteerDelivery::Continuation).await;
                round = 0;
                continue;
            }
            return;
        }

        let steers = active_turn.try_pop_all();
        if !steers.is_empty() {
            emit_steers(tx, &steers, SteerDelivery::MidTurn).await;
            // Gemini expects user text appended inside the same user `parts` array after functionResponse.
            response_parts.push(json!({ "text": join_steers(&steers) }));
        }
        contents.push(json!({ "role": "model", "parts": parts }));
        contents.push(json!({ "role": "user", "parts": response_parts }));
    }
    cancel_api_steers(tx, active_turn, "turn_failed").await;
}

#[cfg(test)]
mod steer_tests {
    use super::*;

    #[test]
    fn anthropic_user_text_merges_consecutive_user_turns() {
        let mut msgs = vec![json!({ "role": "user", "content": "original" })];
        push_anthropic_user_text(&mut msgs, "steered");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"][0]["text"], "original");
        assert_eq!(msgs[0]["content"][1]["text"], "steered");

        msgs.push(json!({ "role": "assistant", "content": [{ "type": "text", "text": "ok" }] }));
        push_anthropic_user_text(&mut msgs, "next");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2], json!({ "role": "user", "content": "next" }));
    }

    #[test]
    fn gemini_user_text_merges_consecutive_user_parts() {
        let mut contents = vec![json!({ "role": "user", "parts": [{ "text": "original" }] })];
        push_gemini_user_text(&mut contents, "steered");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["parts"][0]["text"], "original");
        assert_eq!(contents[0]["parts"][1]["text"], "steered");
    }
}
