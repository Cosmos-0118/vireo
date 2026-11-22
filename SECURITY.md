# Security Policy

Vireo runs a local HTTP/WebSocket backend, spawns terminal PTYs, executes Git commands, and integrates with coding agents and external CLI tools, so security reports are taken seriously. Thank you for reporting problems responsibly.

## Supported versions

| Version | Supported |
| --- | --- |
| Latest release (`0.2.x` on `main`) | ✅ Security fixes go into the next release |
| Older releases | ❌ Please update to the latest release |

Fixes are released as a new version rather than patched into older releases, so please update to the [latest release](https://github.com/Cosmos-0118/vireo/releases/latest) before reporting.

## How to report a vulnerability

**Please do not open a public issue, pull request, or discussion for a security problem.**

1. **Preferred:** report it privately through GitHub at [Security → Report a vulnerability](https://github.com/Cosmos-0118/vireo/security/advisories/new). This keeps the report, discussion, and fix in one private place.
2. **If you cannot use GitHub:** email [dhanush13244@gmail.com](mailto:dhanush13244@gmail.com) with `"Vireo security"` in the subject.

Please include:

- Your Vireo version and how you installed it (installer, portable ZIP, or built from source).
- Your OS version and build.
- What an attacker needs to start with (for example, a malicious repository, a crafted file, or another local process/browser tab) and what they gain.
- Steps to reproduce, and a proof of concept if you have one.

Review any logs or screenshots before attaching them so API keys, tokens, or private repository details are not exposed.

## What happens next

Vireo is maintained by a small team, so these are targets rather than guarantees:

- We aim to acknowledge a report within **7 days**.
- We will confirm whether we can reproduce the issue and keep you updated while we work on a fix.
- Once a fixed release is published, we publish a GitHub security advisory and credit you unless you prefer to stay anonymous.

Please give us a reasonable chance to release a fix before sharing details publicly.

## Scope

Reports are most useful for the parts of Vireo that enforce workspace, execution, or release boundaries:

- **Local HTTP/WebSocket server** (`127.0.0.1`), including cross-origin or DNS-rebinding abuse against `/api/*`, `/ws/pty`, or `/ws/events`.
- **Workspace path boundaries** in the file APIs and the `vx` CLI (`code_intel.rs`), including path-traversal escapes outside the active workspace.
- **Agent and CLI tool execution**, including unintended command execution, permission-prompt bypasses, or argument injection in Git/CLI runners.
- **Credential handling**, including accidental leakage of model provider API keys into logs, checkpoints, or error outputs.
- **Release integrity**: the installer, the portable ZIP, `SHA256SUMS.txt`, and build provenance attestations.

### Out of scope

- Vulnerabilities in third-party CLI agents (`claude`, `codex`, `gemini`, `aider`, `opencode`, etc.) or model provider APIs — please report those to the respective vendor (and let us know if Vireo makes the impact worse).
- Attacks that already require arbitrary local code execution as the current user, physical access to an unlocked machine, or denial of service against your own machine.
