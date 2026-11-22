# LiteCode

A lightweight, agent-first code workbench inspired by VS Code — without Electron or heavy language-server overhead.

## What's Inside
- **Live Diff Viewer** — Side-by-side & unified diffs with word-level change highlights, automatic live refresh via filesystem watcher, per-file Stage/Unstage/Discard, and 1-click "Ask Agent" on any diff or line.
- **Quick Code Editor** — CodeMirror 6 with syntax highlighting, `Ctrl+S` save, and instant diff/code toggle.
- **Agent Panel (Dual Mode)**
  - **Agent Chat**: Built-in tool-using coding agent (`read_file`, `edit_file`, `write_file`, `search_code`, `list_files`, `run_command`) supporting OpenAI / OpenRouter / DeepSeek / Ollama, Anthropic Claude, and Google Gemini — with automatic Git checkpoints for 1-click turn rollback.
  - **CLI Agent**: Embedded native Windows ConPTY terminal tab with quick launchers for `claude`, `codex`, `gemini`, `aider`, and `opencode` right next to the live diff viewer.
- **Integrated Terminal** — Multi-tab `xterm.js` + Rust `portable-pty` terminal with **One-Click Run Tasks** (auto-detected from `package.json` / `Cargo.toml` / Python) and **Send Output to Agent**.
- **Git & GitHub Workflow** — Stage/unstage/discard, branch switcher & creator, Commit, **Commit & Push**, Pull, recent commits log, and instant Git checkpoints.
- **Themes** — 6 built-in themes (`VS Code Dark+`, `Tokyo Night`, `Catppuccin Mocha`, `One Dark Pro`, `GitHub Dark Dimmed`, `GitHub Light`) + **Import any VS Code `.json` theme file**.
- **Quick Open (`Ctrl+P`) & Ripgrep Search (`Ctrl+Shift+F`)**.

## Quick Start

```powershell
# Run as a standalone desktop-style window (opens in Edge App Mode)
.\litecode.ps1 -Dir "C:\path\to\your\repo"
```

Or run manually:

```powershell
# Build frontend
npm.cmd --prefix frontend run build

# Build & start Rust server
cargo run --release --manifest-path backend/Cargo.toml -- --dir "C:\path\to\your\repo" --port 4317
```

Then open `http://127.0.0.1:4317`.
