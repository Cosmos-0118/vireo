use super::check::{compress_plain, env_hint, parse_diags, render_diags, test_summary};
use super::search::builtin_search;
use super::symbols::{find_symbol, qualified};
use super::*;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vireo-ci-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const RS: &str = "use std::fmt;\n\n/// Doc for foo\n#[inline]\nfn foo(x: i32) -> i32 {\n    x + 1\n}\n\nstruct Bar;\n\nimpl Bar {\n    fn baz(&self) -> bool {\n        true\n    }\n}\n";

#[test]
fn outline_and_symbol_ranges() {
    let ws = tmp("outline");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    let o = outline(&ws, "a.rs", None).unwrap();
    assert!(o.contains("fn foo(x: i32) -> i32"), "{o}");
    assert!(o.contains("  ["), "nested method expected: {o}");
    let syms = symbols(Lang::Rust, RS);
    let foo = find_symbol(&syms, "fn foo").unwrap();
    assert_eq!((foo.start, foo.end), (2, 6), "doc comment + attribute are included");
    let baz = find_symbol(&syms, "Bar::baz").unwrap();
    assert_eq!(baz.container.as_deref(), Some("Bar"));
}

#[test]
fn hashline_edit_replace_drift_and_syntax_warning() {
    let ws = tmp("edit");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    let v = view(&ws, "a.rs", Some("5..7"), None).unwrap();
    let l5 = v.lines().nth(1).unwrap().split('|').next().unwrap().to_string();
    let l7 = v.lines().nth(3).unwrap().split('|').next().unwrap().to_string();
    let mut shifted = String::from("// a\n// b\n// c\n");
    shifted.push_str(RS);
    std::fs::write(ws.join("a.rs"), &shifted).unwrap();
    let r = hashline_edit(&ws, "a.rs", "replace", &l5, Some(&l7), "fn foo(x: i32) -> i32 {\n    x + 2\n}", None).unwrap();
    assert!(r.contains("lines 8..10"), "{r}");
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("x + 2"));
    let anchor = format!("8#{}", line_hash("fn foo(x: i32) -> i32 {"));
    let r = hashline_edit(&ws, "a.rs", "replace", &anchor, None, "fn foo(x: i32) -> i32 {{{", None).unwrap();
    assert!(r.contains("syntax error"), "{r}");
}

#[test]
fn stale_and_malformed_anchors_show_live_snippets() {
    let ws = tmp("stale");
    let lines: String = (1..=30).map(|i| format!("let v{i} = {i};\n")).collect();
    std::fs::write(ws.join("a.rs"), &lines).unwrap();
    // Stale single anchor
    let err = hashline_edit(&ws, "a.rs", "replace", "5#ff", None, "x", None).unwrap_err();
    assert!(err.contains("no longer match") && err.contains("let v5 = 5;"), "{err}");
    // Malformed hex anchor shows real lines around line 12
    let err = hashline_edit(&ws, "a.rs", "replace", "12#zz", None, "x", None).unwrap_err();
    assert!(err.contains("bad anchor") && err.contains("let v12 = 12;"), "{err}");
    // Distant end anchor typo shows lines near line 25 (not clipped to start!)
    let s1 = format!("1#{}", line_hash("let v1 = 1;"));
    let err = hashline_edit(&ws, "a.rs", "replace", &s1, Some("25#00"), "x", None).unwrap_err();
    assert!(err.contains("end anchor") && err.contains("let v25 = 25;"), "{err}");
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), lines);
}

#[test]
fn crlf_is_preserved_and_prefixes_stripped() {
    let ws = tmp("crlf");
    std::fs::write(ws.join("a.ts"), "const a = 1;\r\nconst b = 2;\r\n").unwrap();
    let anchor = format!("1#{}", line_hash("const a = 1;"));
    hashline_edit(&ws, "a.ts", "replace", &anchor, None, &format!("1#{}|const a = 9;\nconst c = 3;", line_hash("x")), None)
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.join("a.ts")).unwrap(),
        "const a = 9;\r\nconst c = 3;\r\nconst b = 2;\r\n"
    );
}

#[test]
fn symbol_replace_and_ts_outline() {
    let ws = tmp("sym");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    let r = symbol_op(&ws, "a.rs", "Bar::baz", "replace", "    fn baz(&self) -> bool {\n        false\n    }", None).unwrap();
    assert!(r.contains("Replaced Bar::baz"), "{r}");
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("false"));
    let ts = "export interface A { x: number }\n/** doc */\nexport const f = (a: number) => a;\nexport class K {\n  m() { return 1; }\n}\n";
    std::fs::write(ws.join("b.ts"), ts).unwrap();
    let o = outline(&ws, "b.ts", None).unwrap();
    assert!(o.contains("interface A") && o.contains("const f") && o.contains("class K") && o.contains("  ["), "{o}");
    let py = "import os\n\n@dec\ndef f():\n    pass\n\nclass C:\n    def m(self):\n        pass\n";
    std::fs::write(ws.join("c.py"), py).unwrap();
    let o = outline(&ws, "c.py", None).unwrap();
    assert!(o.contains("def f") && o.contains("class C") && o.contains("def m"), "{o}");
}

#[test]
fn repo_map_ranks_by_query() {
    let ws = tmp("map");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    std::fs::write(ws.join("b.rs"), "fn other() {}\n").unwrap();
    let m = repo_map(&ws, Some("baz"), None).unwrap();
    assert!(m.starts_with("a.rs") && !m.contains("b.rs"), "{m}");
    assert!(repo_map(&ws, None, None).unwrap().contains("b.rs"));
}

#[test]
fn diagnostics_are_compressed() {
    let ws = tmp("diag");
    std::fs::write(ws.join("a.ts"), "let a: number = 'x';\nlet b = 2;\n").unwrap();
    let raw = "a.ts(1,5): error TS2322: Type 'string' is not assignable to type 'number'.\na.ts(1,5): error TS2322: Type 'string' is not assignable to type 'number'.\n";
    let d = parse_diags(raw, "");
    assert_eq!(d.len(), 1, "deduped");
    let r = render_diags(&ws, &d, "tsc", 0.5, false);
    assert!(
        r.contains("a.ts:1:5 [TS2322]")
            && (r.contains(&format!(">  1#{}|", line_hash("let a: number = 'x';"))) || r.contains("1#")),
        "{r}"
    );
}

#[test]
fn markdown_outline_skips_code_fences() {
    let ws = tmp("md");
    std::fs::write(ws.join("R.md"), "# T\n\n```\n# not a heading\n```\n\n## Sec\ntext\n").unwrap();
    let r = outline(&ws, "R.md", None).unwrap();
    assert!(r.contains("] # T"), "{r}");
    assert!(r.contains("] ## Sec"), "{r}");
    assert!(!r.contains("not a heading"), "{r}");
}

#[test]
fn builtin_search_smart_case_and_globs() {
    let ws = tmp("bs");
    std::fs::create_dir_all(ws.join("src/deep")).unwrap();
    std::fs::write(ws.join("src/deep/a.rs"), "fn Alpha() {}\nfn beta() {}\n").unwrap();
    std::fs::write(ws.join("b.py"), "alpha = 1\n").unwrap();
    let (rs, _) = builtin_search(&ws, "alpha", &[], &["**/*.rs".into()], &SearchOpts::default()).unwrap();
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(rs[0].0, "src/deep/a.rs");
    let (both, _) = builtin_search(&ws, "alpha", &[], &["*.{py,rs}".into()], &SearchOpts::default()).unwrap();
    assert_eq!(both.len(), 2);
    assert!(builtin_search(&ws, "fn (", &[], &[], &SearchOpts::default()).is_err());
}

