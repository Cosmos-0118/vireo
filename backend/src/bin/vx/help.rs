//! CLI help and usage text for `vx`.

pub const USAGE: &str = "vx <map|outline|view|search|apply|replace|edit|symbol|create|ast|check|release|mcp> ...   (vx help for details)\n\
  vx map [query] [--dir D]\n\
  vx outline <file>\n\
  vx view <file> [symbol|A..B,C..D]  |  vx view <file:A..B> <file2:C..D> ...\n\
  vx search <regex> [path] [-g G]... [-p P] [-C N] [-F] [-i] [-l] [--limit N]\n\
  vx apply [--file F] [--dry-run]                                                    (V4A / anchored patch on stdin or --file)\n\
  vx replace <old> <new> [path] [-g G]... [-r] [--dry-run] [--expected N] [--all-files]\n\
  vx edit <file> A#h[..B#h]|top|bottom|imports [--insert-after|--insert-before|--delete] [--text T|--file F]\n\
  vx symbol <file> <name> [--replace|--insert-after|--delete] [--text T|--file F]\n\
  vx create <file> [--force] [--text T|--file F]\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
  vx check [--raw] [--warnings] [--timeout S] [--dir D] [--cmd C | cmd args...]\n\
  vx release [status|preflight|notes|bump|draft|publish|verify|activity] [patch|minor|major|prerelease|X.Y.Z] [--dry-run] [--skip-tests] [--write] [--changelog] [--notes-file F] [--publish] [--no-push] [--asset G]... [--only FILE]... [--json]\n\
  vx mcp [--workspace DIR] [--read-only]";

pub const HELP: &str = "vx - token-lean code tools: hash-anchored view/edit, atomic patches & replace, smart search, symbol ops, repo map, build check, MCP server\n\
\n\
  vx map [query] [--dir D]        ranked file/symbol map; a query narrows by file or symbol name\n\
  vx outline <file> [file2 ...]     symbols as [start#hash..end#hash] ranges (markdown: headings; auto-resolves unique paths)\n\
  vx view <file> [symbol|A..B,C..D]  (also `vx view <file> 10..20 40..50` or `<file> Foo Bar`)\n\
                                  lines as N#hash|code; accepts comma-separated ranges/symbols or batch `file:A..B file2:C..D` (the output budget is shared across files)\n\
  vx search <regex> [path] [-g G]... [-C N] [-F] [-i] [-l] [--limit N]\n\
                                  anchored search (`file:N#hash| text`); auto-recovers monorepo paths/globs (e.g. `src/**` -> `**/src/**`),\n\
                                  `-C N` prints anchored context lines, `-F` literal search, `-l` files with hit counts\n\
  vx apply [--file F] [--dry-run] atomic multi-file/multi-hunk V4A patch (anchored `@@ A#h..B#h` or context `@@` hunks; `vx help apply`)\n\
  vx replace <old> <new> [path] [-g G]... [-r] [--dry-run] [--expected N] [--all-files]\n\
                                  atomic literal (default) or regex (`-r`; groups `$1`/`\\1`, `^`/`$` per line) replace across 1 or many files with syntax checks & CRLF preservation\n\
  vx edit <file> A#h[..B#h]       replace those lines with new code from stdin, --file F, or --text\n\
  vx edit <file> top|bottom|imports   insert there with no anchors (`imports`: after the import block, skips duplicates)\n\
         [--insert-after|--insert-before|--delete]   (aliases --after/--before/--remove work; unknown flags are rejected)\n\
  vx symbol <file> <name>         print a whole function/class; add --replace|--insert-after|--delete to change it\n\
  vx create <file> [--force]      new file from stdin, --file F, or --text (--force overwrites)\n\
  vx ast '<pattern>' [-r '<rewrite>' [--dry-run]] [-l lang] [path]\n\
                                  ast-grep structural search; -r rewrites files IN PLACE, --dry-run only lists\n\
  vx check [--raw] [--warnings] [--timeout S] [--dir D] [cmd ...]\n\
                                  run cargo/tsc/go or any command (e.g. `vx check cargo test -p x`); deduplicated anchored errors, rustc notes/labels, failing test blocks; exits 1 on failure\n\
  vx release [action] [version]   polyglot release engine (Rust/Node/Tauri/.NET/Python/Java/Go/installers); exits 1 when a gate fails\n\
      status                      detected stacks, version files + drift, commits since last tag, suggested bump, CI mode & workflow notes wiring\n\
      preflight [ver] [--skip-tests]   gates: clean tree, branch, remote sync, free tag, version order, CI tag/notes wiring, gh auth, build/typecheck\n\
      notes [ver] [--write] [--changelog] [--notes-file F]   release notes + diffstat context (Conventional Commits + fallback; warns on existing tags; --write saves clean draft; --changelog prepends CHANGELOG.md)\n\
      bump <patch|minor|major|prerelease|X.Y.Z> [--dry-run] [--only FILE]...   atomic, lossless version sync of every version file + lockfile\n\
      draft [ver] [--dry-run] [--notes-file F]   stage a private GitHub DRAFT release from the notes file (no bump, tag or push); in CI mode a tag push publishes publicly, so draft first\n\
      publish [ver] [--dry-run] [--no-push] [--publish] [--skip-tests] [--notes-file F] [--asset GLOB]...\n\
                                  preflight -> bump -> commit -> tag -> push (CI-tag repos) or draft GitHub release (--publish makes it public). ALWAYS --dry-run first.\n\
      verify [ver]                remote tag, GitHub release state + assets, CI run for the tag\n\
      activity                    recent release actions (Releases page, agents, CLI), newest first\n\
      Custom version files / assets / checks: `.vireo/release.json` ({\"files\":[{\"path\",\"pattern\"}],\"exclude\":[],\"assets\":[],\"checks\":[],\"mode\":\"ci|gh|local\"}).\n\
  vx mcp [--workspace DIR] [--read-only]\n\
                                  stdio MCP JSON-RPC 2.0 server exposing vx tools with zero shell quoting\n\
\n\
Workflow: map or search -> outline / view (copy N#hh anchors) -> apply / replace / edit / symbol -> check.\n\
Prefer `vx apply` or `vx replace` over Python/sed scripts for multi-file changes. On Windows, prefer `vx mcp` or `--file F` over piping multi-line code.\n\
Every command accepts the same paths: exact (cwd or workspace root) first, else a unique suffix/basename match (noted in the output).\n\
A custom `check` command runs in --dir, else the current folder, else the one top-level folder holding its manifest (package.json, Cargo.toml, ...).\n\
Edits report new anchors and flag new syntax errors immediately; no need to re-read after editing.";

