use super::normalize::{Normalizer, Out};
use super::permissions::*;
use super::prompt::*;
use super::tools::*;
use super::wire::*;
use super::PermMode;
use crate::agent::ChatMessage;
use serde_json::{json, Value};
use std::path::Path;

fn feed_all(ws: &str, lines: &[&str]) -> Vec<Out> {
    let mut n = Normalizer::new(Path::new(ws));
    lines
        .iter()
        .flat_map(|l| n.feed(&serde_json::from_str::<Value>(l).unwrap()))
        .collect()
}

#[test]
fn claude_tools_are_normalized() {
    let out = feed_all(
        r"C:\proj",
        &[
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"model":"claude-x","content":[{"type":"text","text":"Editing."},{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"C:\\proj\\src\\a.ts","old_string":"a","new_string":"b"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"text","text":"ok"}]}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#,
        ],
    );
    assert_eq!(out[0], Out::Text("Editing.".into()));
    assert_eq!(
        out[1],
        Out::ToolCall { id: "tu1".into(), name: "edit_file".into(), args: json!({"path": "src/a.ts"}) }
    );
    assert_eq!(
        out[2],
        Out::ToolResult { id: "tu1".into(), name: "edit_file".into(), result: "ok".into() }
    );
    assert_eq!(out.len(), 3);
}

#[test]
fn claude_powershell_maps_to_run_command() {
    let out = feed_all(
        r"C:\proj",
        &[
            r#"{"type":"assistant","message":{"model":"claude-x","content":[{"type":"tool_use","id":"tu2","name":"PowerShell","input":{"command":"Get-ChildItem"}}]}}"#,
        ],
    );
    assert_eq!(
        out[0],
        Out::ToolCall { id: "tu2".into(), name: "run_command".into(), args: json!({"command": "Get-ChildItem"}) }
    );
}

#[test]
fn claude_not_logged_in_is_single_error() {
    // Captured from claude 2.1.328 when no login exists.
    let out = feed_all(
        r"C:\proj",
        &[
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"Not logged in · Please run /login"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
        ],
    );
    assert_eq!(out, vec![Out::Error("Not logged in · Please run /login".into())]);
}

#[test]
fn prompt_includes_history() {
    let m = |r: &str, c: &str| ChatMessage { role: r.into(), content: c.into() };
    assert_eq!(build_prompt(&[m("user", "hi")]), "hi");
    let p = build_prompt(&[m("user", "a"), m("assistant", "b"), m("user", "c")]);
    assert!(p.contains("User: a") && p.contains("Assistant: b") && p.ends_with("c"));
}

#[test]
fn claude_control_requests_are_normalized() {
    let out = feed_all(
        "C:/w",
        &[
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"C:\\Users\\u\\.claude\\CLAUDE.md"}}}"#,
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"hook_callback"}}"#,
            r#"{"type":"control_cancel_request","request_id":"r1"}"#,
        ],
    );
    assert!(matches!(&out[0], Out::PermissionRequest { request_id, tool, .. } if request_id == "r1" && tool == "Edit"));
    assert_eq!(out[1], Out::UnsupportedControl { request_id: "r2".into() });
    assert_eq!(out[2], Out::PermissionCancel { request_id: "r1".into() });
}

#[test]
fn control_response_shapes() {
    let input = json!({ "command": "ls" });
    let allow: Value = serde_json::from_str(&control_response("r1", true, &input)).unwrap();
    assert_eq!(allow["response"]["request_id"], "r1");
    assert_eq!(allow["response"]["response"]["behavior"], "allow");
    assert_eq!(allow["response"]["response"]["updatedInput"], input);
    let deny: Value = serde_json::from_str(&control_response("r1", false, &input)).unwrap();
    assert_eq!(deny["response"]["response"]["behavior"], "deny");
    assert!(deny["response"]["response"]["message"].is_string());
}