#[test]
fn search_recovers_monorepo_globs_paths_context_literal_and_files() {
    let ws = tmp("search-flex");
    std::fs::create_dir_all(ws.join("frontend/src/components")).unwrap();
    std::fs::create_dir_all(ws.join("backend/src")).unwrap();
    std::fs::write(
        ws.join("frontend/src/components/BranchBar.tsx"),
        "const title = 'hi';\nconst badge = render(a.b[0]);\nconst tail = 1;\n",
    )
    .unwrap();
    std::fs::write(ws.join("backend/src/main.rs"), "fn main() { /* badge */ }\n").unwrap();

    // Exact agent failure #1: `--glob "src/**"` from monorepo root auto-recovers via `**/src/**`.
    let out = search(
        &ws,
        "badge",
        &SearchOpts { globs: vec!["src/**".into()], ..Default::default() },
    )
    .unwrap();
    assert!(out.contains("frontend/src/components/BranchBar.tsx"), "{out}");
    assert!(out.contains("**/src/**"), "should tell agent how it recovered: {out}");

    // Exact agent failure #2: `--glob "src/components/BranchBar.tsx"` resolves to the file!
    let out = search(
        &ws,
        "badge",
        &SearchOpts { globs: vec!["src/components/BranchBar.tsx".into()], ..Default::default() },
    )
    .unwrap();
    assert!(out.contains("frontend/src/components/BranchBar.tsx") && !out.contains("main.rs"), "{out}");

    // Literal search (`-F`) handles regex metacharacters cleanly.
    let lit = search(
        &ws,
        "render(a.b[0])",
        &SearchOpts { literal: true, path: Some("frontend".into()), ..Default::default() },
    )
    .unwrap();
    // Narrow searches include anchored context, grouped under a `# file` header.
    assert!(lit.contains("BranchBar.tsx") && lit.contains("2#") && lit.contains("|const badge"), "{lit}");

    // Context (`-C 1`) returns anchored surrounding lines.
    let ctx = search(
        &ws,
        "badge",
        &SearchOpts { context: 1, path: Some("frontend".into()), ..Default::default() },
    )
    .unwrap();
    assert!(ctx.contains("1#") && ctx.contains("2#") && ctx.contains("3#"), "{ctx}");

    // Files-only (`-l`) returns file hit counts.
    let fl = search(&ws, "badge", &SearchOpts { files_only: true, ..Default::default() }).unwrap();
    assert!(fl.contains("BranchBar.tsx (1 hit)") && fl.contains("main.rs (1 hit)"), "{fl}");
}

#[test]
fn view_errors_smart_resolve_multi_range_and_batch() {
    let ws = tmp("ve");
    std::fs::create_dir_all(ws.join("frontend/src/components")).unwrap();
    std::fs::create_dir_all(ws.join("d")).unwrap();
    std::fs::write(ws.join("a.rs"), "l1\nl2\nl3\nl4\nl5\nl6\n").unwrap();
    std::fs::write(ws.join("frontend/src/components/BranchBar.tsx"), "export const B = 1;\n").unwrap();

    assert!(view(&ws, "a.rs", Some("50..90"), None).unwrap_err().contains("past the end"));
    assert!(view(&ws, "nope.rs", None, None).unwrap_err().contains("not found"));
    assert!(view(&ws, "d", None, None).unwrap_err().contains("directory"));

    // Unique suffix auto-resolution on read-only view.
    let smart = view(&ws, "src/components/BranchBar.tsx", None, None).unwrap();
    assert!(smart.contains("resolved") && smart.contains("export const B = 1;"), "{smart}");

    // Comma-separated multi-range view in one file.
    let multi = view(&ws, "a.rs", Some("1..2,5..6"), None).unwrap();
    assert!(multi.contains("l1") && multi.contains("l2") && !multi.contains("l3") && multi.contains("l5") && multi.contains("l6"), "{multi}");

    // Batch view across files.
    let batch = view_many(&ws, &["a.rs:1..2".into(), "src/components/BranchBar.tsx".into()], None).unwrap();
    assert!(batch.contains("# a.rs") && batch.contains("BranchBar.tsx"), "{batch}");
}

#[test]
fn apply_patch_anchored_multi_hunk_and_atomic_failure() {
    let ws = tmp("apply-anchored");
    std::fs::write(ws.join("a.rs"), "fn one() -> i32 {\n    1\n}\n\nfn two() -> i32 {\n    2\n}\n").unwrap();
    std::fs::write(ws.join("b.rs"), "const X: i32 = 0;\n").unwrap();
    let l2 = format!("2#{}", line_hash("    1"));
    let l6 = format!("6#{}", line_hash("    2"));
    let b1 = format!("1#{}", line_hash("const X: i32 = 0;"));
    let bad_patch = format!(
        "*** Begin Patch\n*** Update File: a.rs\n@@ {l2}\n+    10\n*** Update File: b.rs\n@@ 1#ff\n+const X: i32 = 9;\n*** End Patch\n"
    );
    let err = apply_patch(&ws, &bad_patch, None, false).unwrap_err();
    assert!(err.contains("nothing was written"), "{err}");
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("    1\n"));

    let good_patch = format!(
        "*** Begin Patch\n\
         *** Update File: a.rs\n\
         @@ {l2}\n\
         +    10\n\
         +    + 1\n\
         @@ after {l6}\n\
         +    // added\n\
         *** Update File: b.rs\n\
         @@ {b1}\n\
         +const X: i32 = 42;\n\
         *** Add File: c.rs\n\
         +pub fn c() {{}}\n\
         *** End Patch\n"
    );
    let dry = apply_patch(&ws, &good_patch, None, true).unwrap();
    assert!(dry.contains("Dry run OK"), "{dry}");
    assert!(!ws.join("c.rs").exists());

    let ok = apply_patch(&ws, &good_patch, None, false).unwrap();
    assert!(ok.contains("Applied patch: 3 file(s)"), "{ok}");
    assert!(ok.contains("M a.rs  2 hunk(s)"), "{ok}");
    let a_after = std::fs::read_to_string(ws.join("a.rs")).unwrap();
    assert!(a_after.contains("    10\n    + 1\n") && a_after.contains("    2\n    // added\n"), "{a_after}");
    assert_eq!(std::fs::read_to_string(ws.join("b.rs")).unwrap(), "const X: i32 = 42;\n");
    assert_eq!(std::fs::read_to_string(ws.join("c.rs")).unwrap(), "pub fn c() {}\n");
}

#[test]
fn apply_patch_tolerates_diff_style_minus_lines_in_anchored_hunks() {
    let ws = tmp("apply-minus");
    std::fs::write(ws.join("a.rs"), "let a = 1;\nlet b = 2;\nlet c = 3;\n").unwrap();
    let b = format!("2#{}", line_hash("let b = 2;"));
    // `-old` echo next to `+new`: the anchor already names the replaced line.
    let ok = format!("*** Update File: a.rs\n@@ {b}\n-let b = 2;\n+let b = 20;\n");
    apply_patch(&ws, &ok, None, false).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), "let a = 1;\nlet b = 20;\nlet c = 3;\n");

    // Only `-` lines would silently delete the range: refused, file untouched.
    let b2 = format!("2#{}", line_hash("let b = 20;"));
    let only_minus = format!("*** Update File: a.rs\n@@ {b2}\n-let b = 20;\n");
    let err = apply_patch(&ws, &only_minus, None, false).unwrap_err();
    assert!(err.contains("no `+` lines") && err.contains("@@ delete"), "{err}");
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("let b = 20;"));

    // Context (space-prefixed) lines are still rejected, with a message that says what to do.
    let ctx = format!("*** Update File: a.rs\n@@ {b2}\n let a = 1;\n+let b = 5;\n");
    let err = apply_patch(&ws, &ctx, None, false).unwrap_err();
    assert!(err.contains("no `-` or context lines are needed"), "{err}");
}