pub const HELP_APPLY: &str = "vx apply [--file F] [--dry-run]  (reads patch from --file F or stdin)\n\
\n\
Applies many hunks across many files atomically (all validated in memory first; nothing written on error).\n\
Uses the V4A patch envelope with hashline-anchored hunks (preferred) or classic context hunks:\n\
\n\
  *** Begin Patch                  (optional if starting directly with `*** Update File:` / `*** Add File:`)\n\
  *** Update File: src/app.rs\n\
  @@ 12#ab..15#cd                  # replace lines 12..15 with the `+` lines (no old text, no `-`/context lines)\n\
  +let x = 2;\n\
  @@ after 40#ef                   # insert `+` lines after line 40 (also: `before 40#ef`)\n\
  +log(\"ok\");\n\
  @@ delete 50#aa..52#bb           # remove lines 50..52\n\
  @@ after imports                 # add `+` lines after the import block (already-present lines are skipped)\n\
  +use std::fmt;\n\
  @@ top                           # also `@@ bottom`; `top` stays below a shebang / `//!` / 'use client'\n\
  +// header\n\
  @@ symbol Foo::bar               # replace the whole symbol with the `+` lines (also: `after symbol Foo`, `before symbol Foo`, `delete symbol Foo`)\n\
  +fn bar() {}\n\
  @@ fn helper() {                 # classic V4A context hunk: ' ' context, '-' remove, '+' add\n\
   let a = 1;\n\
  -let b = 2;\n\
  +let b = 3;\n\
  *** Add File: src/new.rs\n\
  +pub fn hi() {}\n\
  *** Delete File: src/old.rs\n\
  *** End Patch\n\
\n\
All anchors in a patch refer to the file BEFORE the patch (so anchors from one `vx view` stay valid for every hunk).";
