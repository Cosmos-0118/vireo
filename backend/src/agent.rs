use crate::git::create_checkpoint_internal;
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
    pub provider: String, // "openai" | "anthropic" | "gemini" | "ollama"
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub auto_checkpoint: Option<bool>,
}

const SYSTEM_PROMPT: &str = r#"You are Vireo Agent, a concise, practical AI software engineer working directly inside the user's local repository.
You have tools to inspect and modify files and run commands:
- list_files(dir?)
- read_file(path, start_line?, end_line?)
- search_code(query)
- write_file(path, content)
- edit_file(path, old_text, new_text)
- run_command(command)

Guidelines:
- Always read relevant files before editing them.
- Make minimal, clean, well-tested changes.
- Prefer edit_file for targeted edits; use write_file for new files.
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
                "name": "read_file",
                "description": "Read a text file in the workspace (optionally a line range).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start_line": { "type": "integer" },
                        "end_line": { "type": "integer" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Search text/regex across the workspace using ripgrep.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
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
                "description": "Replace an exact substring in an existing file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old_text": { "type": "string" },
                        "new_text": { "type": "string" }
                    },
                    "required": ["path", "old_text", "new_text"]
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
    checkpoint_done: &mut bool,
    auto_checkpoint: bool,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> String {
    let is_mutation = matches!(name, "write_file" | "edit_file" | "run_command");
    if is_mutation && auto_checkpoint && !*checkpoint_done {
        *checkpoint_done = true;
        if let Ok(cp) = create_checkpoint_internal(state, "Before agent edit".to_string()).await {
            let _ = tx
                .send(Ok(Event::default()
                    .event("checkpoint")
                    .data(serde_json::to_string(&cp).unwrap_or_default())))
                .await;
        }
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
        "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let full = match resolve_safe_path(ws, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            let text = match tokio::fs::read_to_string(&full).await {
                Ok(t) => t,
                Err(e) => return format!("Error reading {}: {}", path, e),
            };
            let start = args
                .get("start_line")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as usize;
            let end = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let lines: Vec<&str> = text.lines().collect();
            let stop = end.unwrap_or(lines.len()).min(lines.len());
            let mut out = String::new();
            for i in (start - 1)..stop {
                out.push_str(&format!("{:>4} | {}\n", i + 1, lines[i]));
                if out.len() > 24_000 {
                    out.push_str("...[truncated]\n");
                    break;
                }
            }
            out
        }
        "search_code" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            if q.is_empty() {
                return "Empty query".to_string();
            }
            let out = background_command("rg")
                .args([
                    "-n",
                    "--smart-case",
                    "--max-count",
                    "10",
                    "--glob",
                    "!.git",
                    "--glob",
                    "!node_modules",
                    "--glob",
                    "!target",
                    "--",
                    q,
                ])
                .current_dir(ws)
                .output()
                .await;
            match out {
                Ok(o) => {
                    let s = String::from_utf8_lossy(&o.stdout);
                    if s.trim().is_empty() {
                        "No matches found.".to_string()
                    } else {
                        s.chars().take(12_000).collect()
                    }
                }
                Err(e) => format!("Search error: {}", e),
            }
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

async fn send_sse(tx: &mpsc::Sender<Result<Event, Infallible>>, event: &str, payload: Value) {
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
        let mut checkpoint_done = false;

        let provider = req.provider.to_lowercase();
        let api_key = req.api_key.clone().unwrap_or_default();

        if provider != "ollama" && api_key.trim().is_empty() {
            send_sse(
                &tx,
                "error",
                json!({
                    "message": "No API key is set. Open settings (the gear icon in the Agent panel) to add an OpenAI, Anthropic, Gemini, OpenRouter, or local Ollama model. You can also use the CLI Agent tab to run Claude Code, Codex, Gemini CLI, or Aider in a terminal."
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
                    auto_cp,
                    &mut checkpoint_done,
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
                    auto_cp,
                    &mut checkpoint_done,
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
                    auto_cp,
                    &mut checkpoint_done,
                    &tx,
                )
                .await;
            }
        }

        send_sse(&tx, "done", json!({})).await;
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn run_openai_loop(
    state: &AppState,
    ws: &Path,
    req: &AgentChatReq,
    api_key: &str,
    auto_cp: bool,
    checkpoint_done: &mut bool,
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
            let result = exec_tool(state, ws, &name, &args, checkpoint_done, auto_cp, tx).await;
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
    auto_cp: bool,
    checkpoint_done: &mut bool,
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
                    let result = exec_tool(state, ws, &name, &input, checkpoint_done, auto_cp, tx).await;
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
    auto_cp: bool,
    checkpoint_done: &mut bool,
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
                let result = exec_tool(state, ws, &name, &args, checkpoint_done, auto_cp, tx).await;
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
