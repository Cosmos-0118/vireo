# Vireo

A lightweight code workbench built around coding agents. It borrows the feel of VS Code but skips Electron and heavy language servers, so it starts fast and stays light.

## Features

- **Live diff viewer.** Side-by-side and unified diffs with word-level highlights. Diffs refresh automatically as files change on disk. You can stage, unstage, or discard each file, and send any diff or line to the agent with one click.
- **Code editor.** CodeMirror 6 with syntax highlighting, `Ctrl+S` to save, and a quick toggle between code and diff views.
- **Agent panel** with two modes:
  - **Agent Chat** is a built-in coding agent that can read, edit, and write files, search code, list files, and run commands. It works with OpenAI, OpenRouter, DeepSeek, Ollama, Anthropic Claude, and Google Gemini. Every turn creates a Git checkpoint, so you can roll a turn back in one click.
  - **CLI Agent** is an embedded terminal (Windows ConPTY) with launchers for `claude`, `codex`, `gemini`, `aider`, and `opencode`, sitting right beside the diff viewer.
- **Integrated terminal.** Multiple tabs powered by `xterm.js` (WebGL) and Rust `portable-pty`. Shells keep running while the panel is hidden or the page reloads, and the terminal bar can switch the engine to Ghostty (WASM). Project tasks are detected from `package.json`, `Cargo.toml`, and Python projects and can be run in one click. Terminal output can be sent straight to the agent.
- **Git and GitHub.** Stage, unstage, and discard changes, switch or create branches, commit, commit and push, pull, browse recent commits, and create checkpoints.
- **Themes.** Six built-in themes (VS Code Dark+, Tokyo Night, Catppuccin Mocha, One Dark Pro, GitHub Dark Dimmed, and GitHub Light). You can also import any VS Code `.json` theme.
- **Quick Open and search.** `Ctrl+P` opens files, themes, and tasks. `Ctrl+Shift+F` searches the workspace with ripgrep.

## Quick Start

**Build and launch (recommended):**
```powershell
.\build-and-run.ps1 "C:\path\to\your\repo"
```
Optional flags: `-CleanAll` runs a full `cargo clean` first, and `-BuildOnly` builds without launching.

The script finds the Visual Studio C++ build tools itself (via `msvc-env.ps1`), so it works from a plain PowerShell window. To run `cargo` by hand on Windows, load the same environment first: `. .\msvc-env.ps1; Enter-MsvcEnv`.

**Launch an existing build:**
```powershell
.\vireo.ps1 "C:\path\to\your\repo"
```

**Build and run by hand:**

```powershell
# Build the frontend
npm.cmd --prefix frontend run build

# Build and start the desktop app
cargo run --release --manifest-path backend/Cargo.toml -- --dir "C:\path\to\your\repo" --port 4317
```

To use Vireo in a browser instead of the desktop window, add `--server-only` and open `http://127.0.0.1:4317`.