#[test]
fn env_hint_names_the_missing_program() {
    let ps = "npx : The term 'npx' is not recognized as the name of a cmdlet, function, script file, or operable program.\n    + CategoryInfo          : ObjectNotFound: (npx:String) [], CommandNotFoundException\n";
    let h = env_hint(ps).unwrap();
    assert!(h.contains("`npx`") && h.contains("PATH"), "{h}");
    let cmd = "'node' is not recognized as an internal or external command,\n";
    assert!(env_hint(cmd).unwrap().contains("`node`"));
    assert!(env_hint("bash: cargo: command not found\n").unwrap().contains("`cargo`"));
    assert!(env_hint("error[E0308]: mismatched types\n").is_none());
}

#[test]
fn apply_patch_forgiving_envelope_and_bad_anchor_snippet() {
    let ws = tmp("apply-forgive");
    std::fs::write(ws.join("a.rs"), "let a = 1;\nlet b = 2;\n").unwrap();
    // Malformed anchored header gives live anchored lines near line 2 instead of context error.
    let bad = "*** Update File: a.rs\n@@ 2#zz\n+let b = 3;\n";
    let err = apply_patch(&ws, bad, None, false).unwrap_err();
    assert!(err.contains("bad anchor") && err.contains("let b = 2;"), "{err}");

    // Forgiving envelope (no `*** Begin Patch` / `*** End Patch`) succeeds when valid.
    let a2 = format!("2#{}", line_hash("let b = 2;"));
    let ok = apply_patch(&ws, &format!("*** Update File: a.rs\n@@ {a2}\n+let b = 3;\n"), None, false).unwrap();
    assert!(ok.contains("Applied patch: 1 file(s)"), "{ok}");
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), "let a = 1;\nlet b = 3;\n");
}

#[test]
fn apply_patch_context_reindents_and_preserves_unicode_context() {
    let ws = tmp("apply-ctx");
    std::fs::write(ws.join("a.py"), "def greet():\n    msg = \"hello — world\"\n    return msg\n").unwrap();
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Update File: a.py\n",
        "@@ def greet():\n",
        "   msg = \"hello ? world\"\n",
        "-  return msg\n",
        "+  return msg.upper()\n",
        "*** End Patch\n",
    );
    let out = apply_patch(&ws, patch, None, false).unwrap();
    assert!(out.contains("re-indented") && out.contains("non-ASCII"), "{out}");
    let after = std::fs::read_to_string(ws.join("a.py")).unwrap();
    assert_eq!(after, "def greet():\n    msg = \"hello — world\"\n    return msg.upper()\n");
}

#[test]
fn apply_patch_rejects_overlapping_hunks() {
    let ws = tmp("apply-overlap");
    std::fs::write(ws.join("a.rs"), "a\nb\nc\n").unwrap();
    let l1 = format!("1#{}", line_hash("a"));
    let l2 = format!("2#{}", line_hash("b"));
    let l3 = format!("3#{}", line_hash("c"));
    let patch = format!(
        "*** Begin Patch\n*** Update File: a.rs\n@@ {l1}..{l2}\n+x\n@@ {l2}..{l3}\n+y\n*** End Patch\n"
    );
    let err = apply_patch(&ws, &patch, None, false).unwrap_err();
    assert!(err.contains("touch the same lines"), "{err}");
}

#[test]
fn replace_text_multi_file_crlf_dry_run_and_expected_guard() {
    let ws = tmp("replace");
    std::fs::create_dir_all(ws.join("frontend/src")).unwrap();
    std::fs::write(ws.join("frontend/src/a.ts"), "const x = oldBadge(1);\r\nconst y = oldBadge(2);\r\n").unwrap();
    std::fs::write(ws.join("frontend/src/b.ts"), "export const z = oldBadge(3);\n").unwrap();

    // Unscoped write is rejected without --all-files or path/glob.
    assert!(replace_text(&ws, "oldBadge", "newBadge", &ReplaceOpts::default()).is_err());

    // Dry run previews without writing.
    let dry = replace_text(
        &ws,
        "oldBadge",
        "newBadge",
        &ReplaceOpts { path: Some("frontend/src".into()), dry_run: true, ..Default::default() },
    )
    .unwrap();
    assert!(dry.contains("Would replace 3 occurrence(s) in 2 file(s)"), "{dry}");
    assert!(std::fs::read_to_string(ws.join("frontend/src/a.ts")).unwrap().contains("oldBadge"));

    // Expected count mismatch aborts atomically.
    let exp_err = replace_text(
        &ws,
        "oldBadge",
        "newBadge",
        &ReplaceOpts { path: Some("frontend/src".into()), expected: Some(2), ..Default::default() },
    )
    .unwrap_err();
    assert!(exp_err.contains("expected 2") && exp_err.contains("found 3"), "{exp_err}");

    // Real replace updates both files and preserves CRLF in a.ts.
    let ok = replace_text(
        &ws,
        "oldBadge",
        "newBadge",
        &ReplaceOpts { path: Some("frontend/src".into()), expected: Some(3), ..Default::default() },
    )
    .unwrap();
    assert!(ok.contains("Replaced 3 occurrence(s) in 2 file(s)"), "{ok}");
    assert_eq!(
        std::fs::read_to_string(ws.join("frontend/src/a.ts")).unwrap(),
        "const x = newBadge(1);\r\nconst y = newBadge(2);\r\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("frontend/src/b.ts")).unwrap(),
        "export const z = newBadge(3);\n"
    );
}

#[test]
fn check_preserves_rustc_labels_notes_and_cargo_test_failures() {
    let ws = tmp("check-rich");
    std::fs::write(ws.join("a.rs"), "fn f() -> u32 {\n    \"nope\"\n}\n").unwrap();
    let cargo_json = json!({
        "reason": "compiler-message",
        "message": {
            "level": "error",
            "code": { "code": "E0308" },
            "message": "mismatched types",
            "spans": [
                { "file_name": "a.rs", "line_start": 2, "column_start": 5, "is_primary": true, "label": "expected `u32`, found `&str`" },
                { "file_name": "a.rs", "line_start": 1, "column_start": 11, "is_primary": false, "label": "expected `u32` because of return type" }
            ],
            "children": [
                { "level": "help", "message": "try parsing", "spans": [{ "suggested_replacement": "\"nope\".parse()" }] }
            ]
        }
    });
    let diags = parse_diags(&cargo_json.to_string(), "");
    assert_eq!(diags.len(), 1);
    let rendered = render_diags(&ws, &diags, "cargo check", 0.4, false);
    assert!(rendered.contains("expected `u32`, found `&str`"), "{rendered}");
    assert!(rendered.contains("because of return type"), "{rendered}");
    assert!(rendered.contains("help: try parsing"), "{rendered}");
    assert!(check_failed(&rendered));

    let test_out = "running 3 tests\ntest a ... ok\ntest b ... FAILED\n\nfailures:\n\n---- b stdout ----\nthread 'b' panicked at src/lib.rs:10:5:\nassertion `left == right` failed\n  left: 1\n right: 2\nnote: run with `RUST_BACKTRACE=1`\n\nfailures:\n    b\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured\n";
    assert_eq!(test_summary(test_out).as_deref(), Some("2 passed, 1 failed"));
    let comp = compress_plain(test_out);
    assert!(
        comp.contains("---- b stdout ----") && comp.contains("left: 1") && !comp.contains("test a ... ok"),
        "{comp}"
    );
}

