//! Command-line flags for the Claude Code and Codex sessions.

use super::{CliKind, PermMode};
use serde_json::json;
use std::path::Path;

/// Flags that hand the CLI the `vx` tools (on PATH and as an inline stdio MCP server, so edits never
/// fight PowerShell/Bash quoting) plus the Vireo environment hint. Antigravity (`agy`) has no inline
/// MCP or system-prompt flag and receives the hint through the turn prompt instead.
pub(crate) fn vx_args(kind: CliKind, ws: &Path, mode: PermMode) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(dir) = crate::pty::vx_dir() {
        let vx_exe = dir
            .join(if cfg!(windows) { "vx.exe" } else { "vx" })
            .to_string_lossy()
            .replace('\\', "/");
        let ws_slash = ws.to_string_lossy().replace('\\', "/");
        match kind {
            CliKind::Claude => {
                let mcp_cfg = json!({
                    "mcpServers": {
                        "vx": {
                            "command": vx_exe,
                            "args": ["mcp", "--workspace", ws_slash]
                        }
                    }
                });
                args.push("--mcp-config".to_string());
                args.push(mcp_cfg.to_string());
                args.push("--append-system-prompt".to_string());
                args.push(crate::code_intel::AGENT_HINT.to_string());
                args.push("--allowedTools".to_string());
                // Claude Code's shell tool is `PowerShell` or `Bash` on Windows (depending on setup) and `Bash` elsewhere.
                let shell = if cfg!(windows) { "Bash(vx:*),PowerShell(vx:*)" } else { "Bash(vx:*)" };
                args.push(match mode {
                    PermMode::Ask => format!("{shell},mcp__vx__map,mcp__vx__view,mcp__vx__search"),
                    PermMode::AutoEdit | PermMode::FullAccess => format!("{shell},mcp__vx__*"),
                });
            }
            CliKind::Codex => {
                args.push("-c".to_string());
                let quoted = serde_json::to_string(crate::code_intel::AGENT_HINT).unwrap_or_default();
                args.push(format!("developer_instructions={quoted}"));
                let mut mcp_args = vec!["mcp".to_string(), "--workspace".to_string(), ws_slash];
                if mode == PermMode::Ask {
                    mcp_args.push("--read-only".to_string());
                }
                args.push("-c".to_string());
                args.push(format!(
                    "mcp_servers.vx.command={}",
                    serde_json::to_string(&vx_exe).unwrap_or_default()
                ));
                args.push("-c".to_string());
                args.push(format!(
                    "mcp_servers.vx.args={}",
                    serde_json::to_string(&mcp_args).unwrap_or_default()
                ));
                args.push("-c".to_string());
                args.push("mcp_servers.vx.enabled=true".to_string());
                args.push("-c".to_string());
                args.push("mcp_servers.vx.required=false".to_string());
            }
            _ => {}
        }
    }
    args
}

/// Claude Code kept alive across turns: stream-json both ways, token streaming, and tool
/// approvals answered through `control_request`s. Vireo owns the session id so it can resume it.
pub(crate) fn claude_session_args(
    ws: &Path,
    model: &str,
    mode: PermMode,
    session_id: &str,
    resume: bool,
) -> Vec<String> {
    let pm = match mode {
        PermMode::Ask => "default",
        PermMode::AutoEdit => "acceptEdits",
        PermMode::FullAccess => "bypassPermissions",
    };
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        pm,
        if resume { "--resume" } else { "--session-id" },
        session_id,
    ]
    .map(String::from)
    .to_vec();
    // The auto-allow list is fixed at launch (the mode can change mid-session), so it only holds
    // the read-only `vx` tools; edits by `vx` are auto-approved by Vireo itself outside Ask mode.
    args.extend(vx_args(CliKind::Claude, ws, PermMode::Ask));
    let model = model.trim();
    if !model.is_empty() && !model.eq_ignore_ascii_case("default") {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args
}

/// `codex app-server` (JSON-RPC over stdio). Model, sandbox and approval policy are chosen per
/// thread / turn, so only the `vx` tools (read-only in Ask mode) and Vireo hint are set here.
pub(crate) fn codex_app_server_args(ws: &Path, mode: PermMode) -> Vec<String> {
    let mut args = vec!["app-server".to_string()];
    args.extend(vx_args(CliKind::Codex, ws, mode));
    args
}
