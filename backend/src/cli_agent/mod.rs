//! Claude Code, Codex and GitHub Copilot as chat agents: the CLI definitions, flags, event
//! normalization and permission rules. The long-lived processes themselves live in
//! [`crate::agent_session`].

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CliKind {
    Claude,
    Codex,
    Copilot,
}

impl CliKind {
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "claude-cli" => Some(Self::Claude),
            "codex-cli" => Some(Self::Codex),
            "copilot-cli" => Some(Self::Copilot),
            _ => None,
        }
    }

    pub(crate) fn cmds(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Copilot => &["copilot"],
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Copilot => "GitHub Copilot",
        }
    }

    pub(crate) fn install_hint(self) -> &'static str {
        match self {
            Self::Claude => "irm https://claude.ai/install.ps1 | iex   (macOS/Linux: curl -fsSL https://claude.ai/install.sh | bash)",
            Self::Codex => "npm install -g @openai/codex",
            Self::Copilot => "winget install GitHub.Copilot   (or: npm install -g @github/copilot)",
        }
    }
}

/// How much the CLI may do without asking, chosen per request in the composer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PermMode {
    /// Ask before edits and shell commands.
    Ask,
    /// Auto-accept workspace edits; ask for anything else (default).
    AutoEdit,
    /// No prompts, no sandbox.
    FullAccess,
}

impl PermMode {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            Some("ask") => Self::Ask,
            Some("full_access") => Self::FullAccess,
            _ => Self::AutoEdit,
        }
    }
}

mod args;
mod emit;
mod normalize;
mod permissions;
mod prompt;
mod tools;
mod wire;

#[cfg(test)]
mod tests;

pub(crate) use args::{claude_session_args, codex_app_server_args};
pub(crate) use emit::emit;
pub(crate) use normalize::{Normalizer, Out};
pub use permissions::answer_permission;
pub(crate) use permissions::{permission_rule, permission_summary, rule_allows, PendingGuard};
pub(crate) use prompt::build_prompt;
pub(crate) use tools::{map_vx, map_vx_mcp_tool, rel_path, simplify_command, tail_chars};
pub(crate) use wire::{control_error, control_request, control_response, steer_user_message_line, user_message_line};