#[test]
fn ast_builtin_search_rewrite_and_ignore() {
    let ws = tmp("ast");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join("gen")).unwrap();
    std::fs::write(ws.join(".gitignore"), "gen/\n").unwrap();
    std::fs::write(ws.join("src/a.rs"), "fn f() {\n    let v = foo(1, 2);\n    foo(3, bar(4));\n}\n").unwrap();
    std::fs::write(ws.join("gen/b.rs"), "fn g() { foo(9, 9); }\n").unwrap();
    std::fs::write(ws.join("src/c.py"), "def h():\n    return foo(5, 6)\n").unwrap();
    std::fs::write(ws.join("src/d.ts"), "const x = foo(7, 8);\n").unwrap();

    // Metavariables work in languages that need the expando placeholder (Rust, Python) and in
    // ones that do not (TypeScript); .gitignore'd files are skipped.
    let all = edit::ast(&ws, "foo($A, $B)", None, None, None, false, None).unwrap();
    assert!(all.starts_with("4 match(es)"), "{all}");
    assert!(all.contains("src/a.rs:2#") && all.contains("src/a.rs:3#"), "{all}");
    assert!(all.contains("src/c.py:2#") && all.contains("src/d.ts:1#"), "{all}");
    assert!(!all.contains("gen/b.rs"), "{all}");

    let py = edit::ast(&ws, "foo($$$ARGS)", Some("python"), None, None, false, None).unwrap();
    assert!(py.starts_with("1 match(es)") && py.contains("src/c.py"), "{py}");

    let dry = edit::ast(&ws, "foo($A, $B)", Some("rust"), Some("src"), Some("baz($B, $A)"), true, None).unwrap();
    assert!(dry.contains("dry run"), "{dry}");
    assert!(std::fs::read_to_string(ws.join("src/a.rs")).unwrap().contains("foo(1, 2)"));

    let done = edit::ast(&ws, "foo($A, $B)", Some("rs"), None, Some("baz($B, $A)"), false, None).unwrap();
    assert!(done.starts_with("Rewrote 2 match(es) in 1 file(s)"), "{done}");
    assert_eq!(
        std::fs::read_to_string(ws.join("src/a.rs")).unwrap(),
        "fn f() {\n    let v = baz(2, 1);\n    baz(bar(4), 3);\n}\n"
    );
    assert_eq!(std::fs::read_to_string(ws.join("gen/b.rs")).unwrap(), "fn g() { foo(9, 9); }\n");
    assert_eq!(edit::ast(&ws, "nothing_here($A)", Some("go"), None, None, false, None).unwrap(), "No matches.");
}

#[test]
fn add_file_accepts_raw_content_but_rejects_mixed() {
    let ws = tmp("add-raw");
    std::fs::write(ws.join("a.rs"), "fn one() {}\n").unwrap();
    let a1 = format!("1#{}", line_hash("fn one() {}"));
    // Raw (no `+`) Add File with inner blank lines, followed by another op: both applied.
    let patch = format!(
        "*** Begin Patch\n*** Add File: sub/n.ts\nimport x from 'y';\n\nexport const a = 1;\n\n*** Update File: a.rs\n@@ {a1}\n+fn uno() {{}}\n*** End Patch\n"
    );
    let out = apply_patch(&ws, &patch, None, false).unwrap();
    assert!(out.contains("new anchors:"), "{out}");
    assert_eq!(
        std::fs::read_to_string(ws.join("sub/n.ts")).unwrap().trim_end(),
        "import x from 'y';\n\nexport const a = 1;"
    );
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("fn uno"));

    // A single `+` among raw lines is ambiguous: reject and write nothing.
    let mixed = "*** Begin Patch\n*** Add File: m.txt\n+one\ntwo\n*** End Patch\n";
    let err = apply_patch(&ws, mixed, None, false).unwrap_err();
    assert!(err.contains("must all start with `+`"), "{err}");
    assert!(!ws.join("m.txt").exists());
}

#[test]
fn compress_collapses_repeated_build_noise_and_keeps_root_cause() {
    let mut raw = String::new();
    for c in ["tree-sitter@0.27.2", "tree-sitter-json@0.24.8", "tree-sitter-python@0.25.0"] {
        raw.push_str("  cargo:warning=Compiler family detection failed due to error: ToolNotFound: failed to find tool \"cl.exe\"\n");
        raw.push_str(&format!("warning: {c}: Compiler family detection failed due to error: ToolNotFound: failed to find tool \"cl.exe\"\n"));
        raw.push_str(&format!("error: failed to run custom build command for `{}`\n", c.split('@').next().unwrap()));
        raw.push_str("  CFLAGS = None\n  CRATE_CC_NO_DEFAULTS = None\n");
    }
    raw.push_str("error: could not compile `vireo` due to previous error\n");
    let c = compress_plain(&raw);
    assert_eq!(c.matches("Compiler family detection").count(), 1, "{c}");
    assert!(c.contains("(×6)"), "{c}");
    assert!(c.contains("could not compile `vireo`"), "{c}");
    assert!(c.lines().count() < 12, "{c}");
}

#[test]
fn check_reports_silent_passes_and_environment_causes() {
    let ws = tmp("check-quiet");
    let opts = check::CheckOpts::default();
    let ok = check::check(&ws, Some("exit 0"), 30, opts.clone()).unwrap();
    assert!(ok.contains("ok (") && ok.contains("exit 0, no output"), "{ok}");
    let chatty = check::check(&ws, Some("echo hello"), 30, opts.clone()).unwrap();
    assert!(chatty.contains("1 output line(s), last: hello"), "{chatty}");
    let env = check::check(&ws, Some("echo cl.exe not found && exit 1"), 30, opts.clone()).unwrap();
    assert!(env.contains("✖") && env.contains("hint: MSVC build tools"), "{env}");
    assert!(check::check_failed(&env));
}

#[test]
fn familiar_parameter_names_and_range_strings_work_first_try() {
    let ws = tmp("aliases");
    std::fs::write(ws.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    // Claude-Code style names for replace.
    let r = run_op(&ws, "replace", &json!({"file_path": "a.txt", "old_string": "two", "new_string": "TWO"})).unwrap();
    assert!(r.contains("Replaced 1"), "{r}");
    // search with `pattern`, edit with `new_string`.
    let s = run_op(&ws, "search", &json!({"pattern": "three"})).unwrap();
    assert!(s.contains("# a.txt") && s.contains("|three") && s.contains("4#"), "{s}");
    // `path:symbol-or-range` and Read-style offset/limit both work over the MCP-style `view`.
    let v = run_op(&ws, "view", &json!({"path": "a.txt:3..4"})).unwrap();
    assert!(v.contains("|three") && v.contains("|four") && !v.contains("|one"), "{v}");
    let v = run_op(&ws, "view", &json!({"file_path": "a.txt", "offset": 2, "limit": 2})).unwrap();
    assert!(v.contains("|TWO") && v.contains("|three") && !v.contains("|four"), "{v}");
    // A whole `N#hh..M#hh` range in `start`, as printed by view/outline.
    let range = format!("2#{}..3#{}", line_hash("TWO"), line_hash("three"));
    let e = run_op(&ws, "edit", &json!({"path": "a.txt", "start": range, "new_string": "mid"})).unwrap();
    assert!(e.contains("Edited a.txt lines 2..3"), "{e}");
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "one\nmid\nfour\n");
    // Bracketed range, copied straight from `outline`.
    let br = format!("[2#{}..2#{}]", line_hash("mid"), line_hash("mid"));
    run_op(&ws, "edit", &json!({"path": "a.txt", "start": br, "op": "delete"})).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "one\nfour\n");
    // A forgotten new_text must not silently delete lines.
    let a1 = format!("1#{}", line_hash("one"));
    let err = run_op(&ws, "edit", &json!({"path": "a.txt", "start": a1})).unwrap_err();
    assert!(err.contains("use op `delete`"), "{err}");
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "one\nfour\n");
}

