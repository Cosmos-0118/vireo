# Contributing to Vireo

Thanks for helping improve Vireo. Bug reports, ideas, documentation fixes, and pull requests are all welcome.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## Ways to help

- **Report a bug.** [Open a bug report](https://github.com/Cosmos-0118/vireo/issues/new/choose) with your Vireo version, how you installed it, your OS version, and steps to reproduce.
- **Suggest a feature.** Open a feature request explaining the workflow problem first, before proposing a solution.
- **Send a pull request.** For anything larger than a small fix, open an issue first so we can agree on the approach before you spend time on it.

## Set up a development environment

You need:

- **Windows 10 (build 19041 / 2004) or later (x64)** with WebView2 Runtime (included in Windows 11 and modern Windows 10).
- **Visual Studio Build Tools** with the *Desktop development with C++* workload (for MSVC `cl.exe` / `link.exe`).
- **[Rust toolchain](https://rustup.rs/)** (stable, `edition = 2021`).
- **[Node.js](https://nodejs.org/)** 22 LTS or newer (with `npm`).
- **Git** (and optionally [`ripgrep`](https://github.com/BurntSushi/ripgrep) and [`ast-grep`](https://ast-grep.github.io/) for workspace search and structural queries).

### Build and run locally

```powershell
git clone https://github.com/Cosmos-0118/vireo.git
cd vireo

# Builds the frontend, compiles the Rust/Tauri release binary, and opens Vireo on a repo
.\build-and-run.ps1 "C:\path\to\your\repo"
```

`build-and-run.ps1` loads the MSVC toolchain automatically via `msvc-env.ps1`. If you want to invoke `cargo` directly from a plain PowerShell terminal, load the MSVC environment first:

```powershell
. .\msvc-env.ps1; Enter-MsvcEnv
```

For fast frontend iteration against a running backend:

```powershell
# Terminal 1 — backend in browser/server mode on port 4317
. .\msvc-env.ps1; Enter-MsvcEnv
cargo run --manifest-path backend/Cargo.toml -- --dir "C:\path\to\your\repo" --server-only --port 4317

# Terminal 2 — Vite dev server with HMR (proxies /api and /ws to 127.0.0.1:4317)
npm.cmd --prefix frontend run dev
```

### Project layout

| Folder | What is in it |
| --- | --- |
| `backend/` | Rust backend, Tauri v2 desktop shell, embedded Axum HTTP/WebSocket server, Git & checkpoint engine, agent & CLI runners, PTY manager, and `vx` code-intelligence CLI (`backend/src/bin/vx.rs`). |
| `frontend/` | React 19 + TypeScript + Vite + Tailwind CSS v4 workbench UI (CodeMirror 6 editor, live diff viewer, xterm.js WebGL & Ghostty WASM terminals, commit graph, agent panel). |
| `installer/` | Inno Setup 6 script (`Vireo.iss`), installer build script (`build.ps1`), and automated install/launch/uninstall smoke test (`smoke-test.ps1`). |
| `scripts/` | Release staging (`stage-release.ps1`) and README/checksum integrity automation (`update-release-integrity.ps1`). |
| `.github/` | CI, release, CodeQL workflows, Dependabot config, and issue/PR templates. |

## Run checks and tests

Before opening a pull request, run the frontend build and backend tests:

```powershell
# 1. Type-check and build the frontend bundle (embedded into the Rust binary via rust-embed)
npm.cmd --prefix frontend ci
npm.cmd --prefix frontend run build

# 2. Run Rust unit tests (covers both `vireo` and `vx`)
. .\msvc-env.ps1; Enter-MsvcEnv
cargo test --manifest-path backend/Cargo.toml --locked
```

Optional end-to-end pipeline test for the `vx` CLI (requires Git Bash / bash):

```bash
cargo build --bins --manifest-path backend/Cargo.toml --locked
bash backend/tests/vx_pipeline.sh
```

## Making a change

1. Fork the repository and create a branch from `main` named for the change, for example `fix/diff-scroll` or `feat/git-stash`.
2. Keep the change focused. One fix or feature per pull request is easier to review and revert.
3. Follow the formatting in [`.editorconfig`](.editorconfig) (4 spaces for Rust/PowerShell, 2 spaces for TypeScript/CSS/JSON/YAML).
4. Keep lockfiles in sync: if you change `backend/Cargo.toml` or `frontend/package.json`, commit the updated `backend/Cargo.lock` or `frontend/package-lock.json` too.
5. Never commit `.env` files, API keys, or local credentials.
6. Write clear commit messages that describe what changed (a `feat:`, `fix:`, `docs:`, or `chore:` prefix is preferred).
7. Run the build and tests before opening the PR, and fill in the pull request template.

## Continuous integration

Every pull request and push to `main` runs the `CI` workflow:

- **Frontend** — installs locked npm dependencies, runs TypeScript checking (`tsc -b`), and builds the Vite bundle.
- **Backend** — builds the frontend, runs `cargo test --locked`, and builds the release binaries (`vireo.exe` and `vx.exe`).
- **Installer** — packages the release binaries into the Inno Setup installer and runs the automated install/health/uninstall smoke test.

The required branch-protection check is **CI result**. CodeQL also scans both the TypeScript frontend and Rust backend for security issues.

## License

By contributing, you agree that your contributions are licensed under the project's [MIT license](LICENSE).
