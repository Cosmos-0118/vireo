//! Slash-command discovery: turns each agent's own command / skill listing into palette entries.

use super::types::SlashCommand;
use serde_json::Value;

/// Claude Code commands that only make sense in its interactive terminal UI.
const CLAUDE_TUI_ONLY: &[&str] = &[
    "login", "logout", "doctor", "config", "settings", "permissions", "ide", "terminal-setup", "vim", "theme",
    "statusline", "exit", "quit", "help", "bug", "feedback", "upgrade", "release-notes", "status", "usage",
    "resume", "export", "copy", "mobile", "desktop", "tui", "keybindings", "color", "rename", "btw",
];

/// From the `initialize` control response (`response.response`).
pub fn from_claude_init(init: &Value) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = init["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let name = c["name"].as_str()?.trim().trim_start_matches('/').to_string();
            if name.is_empty() || CLAUDE_TUI_ONLY.contains(&name.as_str()) {
                return None;
            }
            let source = if c["builtin"].as_bool() == Some(true) {
                "builtin"
            } else if name.contains(':') {
                "skill"
            } else {
                "project"
            };
            Some(SlashCommand {
                description: c["description"].as_str().unwrap_or("").trim().to_string(),
                hint: c["argumentHint"].as_str().filter(|h| !h.is_empty()).map(str::to_string),
                source: source.to_string(),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Commands Vireo itself maps onto Codex `app-server` requests.
pub fn codex_builtin() -> Vec<SlashCommand> {
    vec![
        SlashCommand {
            name: "compact".into(),
            description: "Summarize the conversation to free up context".into(),
            hint: None,
            source: "builtin".into(),
        },
        SlashCommand {
            name: "review".into(),
            description: "Review the uncommitted changes".into(),
            hint: Some("[instructions]".into()),
            source: "builtin".into(),
        },
    ]
}

/// From a `skills/list` response (`result`).
pub fn from_codex_skills(result: &Value) -> Vec<SlashCommand> {
    let mut out = codex_builtin();
    for group in result["data"].as_array().into_iter().flatten() {
        for s in group["skills"].as_array().into_iter().flatten() {
            if s["enabled"].as_bool() == Some(false) {
                continue;
            }
            let Some(name) = s["name"].as_str().filter(|n| !n.is_empty()) else { continue };
            let description = s["interface"]["shortDescription"]
                .as_str()
                .or_else(|| s["description"].as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            out.push(SlashCommand { name: name.to_string(), description, hint: None, source: "skill".into() });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Skill name -> `SKILL.md` path from a `skills/list` response; Codex needs the path to run a skill.
pub fn codex_skill_paths(result: &Value) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for group in result["data"].as_array().into_iter().flatten() {
        for s in group["skills"].as_array().into_iter().flatten() {
            if let (Some(name), Some(path)) = (s["name"].as_str(), s["path"].as_str()) {
                map.insert(name.to_string(), path.to_string());
            }
        }
    }
    map
}

/// From an ACP `available_commands_update` (`availableCommands`).
pub fn from_acp(list: &Value) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let name = c["name"].as_str()?.trim().trim_start_matches('/').to_string();
            (!name.is_empty()).then(|| SlashCommand {
                description: c["description"].as_str().unwrap_or("").trim().to_string(),
                hint: c["input"]["hint"].as_str().filter(|h| !h.is_empty()).map(str::to_string),
                source: "builtin".into(),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// If `text` is `/name args` for a known command, returns `(name, args)`.
pub fn parse_invocation<'a>(text: &'a str, known: &[SlashCommand]) -> Option<(&'a str, &'a str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    known.iter().any(|c| c.name == name).then_some((name, args.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_commands_hide_tui_only_entries_and_sort() {
        let init = json!({ "commands": [
            { "name": "login", "description": "Sign in" },
            { "name": "design:ux-copy", "description": "(design) Write copy", "argumentHint": "<context>" },
            { "name": "compact", "description": "Clear history", "builtin": true },
            { "name": "deploy", "description": "Ship it", "argumentHint": "" },
        ]});
        let cmds = from_claude_init(&init);
        let names: Vec<_> = cmds.iter().map(|c| (c.name.as_str(), c.source.as_str())).collect();
        assert_eq!(names, [("compact", "builtin"), ("deploy", "project"), ("design:ux-copy", "skill")]);
        assert_eq!(cmds[2].hint.as_deref(), Some("<context>"));
        assert_eq!(cmds[1].hint, None);
    }

    #[test]
    fn codex_skills_join_the_builtins() {
        let res = json!({ "data": [{ "cwd": "C:/w", "skills": [
            { "name": "codex-security:assess-patch-risk", "description": "Long text", "interface": { "shortDescription": "Assess patch impact" } },
            { "name": "off", "description": "x", "enabled": false },
        ]}]});
        let cmds = from_codex_skills(&res);
        let names: Vec<_> = cmds.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["codex-security:assess-patch-risk", "compact", "review"]);
        assert_eq!(cmds[0].description, "Assess patch impact");
    }

    #[test]
    fn invocations_only_match_known_commands() {
        let known = codex_builtin();
        assert_eq!(parse_invocation("/review focus on auth", &known), Some(("review", "focus on auth")));
        assert_eq!(parse_invocation("  /compact", &known), Some(("compact", "")));
        assert_eq!(parse_invocation("/etc/hosts is a path", &known), None);
        assert_eq!(parse_invocation("hello", &known), None);
    }
}