#[test]
fn view_never_returns_zero_lines_for_a_huge_line() {
    let ws = tmp("viewlong");
    let huge = "x".repeat(60_000);
    std::fs::write(ws.join("min.js"), format!("{huge}\nsecond\n")).unwrap();
    let v = view(&ws, "min.js", None, None).unwrap();
    assert!(v.contains("1#") && v.contains("chars on this line]"), "first line shown, clipped");
    assert!(v.contains("second"), "{}", &v[v.len().saturating_sub(200)..]);
    assert!(v.len() < 8_000, "{}", v.len());
}

#[test]
fn human_rustc_output_is_parsed_with_label_and_zero_tests_flagged() {
    let raw = "warning: unused variable: `x`\n --> src/a.rs:3:9\n  |\n3 |     let x = 1;\n  |         ^ help: prefix with underscore\n\nerror[E0308]: mismatched types\n  --> src/lib.rs:12:5\n   |\n11 | fn bar() -> usize {\n12 |     \"hello\"\n   |     ^^^^^^^ expected `usize`, found `&str`\n\nerror: could not compile `vireo` due to 1 previous error\n";
    let d = parse_diags(raw, "");
    assert_eq!(d.len(), 2, "one error + one warning, and `could not compile` is not a located diagnostic");
    let ws = tmp("human-rustc");
    let r = render_diags(&ws, &d, "cargo test", 1.0, true);
    assert!(r.starts_with("✖ 1 error(s), 1 warning(s)"), "{r}");
    assert!(r.contains("src/lib.rs:12") && r.contains("E0308") && r.contains("expected `usize`"), "{r}");
    assert_eq!(
        test_summary("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out").as_deref(),
        Some("0 tests ran ⚠ (check the test name/filter)")
    );
    // Long failing output: root error at the top and the summary at the bottom both survive.
    let mut long = String::from("error[E0599]: no method named `frob`\n  --> src/a.rs:1:1\n");
    for i in 0..120 {
        long.push_str(&format!("note: filler line number {i} with enough text to be kept as distinct\n"));
    }
    long.push_str("error: could not compile `vireo`\n");
    let c = compress_plain(&long);
    assert!(c.contains("no method named `frob`") && c.contains("could not compile"), "{c}");
}

#[test]
fn replace_tolerates_indent_drift_in_one_file_and_hints_on_a_miss() {
    let ws = tmp("replace-fuzzy");
    std::fs::write(ws.join("a.rs"), "fn foo() {\n    let a = 1;\n    bar(a);\n}\n").unwrap();
    // Model forgot the 4-space indent; unique match is applied with the file's indentation.
    let opts = ReplaceOpts { path: Some("a.rs".into()), ..Default::default() };
    let r = replace_text(&ws, "let a = 1;\nbar(a);", "let a = 2;\nbaz(a);", &opts).unwrap();
    assert!(r.contains("matched ignoring indentation"), "{r}");
    assert_eq!(
        std::fs::read_to_string(ws.join("a.rs")).unwrap(),
        "fn foo() {\n    let a = 2;\n    baz(a);\n}\n"
    );
    // A real miss shows the closest anchored lines instead of a bare error.
    let err = replace_text(&ws, "fn foo() {\n    nothing_like_this();", "x", &opts).unwrap_err();
    assert!(err.contains("no matches found") && err.contains("Closest lines"), "{err}");
}

#[test]
fn symbol_can_be_pinned_by_line_and_anchor_range_survives_length_drift() {
    let src = "struct Foo;\n\nimpl Foo {\n    fn a(&self) {}\n}\n\nimpl Foo {\n    fn b(&self) {}\n}\n";
    let syms = symbols(Lang::Rust, src);
    let err = find_symbol(&syms, "impl Foo").unwrap_err();
    assert!(err.contains("ambiguous") && err.contains("Foo:3"), "{err}");
    let second = find_symbol(&syms, "impl Foo:7").unwrap();
    assert_eq!(second.start + 1, 7);

    // End anchor is two lines off after an earlier edit: still resolved.
    let ws = tmp("locate-drift");
    std::fs::write(ws.join("a.txt"), "head\nstart\nx\ny\nz\nend\ntail\n").unwrap();
    let s = format!("2#{}", line_hash("start"));
    let e = format!("7#{}", line_hash("end")); // really line 6
    let out = hashline_edit(&ws, "a.txt", "replace", &s, Some(&e), "S\nE", None).unwrap();
    assert!(out.contains("Edited"), "{out}");
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "head\nS\nE\ntail\n");
}

#[test]
fn glob_braces_work() {
    let m = super::search::GlobMatcher::compile(&["*.{ts,tsx}".to_string()]).unwrap();
    assert!(m.matches("src/a.tsx", "a.tsx") && m.matches("b.ts", "b.ts") && !m.matches("c.rs", "c.rs"));
}

#[test]
fn view_spec_splits_at_first_colon_and_keeps_drive_letters() {
    use super::split_view_spec as s;
    assert_eq!(s("a.rs:Foo:7"), ("a.rs", Some("Foo:7")));
    assert_eq!(s("a.rs:10..20"), ("a.rs", Some("10..20")));
    assert_eq!(s(r"C:\x\a.rs:Foo:7"), (r"C:\x\a.rs", Some("Foo:7")));
    assert_eq!(s("C:/x/a.rs"), ("C:/x/a.rs", None));
    assert_eq!(s("a.rs"), ("a.rs", None));
}