#[test]
fn remembered_rules_are_narrow() {
    let edit = json!({ "file_path": "C:\\Users\\U\\.claude\\CLAUDE.md" });
    let rule = permission_rule("Edit", &edit);
    assert_eq!(rule, "Edit:dir:c:/users/u/.claude");
    assert!(rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude/settings.json" })));
    assert!(!rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude-other/x" })));
    assert!(!rule_allows(&rule, "Edit", &json!({ "file_path": "C:/Users/u/.claude/../.ssh/id" })));
    assert!(!rule_allows(&rule, "Write", &edit));

    let sh = permission_rule("Bash", &json!({ "command": "cargo build" }));
    assert!(rule_allows(&sh, "Bash", &json!({ "command": "cargo test -p x" })));
    assert!(!rule_allows(&sh, "Bash", &json!({ "command": "cargo build && rm -rf x" })));
    assert!(!rule_allows(&sh, "Bash", &json!({ "command": "rm -rf x" })));
    assert!(rule_allows("WebFetch", "WebFetch", &json!({ "url": "https://x" })));
}

#[test]
fn vx_shell_calls_map_to_chat_tools() {
    let ws = Path::new("C:/w");
    let (n, a) = map_vx("vx edit C:/w/src/a.rs 5#ab..9#cd <<'EOF'
new
EOF", ws).unwrap();
    assert_eq!((n.as_str(), a["path"].as_str()), ("edit_file", Some("src/a.rs")));
    let (n, a) = map_vx("@'
x
'@ | vx symbol src/a.rs Foo::bar --replace", ws).unwrap();
    assert_eq!((n.as_str(), a["mode"].as_str()), ("symbol_edit", Some("replace")));
    let (n, a) = map_vx("vx view src/a.rs 10..20", ws).unwrap();
    assert_eq!((n.as_str(), a["start_line"].as_u64(), a["end_line"].as_u64()), ("read_file", Some(10), Some(20)));
    let (n, a) = map_vx("vx apply --file patch.txt", ws).unwrap();
    assert_eq!((n.as_str(), a["path"].as_str()), ("edit_file", Some("")));
    let (n, a) = map_vx("vx check cargo test -p vireo", ws).unwrap();
    assert_eq!((n.as_str(), a["command"].as_str()), ("verify", Some("cargo test vireo")));
    assert!(map_vx("cargo build", ws).is_none());

    // MCP tool calls from Claude Code map onto the same UI vocabulary.
    let (n, a) = map_claude_tool("mcp__vx__view", &json!({ "path": "C:/w/src/a.rs", "outline": true }), ws);
    assert_eq!((n.as_str(), a["path"].as_str()), ("outline", Some("src/a.rs")));
    let (n, a) = map_claude_tool("mcp__vx__apply", &json!({ "patch": "*** Begin Patch..." }), ws);
    assert_eq!((n.as_str(), a["path"].as_str()), ("edit_file", Some("")));
    let (n, a) = map_claude_tool("mcp__vx__check", &json!({ "cmd": "cargo test" }), ws);
    assert_eq!((n.as_str(), a["command"].as_str()), ("verify", Some("cargo test")));
}

#[test]
fn claude_streaming_deltas_do_not_repeat_in_the_final_message() {
    let ws = "C:/w";
    let out = feed_all(
        ws,
        &[
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}},"parent_tool_use_id":null}"#,
            r#"{"type":"assistant","message":{"id":"m1","model":"claude-haiku","content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"C:/w/a.rs"}}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Done"}},"parent_tool_use_id":null}"#,
            r#"{"type":"assistant","message":{"id":"m2","model":"claude-haiku","content":[{"type":"text","text":"Done"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Done"}"#,
        ],
    );
    assert_eq!(
        out,
        vec![
            Out::TextDelta("Hel".into()),
            Out::TextDelta("lo".into()),
            Out::ToolCall { id: "t1".into(), name: "read_file".into(), args: json!({ "path": "a.rs" }) },
            Out::TextDelta("\n\nDone".into()),
        ]
    );
}

#[test]
fn claude_slash_command_output_comes_from_the_result() {
    let out = feed_all(
        "C:/w",
        &[
            r#"{"type":"assistant","message":{"id":"x","model":"<synthetic>","content":[{"type":"text","text":"Context Usage"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Context Usage\n| a | b |"}"#,
        ],
    );
    assert_eq!(out, vec![Out::Text("Context Usage\n| a | b |".into())]);
    // An ordinary reply that already streamed is not repeated from `result`.
    let out = feed_all(
        "C:/w",
        &[
            r#"{"type":"assistant","message":{"id":"y","model":"claude-haiku","content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"hi"}"#,
        ],
    );
    assert_eq!(out, vec![Out::Text("hi".into())]);
}

#[test]
fn claude_session_args_keep_the_session_alive_and_resumable() {
    use super::args::claude_session_args;
    let ws = Path::new("C:/w");
    let fresh = claude_session_args(ws, "default", PermMode::Ask, "sid-1", false);
    assert!(fresh.windows(2).any(|w| w == ["--session-id", "sid-1"]));
    assert!(fresh.contains(&"--include-partial-messages".to_string()));
    assert!(!fresh.contains(&"--no-session-persistence".to_string()));
    assert!(!fresh.contains(&"--model".to_string()));
    let resumed = claude_session_args(ws, "opus", PermMode::AutoEdit, "sid-1", true);
    assert!(resumed.windows(2).any(|w| w == ["--resume", "sid-1"]));
    assert!(resumed.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
    assert!(resumed.windows(2).any(|w| w == ["--model", "opus"]));
}

#[test]
fn claude_normalizer_resets_between_auto_chained_turns() {
    let mut n = Normalizer::new(Path::new("C:/w"));
    let feed = |n: &mut Normalizer, s: &str| n.feed(&serde_json::from_str::<Value>(s).unwrap());

    assert_eq!(
        feed(&mut n, r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}},"parent_tool_use_id":null}"#),
        vec![]
    );
    assert_eq!(
        feed(&mut n, r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"First"}},"parent_tool_use_id":null}"#),
        vec![Out::TextDelta("First".into())]
    );
    assert_eq!(
        feed(&mut n, r#"{"type":"result","subtype":"success","is_error":false,"result":"First","queued_turn_count":1}"#),
        vec![]
    );
    assert!(n.finished);

    // Chained Turn 2 arrives on stdout before pump() calls begin_turn():
    // must NOT prepend "\n\n" and must clear `finished`.
    assert_eq!(
        feed(&mut n, r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}},"parent_tool_use_id":null}"#),
        vec![]
    );
    assert!(!n.finished);
    assert_eq!(
        feed(&mut n, r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Second"}},"parent_tool_use_id":null}"#),
        vec![Out::TextDelta("Second".into())]
    );
}

#[test]
fn steer_user_message_includes_uuid_and_priority() {
    let v: Value = serde_json::from_str(&steer_user_message_line("pivot", "u-123", "next")).unwrap();
    assert_eq!(v["type"], "user");
    assert_eq!(v["uuid"], "u-123");
    assert_eq!(v["priority"], "next");
    assert_eq!(v["message"]["content"][0]["text"], "pivot");
}