#[test]
fn search_falls_back_to_literal_and_ast_and_add_file_keep_crlf() {
    let ws = tmp("literal-fallback");
    std::fs::write(ws.join("a.txt"), "call foo(bar) now\nx = a.b[0]\n").unwrap();
    // `foo(bar)` as a regex matches "foobar", not the text: falls back to literal.
    let out = search(&ws, "foo(bar)", &SearchOpts::default()).unwrap();
    assert!(out.contains("showing literal matches") && out.contains("foo(bar)"), "{out}");
    // Unclosed group is an invalid regex: also literal.
    let out = search(&ws, "a.b[0", &SearchOpts::default()).unwrap();
    assert!(out.contains("showing literal matches"), "{out}");
    // Real regex hits are untouched.
    let out = search(&ws, r"foo\(bar\)",&SearchOpts::default()).unwrap();
    assert!(!out.contains("literal matches") && out.contains("foo(bar)"), "{out}");

    // `ast -r` keeps CRLF files CRLF.
    let ws = tmp("ast-crlf");
    std::fs::write(ws.join("a.rs"), "fn a() {\r\n    old(1);\r\n}\r\n").unwrap();
    let out = ast(&ws, "old($X)", Some("rust"), None, Some("new($X);\nnew2($X)"), false, None).unwrap();
    assert!(out.contains("a.rs"), "{out}");
    let s = std::fs::read(ws.join("a.rs")).unwrap();
    assert!(!s.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'), "{:?}", String::from_utf8_lossy(&s));

    // `*** Add File:` follows the neighbours' line endings.
    let ws = tmp("add-eol");
    std::fs::write(ws.join("old.rs"), "fn a() {}\r\nfn b() {}\r\n").unwrap();
    apply_patch(&ws, "*** Begin Patch\n*** Add File: new.rs\n+fn c() {}\n+fn d() {}\n*** End Patch\n", None, false).unwrap();
    assert_eq!(std::fs::read(ws.join("new.rs")).unwrap(), b"fn c() {}\r\nfn d() {}\r\n");
}

#[test]
fn edit_batch_is_atomic_and_edit_accepts_a_symbol() {
    let ws = tmp("edit-batch");
    std::fs::write(ws.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let h = |n: usize, t: &str| format!("{n}#{}", line_hash(t));
    let edits = serde_json::json!([
        { "start": h(1, "one"), "new_text": "ONE" },
        { "start": format!("{}..{}", h(3, "three"), h(4, "four")), "new_text": "34a\n34b" },
        { "op": "insert_after", "start": h(5, "five"), "new_text": "six" },
    ]);
    let out = edit_batch(&ws, "a.txt", edits.as_array().unwrap(), None).unwrap();
    assert!(out.contains("a.txt"), "{out}");
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "ONE\ntwo\n34a\n34b\nfive\nsix\n");

    // One bad entry: nothing is written.
    std::fs::write(ws.join("b.txt"), "x\ny\n").unwrap();
    let bad = serde_json::json!([
        { "start": h(1, "x"), "new_text": "X" },
        { "start": "2#zz", "new_text": "Y" },
    ]);
    assert!(edit_batch(&ws, "b.txt", bad.as_array().unwrap(), None).is_err());
    assert_eq!(std::fs::read_to_string(ws.join("b.txt")).unwrap(), "x\ny\n");

    // `edit` with `symbol` instead of anchors.
    std::fs::write(ws.join("s.rs"), "fn a() -> i32 {\n    1\n}\n\nfn b() {}\n").unwrap();
    let args = serde_json::json!({"path":"s.rs","symbol":"a","new_text":"fn a() -> i32 {\n    2\n}"});
    run_op(&ws, "edit", &args).unwrap();
    assert!(std::fs::read_to_string(ws.join("s.rs")).unwrap().contains("    2\n"));
}

#[test]
fn warning_text_does_not_hide_the_real_failure() {
    let raw = "warning: unused variable `expected`\n  --> src/a.rs:3:9\nwarning: field never read, error type\nerror[E0308]: mismatched types\n  --> src/a.rs:9:5\n";
    let out = compress_plain(raw);
    assert!(out.contains("E0308"), "{out}");
}

// ---------------------------------------------------------------------------------------------
// Languages beyond rs/ts/js/py/go
// ---------------------------------------------------------------------------------------------

/// `(container, name)` pairs of every symbol, e.g. `Foo.bar`, `baz` for top level.
fn names(lang: Lang, src: &str) -> Vec<String> {
    symbols(lang, src).iter().map(|s| qualified(s).replace("::", ".")).collect()
}

fn assert_has(lang: Lang, src: &str, expected: &[&str]) {
    let got = names(lang, src);
    for e in expected {
        assert!(got.iter().any(|g| g == e), "{lang:?}: missing `{e}` in {got:?}");
    }
}

#[test]
fn extensions_map_to_languages() {
    use super::symbols::lang_for;
    for (file, lang) in [
        ("A.cs", Lang::CSharp),
        ("A.java", Lang::Java),
        ("a.c", Lang::C),
        ("a.cpp", Lang::Cpp),
        ("a.h", Lang::Cpp),
        ("a.hpp", Lang::Cpp),
        ("a.rb", Lang::Ruby),
        ("a.php", Lang::Php),
        ("a.kt", Lang::Kotlin),
        ("a.sh", Lang::Bash),
    ] {
        assert_eq!(lang_for(file), Some(lang), "{file}");
    }
}

#[test]
fn csharp_outline_includes_namespaces_members_and_attributes() {
    let src = "namespace App.Core;\n\n/// <summary>Doc</summary>\n[Serializable]\npublic class Foo : Bar\n{\n    public int X { get; set; }\n\n    [Fact]\n    public void M(int a)\n    {\n    }\n\n    public Foo() { }\n\n    enum E { A, B }\n}\n\nnamespace Other\n{\n    public struct S { public void N() { } }\n    public interface I { void Q(); }\n    public record R(int A);\n}\n";
    assert_has(
        Lang::CSharp,
        src,
        &["Foo", "Foo.X", "Foo.M", "Foo.Foo", "Foo.E", "Other", "Other.S", "S.N", "Other.I", "I.Q", "Other.R"],
    );
    let syms = symbols(Lang::CSharp, src);
    let foo = syms.iter().find(|s| s.name == "Foo" && s.container.is_none()).unwrap();
    assert_eq!(foo.start, 2, "doc comment and attribute belong to the class");
    let m = syms.iter().find(|s| s.name == "M").unwrap();
    assert_eq!(m.sig, "public void M(int a)", "attribute line is skipped in the signature");
}

#[test]
fn csharp_symbol_replace_keeps_the_file_valid() {
    let ws = tmp("cs-replace");
    std::fs::write(ws.join("A.cs"), "class A\n{\n    int F() { return 1; }\n}\n").unwrap();
    let r = symbol_op(&ws, "A.cs", "A.F", "replace", "    int F() { return 2; }", None).unwrap();
    assert!(r.contains("Replaced"), "{r}");
    assert!(std::fs::read_to_string(ws.join("A.cs")).unwrap().contains("return 2;"));
    let broken = symbol_op(&ws, "A.cs", "A.F", "replace", "    int F() { return 3;", None).unwrap();
    assert!(broken.to_lowercase().contains("syntax"), "syntax check must run for C#: {broken}");
}

#[test]
fn java_outline() {
    let src = "package a;\n\n/** Doc */\n@Entity\npublic class Foo {\n    @Override\n    public void m(int a) {}\n    Foo() {}\n    enum E { A, B; void x() {} }\n    interface I { void q(); }\n    record R(int a) {}\n}\n";
    assert_has(Lang::Java, src, &["Foo", "Foo.m", "Foo.Foo", "Foo.E", "E.x", "Foo.I", "I.q", "Foo.R"]);
    let syms = symbols(Lang::Java, src);
    assert_eq!(syms.iter().find(|s| s.name == "m").unwrap().sig, "public void m(int a) {}");
}

#[test]
fn c_outline() {
    let src = "#include <x.h>\n#define M(x) x\nstruct Foo { int a; };\nstruct Fwd;\ntypedef struct { int b; } Bar;\nenum E { A };\nstatic int *foo(int a) { return 0; }\nint proto(int);\n#ifdef X\nvoid g(void) {}\n#endif\n";
    let got = names(Lang::C, src);
    for e in ["M", "Foo", "Bar", "E", "foo", "g"] {
        assert!(got.iter().any(|g| g == e), "missing `{e}` in {got:?}");
    }
    assert!(!got.iter().any(|g| g == "Fwd" || g == "proto"), "forward declarations and prototypes are not symbols: {got:?}");
}

#[test]
fn cpp_outline() {
    let src = "namespace n { class Foo : public B { public: void m() {} Foo(); int x; }; template<typename T> class Tpl { void q() {} }; }\nvoid Foo::m() {}\ntemplate<typename T> T add(T a, T b) { return a; }\nextern \"C\" { void cf() {} }\nstruct S { void z() {} };\n";
    assert_has(Lang::Cpp, src, &["n", "n.Foo", "Foo.m", "n.Tpl", "Tpl.q", "add", "cf", "S", "S.z"]);
}

#[test]
fn ruby_outline() {
    let src = "# doc\nmodule M\n  class Foo < Bar\n    def a(x); end\n    def self.b; end\n  end\nend\ndef top; end\n";
    assert_has(Lang::Ruby, src, &["M", "M.Foo", "Foo.a", "Foo.b", "top"]);
}

#[test]
fn php_outline() {
    let src = "<?php\nnamespace A;\n/** doc */\nclass Foo extends B { public function m($a) {} }\ninterface I { function q(); }\ntrait T { function t() {} }\nenum E { case A; }\nfunction top() {}\n";
    assert_has(Lang::Php, src, &["Foo", "Foo.m", "I", "I.q", "T", "T.t", "E", "top"]);
}

#[test]
fn kotlin_outline() {
    let src = "package a\n\n/** doc */\nclass Foo(val x: Int) : Bar() {\n    fun m(a: Int): Int = a\n    val p = 1\n    companion object {\n        fun c() {\n        }\n    }\n}\n\nobject O {\n    fun o() {\n    }\n}\n\nfun top() {\n}\n\nval tv = 1\ntypealias TA = Int\n";
    assert_has(Lang::Kotlin, src, &["Foo", "Foo.m", "Foo.p", "Foo.Companion", "Companion.c", "O", "O.o", "top", "tv", "TA"]);
}

#[test]
fn bash_outline() {
    assert_has(Lang::Bash, "# doc\nfoo() { echo hi; }\nfunction bar { echo; }\n", &["foo", "bar"]);
}

#[test]
fn repo_map_lists_new_languages() {
    let ws = tmp("map-langs");
    std::fs::write(ws.join("Svc.cs"), "class Svc { void Run() { } }\n").unwrap();
    std::fs::write(ws.join("App.java"), "class App { void go() { } }\n").unwrap();
    let m = repo_map(&ws, None, None).unwrap();
    assert!(m.contains("Svc.cs") && m.contains("App.java"), "{m}");
}

#[test]
fn structural_search_works_in_csharp_and_java() {
    let ws = tmp("ast-cs");
    std::fs::write(ws.join("A.cs"), "class A { void F() { Console.WriteLine(1); } }\n").unwrap();
    let r = edit::ast(&ws, "Console.WriteLine($A)", Some("csharp"), None, None, false, None).unwrap();
    assert!(r.contains("A.cs"), "{r}");
}

#[test]
fn writes_auto_resolve_unique_paths_like_view() {
    let ws = tmp("resolve-writes");
    let dir = ws.join("frontend/src/components");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Pane.tsx"), "const a = 1;\nconst b = 2;\n").unwrap();
    let a1 = format!("1#{}", line_hash("const a = 1;"));
    // Monorepo-relative path (as typed from `frontend/src`) resolves for edit, with a note.
    let r = hashline_edit(&ws, "components/Pane.tsx", "replace", &a1, None, "const a = 9;", None).unwrap();
    assert!(r.contains("resolved `components/Pane.tsx` -> `frontend/src/components/Pane.tsx`"), "{r}");
    // ... and for apply, edit batches and symbol reads.
    let p = "*** Begin Patch\n*** Update File: src/components/Pane.tsx\n@@ bottom\n+const c = 3;\n*** End Patch\n";
    let r = apply_patch(&ws, p, None, false).unwrap();
    assert!(r.contains("resolved"), "{r}");
    assert_eq!(std::fs::read_to_string(dir.join("Pane.tsx")).unwrap(), "const a = 9;\nconst b = 2;\nconst c = 3;\n");
    // Ambiguity is an error that lists the candidates, never a guess.
    std::fs::create_dir_all(ws.join("other")).unwrap();
    std::fs::write(ws.join("other/Pane.tsx"), "x\n").unwrap();
    let e = hashline_edit(&ws, "Pane.tsx", "replace", &a1, None, "y", None).unwrap_err();
    assert!(e.contains("Did you mean") && e.contains("other/Pane.tsx"), "{e}");
    // A patch naming one file twice via different spellings is rejected.
    let p2 = "*** Begin Patch\n*** Update File: frontend/src/components/Pane.tsx\n@@ top\n+// x\n*** Update File: src/components/Pane.tsx\n@@ bottom\n+// y\n*** End Patch\n";
    assert!(apply_patch(&ws, p2, None, false).unwrap_err().contains("appears twice"));
}

#[test]
fn new_files_land_next_to_existing_monorepo_folders() {
    let ws = tmp("resolve-new");
    std::fs::create_dir_all(ws.join("frontend/src/components/ui")).unwrap();
    let r = create(&ws, "components/ui/Empty.tsx", "export const E = 1;\n", false, None).unwrap();
    assert!(ws.join("frontend/src/components/ui/Empty.tsx").is_file(), "{r}");
    assert!(!ws.join("components").exists());
    let p = "*** Begin Patch\n*** Add File: src/components/ui/Row.tsx\n+export const R = 1;\n*** End Patch\n";
    apply_patch(&ws, p, None, false).unwrap();
    assert!(ws.join("frontend/src/components/ui/Row.tsx").is_file());
    // A genuinely new folder is still created where it was asked for.
    create(&ws, "newpkg/lib.rs", "fn a() {}\n", false, None).unwrap();
    assert!(ws.join("newpkg/lib.rs").is_file());
}

#[test]
fn edge_hunks_top_bottom_and_imports() {
    let ws = tmp("edges");
    std::fs::write(
        ws.join("a.ts"),
        "'use client';\nimport { a } from './a';\nimport {\n  b,\n} from './b';\n\nexport const x = a + b;\n",
    )
    .unwrap();
    std::fs::write(ws.join("b.rs"), "#!/usr/bin/env rust\n//! crate doc\nfn main() {}\n").unwrap();
    std::fs::write(ws.join("c.py"), "\"\"\"doc\"\"\"\nimport os\nfrom x import y\n\nprint(os)\n").unwrap();
    let p = "*** Begin Patch\n\
*** Update File: a.ts\n@@ after imports\n+import { c } from './c';\n+import { a } from './a';\n\
*** Update File: b.rs\n@@ top\n+use std::fmt;\n@@ bottom\n+fn tail() {}\n\
*** Update File: c.py\n@@ imports\n+import sys\n\
*** End Patch\n";
    let r = apply_patch(&ws, p, None, false).unwrap();
    assert!(r.contains("skipped 1 line(s) already present"), "{r}");
    assert_eq!(
        std::fs::read_to_string(ws.join("a.ts")).unwrap(),
        "'use client';\nimport { a } from './a';\nimport {\n  b,\n} from './b';\nimport { c } from './c';\n\nexport const x = a + b;\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("b.rs")).unwrap(),
        "#!/usr/bin/env rust\n//! crate doc\nuse std::fmt;\nfn main() {}\nfn tail() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("c.py")).unwrap(),
        "\"\"\"doc\"\"\"\nimport os\nfrom x import y\nimport sys\n\nprint(os)\n"
    );
    // Re-adding only existing imports is a no-op; `edit` takes the same keywords.
    let again = "*** Begin Patch\n*** Update File: c.py\n@@ after imports\n+import sys\n*** End Patch\n";
    assert!(apply_patch(&ws, again, None, false).unwrap().contains("already present"));
    let r = run_op(&ws, "edit", &serde_json::json!({ "path": "c.py", "start": "bottom", "new_text": "print(1)" })).unwrap();
    assert!(r.contains("M c.py"), "{r}");
    assert!(std::fs::read_to_string(ws.join("c.py")).unwrap().ends_with("print(os)\nprint(1)\n"));
}

#[test]
fn syntax_warnings_show_the_offending_lines() {
    let ws = tmp("syntax-ctx");
    std::fs::write(ws.join("a.rs"), "fn a() {\n    let x = 1;\n}\n").unwrap();
    let a2 = format!("2#{}", line_hash("    let x = 1;"));
    let r = hashline_edit(&ws, "a.rs", "replace", &a2, None, "    let x = (1;", None).unwrap();
    assert!(r.contains("⚠ new syntax error at"), "{r}");
    assert!(r.lines().any(|l| l.starts_with("> ")), "offending line marked: {r}");
}

#[test]
fn check_runs_custom_commands_in_the_right_folder() {
    let ws = tmp("check-dir");
    std::fs::create_dir_all(ws.join("web")).unwrap();
    std::fs::write(ws.join("web/package.json"), "{}").unwrap();
    std::fs::write(ws.join("web/marker.txt"), "WEBMARK").unwrap();
    let list = if cfg!(windows) { "type marker.txt" } else { "cat marker.txt" };
    // Explicit `dir`, auto-resolved by name.
    let opts = check::CheckOpts { dir: Some("web".into()), ..Default::default() };
    let r = check::check(&ws, Some(list), 30, opts).unwrap();
    assert!(r.contains("WEBMARK") && r.contains("[web]"), "{r}");
    // Inferred: `npm ...` with package.json only in `web/`.
    assert_eq!(check::infer_dir(&ws, "npm test").as_deref(), Some("web/"));
    assert_eq!(check::infer_dir(&ws, "npm --prefix web test"), None);
    assert_eq!(check::infer_dir(&ws, "echo hi"), None);
}

#[test]
fn search_lists_lockfile_hits_last() {
    let ws = tmp("search-noise");
    std::fs::write(ws.join("a.rs"), "// needle\n").unwrap();
    std::fs::write(ws.join("package-lock.json"), "needle\nneedle\nneedle\nneedle\n").unwrap();
    let files_only = super::search::SearchOpts { files_only: true, ..Default::default() };
    let r = super::search::search(&ws, "needle", &files_only).unwrap();
    let (src, lock) = (r.find("a.rs").unwrap(), r.find("package-lock.json").unwrap());
    assert!(src < lock, "source hit must precede the lockfile even with fewer hits: {r}");
    // Naming the lockfile in scope opts out of the demotion.
    let scoped = super::search::SearchOpts { path: Some("package-lock.json".into()), ..Default::default() };
    assert!(super::search::search(&ws, "needle", &scoped).unwrap().contains("package-lock.json"));
}

#[test]
fn view_continuation_hint_is_a_copyable_spec() {
    let ws = tmp("view-hint");
    let body: String = (1..=500).map(|i| format!("line {i}\n")).collect();
    std::fs::write(ws.join("big.txt"), body).unwrap();
    let v = view(&ws, "big.txt", None, None).unwrap();
    assert!(v.contains("continue with big.txt:401..500"), "{v}");
    assert!(!v.contains("vx view"), "hint must work for both CLI and MCP callers: {v}");
    // The hinted spec round-trips through the same entry point.
    let next = view(&ws, "big.txt:401..500", None, None).unwrap();
    assert!(next.contains("401#") && next.contains("500#"), "{next}");
}

#[test]
fn bare_repo_map_has_directory_overview() {
    let ws = tmp("map-overview");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/a.rs"), RS).unwrap();
    std::fs::write(ws.join("b.rs"), "fn other() {}\n").unwrap();
    let m = repo_map(&ws, None, None).unwrap();
    assert!(m.starts_with("# 2 source files (") && m.contains("src 1"), "{m}");
    assert!(!repo_map(&ws, Some("baz"), None).unwrap().starts_with("# "));
}

#[test]
fn anchors_copied_from_any_tool_parse() {
    use super::doc::parse_anchor;
    for s in ["12#a4", "12#a4|let x = 1;", "> 12#a4|", "[12#a4", "12#a4]", "src/a.rs:12#a4"] {
        assert_eq!(parse_anchor(s).unwrap(), (12, "a4".to_string()), "{s}");
    }
    assert_eq!(parse_anchor("110#5E-    code").unwrap(), (110, "5e".to_string()));
    assert!(parse_anchor("12#abc").is_err());
    assert!(parse_anchor("0#a4").is_err());
    assert_eq!(super::doc::text_to_lines("110#5e-    x\n> 12#a4|y"), vec!["    x".to_string(), "y".to_string()]);
}

#[test]
fn edit_accepts_old_string_without_anchor() {
    let ws = tmp("edit-old");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    let a = json!({"path": "a.rs", "old_string": "x + 1", "new_string": "x + 9"});
    run_op(&ws, "edit", &a).unwrap();
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("x + 9"));
}

#[test]
fn view_splits_inline_range_even_with_target() {
    let ws = tmp("view-inline");
    std::fs::write(ws.join("a.rs"), RS).unwrap();
    let v = view(&ws, "a.rs:5..7", Some("5..7"), None).unwrap();
    assert!(v.contains("lines 5..7"), "{v}");
}

#[test]
fn search_demotes_lockfiles_and_prose_below_source() {
    let ws = tmp("search-tier");
    std::fs::write(ws.join("a.rs"), "fn needle() {}\n").unwrap();
    std::fs::write(ws.join("Cargo.lock"), "needle\nneedle\n").unwrap();
    std::fs::write(ws.join("notes.txt"), "needle\n").unwrap();
    let out = search(&ws, "needle", &SearchOpts { limit: 80, ..Default::default() }).unwrap();
    let a = out.find("a.rs").unwrap();
    let t = out.find("notes.txt").unwrap();
    assert!(a < t, "{out}");
    assert!(out.contains("+2 hits in lockfiles/generated files: Cargo.lock"), "{out}");
    assert!(out.contains("#") && out.contains("fn needle"), "auto-context kept for source hits: {out}");
}

#[test]
fn locate_rejects_shifted_inner_closer_with_same_hash() {
    use super::doc::{locate, Doc};
    // An earlier edit grew the `if` block, so the stale end line (6) is now the inner `    }`.
    let doc = Doc::parse("fn foo() {\n    if a {\n        one();\n        two();\n        three();\n    }\n    bar();\n}\n");
    let s = (1, line_hash("fn foo() {"));
    let e = (6, line_hash("}"));
    assert_eq!(locate(&doc, "a.rs", &s, &e).unwrap(), (0, 7));
    // Unshifted: the exact match is still taken.
    let doc = Doc::parse("fn foo() {\n    if a {\n        one();\n    }\n    bar();\n}\n");
    assert_eq!(locate(&doc, "a.rs", &(1, line_hash("fn foo() {")), &(6, line_hash("}"))).unwrap(), (0, 5));
}

#[test]
fn malformed_prefixed_anchor_still_shows_live_lines() {
    use super::doc::{parse_anchor_in_doc, Doc};
    let doc = Doc::parse("a\nb\nc\n");
    for raw in ["src/a.rs:2#zz", "[2#zz", "> 2#zz", "2#zz"] {
        let err = parse_anchor_in_doc(&doc, "a.rs", raw).unwrap_err();
        assert!(err.contains("Current lines near 2"), "{raw}: {err}");
    }
}

#[test]
fn repo_map_skips_hidden_dirs() {
    let ws = tmp("map-hidden");
    std::fs::create_dir_all(ws.join(".github")).unwrap();
    std::fs::write(ws.join(".github/w.rs"), "fn hidden_one() {}\n").unwrap();
    std::fs::write(ws.join("a.rs"), "fn shown_one() {}\n").unwrap();
    let m = repo_map(&ws, None, None).unwrap();
    assert!(m.contains("shown_one") && !m.contains("hidden_one"), "{m}");
}
