use super::check::{compress_plain, parse_diags, render_diags, test_summary};
use super::search::builtin_search;
use super::symbols::find_symbol;
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
    let r = hashline_edit(&ws, "a.rs", "replace", &l5, Some(&l7), "fn foo(x: i32) -> i32 {\n    x + 2\n}").unwrap();
    assert!(r.contains("lines 8..10"), "{r}");
    assert!(std::fs::read_to_string(ws.join("a.rs")).unwrap().contains("x + 2"));
    let anchor = format!("8#{}", line_hash("fn foo(x: i32) -> i32 {"));
    let r = hashline_edit(&ws, "a.rs", "replace", &anchor, None, "fn foo(x: i32) -> i32 {{{").unwrap();
    assert!(r.contains("syntax error"), "{r}");
}

#[test]
fn stale_and_malformed_anchors_show_live_snippets() {
    let ws = tmp("stale");
    let lines: String = (1..=30).map(|i| format!("let v{i} = {i};\n")).collect();
    std::fs::write(ws.join("a.rs"), &lines).unwrap();
    // Stale single anchor
    let err = hashline_edit(&ws, "a.rs", "replace", "5#ff", None, "x").unwrap_err();
    assert!(err.contains("no longer match") && err.contains("let v5 = 5;"), "{err}");
    // Malformed hex anchor shows real lines around line 12
    let err = hashline_edit(&ws, "a.rs", "replace", "12#zz", None, "x").unwrap_err();
    assert!(err.contains("bad anchor") && err.contains("let v12 = 12;"), "{err}");
    // Distant end anchor typo shows lines near line 25 (not clipped to start!)
    let s1 = format!("1#{}", line_hash("let v1 = 1;"));
    let err = hashline_edit(&ws, "a.rs", "replace", &s1, Some("25#00"), "x").unwrap_err();
    assert!(err.contains("end anchor") && err.contains("let v25 = 25;"), "{err}");
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), lines);
}

#[test]
fn crlf_is_preserved_and_prefixes_stripped() {
    let ws = tmp("crlf");
    std::fs::write(ws.join("a.ts"), "const a = 1;\r\nconst b = 2;\r\n").unwrap();
    let anchor = format!("1#{}", line_hash("const a = 1;"));
    hashline_edit(
        &ws,
        "a.ts",
        "replace",
        &anchor,
        None,
        &format!("1#{}|const a = 9;\nconst c = 3;", line_hash("x")),
    )
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
    let r = symbol_op(&ws, "a.rs", "Bar::baz", "replace", "    fn baz(&self) -> bool {\n        false\n    }").unwrap();
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
    let all = edit::ast(&ws, "foo($A, $B)", None, None, None, false).unwrap();
    assert!(all.starts_with("4 match(es)"), "{all}");
    assert!(all.contains("src/a.rs:2#") && all.contains("src/a.rs:3#"), "{all}");
    assert!(all.contains("src/c.py:2#") && all.contains("src/d.ts:1#"), "{all}");
    assert!(!all.contains("gen/b.rs"), "{all}");

    let py = edit::ast(&ws, "foo($$$ARGS)", Some("python"), None, None, false).unwrap();
    assert!(py.starts_with("1 match(es)") && py.contains("src/c.py"), "{py}");

    let dry = edit::ast(&ws, "foo($A, $B)", Some("rust"), Some("src"), Some("baz($B, $A)"), true).unwrap();
    assert!(dry.contains("dry run"), "{dry}");
    assert!(std::fs::read_to_string(ws.join("src/a.rs")).unwrap().contains("foo(1, 2)"));

    let done = edit::ast(&ws, "foo($A, $B)", Some("rs"), None, Some("baz($B, $A)"), false).unwrap();
    assert!(done.starts_with("Rewrote 2 match(es) in 1 file(s)"), "{done}");
    assert_eq!(
        std::fs::read_to_string(ws.join("src/a.rs")).unwrap(),
        "fn f() {\n    let v = baz(2, 1);\n    baz(bar(4), 3);\n}\n"
    );
    assert_eq!(std::fs::read_to_string(ws.join("gen/b.rs")).unwrap(), "fn g() { foo(9, 9); }\n");
    assert_eq!(edit::ast(&ws, "nothing_here($A)", Some("go"), None, None, false).unwrap(), "No matches.");
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
    let ok = check::check(&ws, Some("exit 0"), 30, opts).unwrap();
    assert!(ok.contains("ok (") && ok.contains("exit 0, no output"), "{ok}");
    let chatty = check::check(&ws, Some("echo hello"), 30, opts).unwrap();
    assert!(chatty.contains("1 output line(s), last: hello"), "{chatty}");
    let env = check::check(&ws, Some("echo cl.exe not found && exit 1"), 30, opts).unwrap();
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
    let out = hashline_edit(&ws, "a.txt", "replace", &s, Some(&e), "S\nE").unwrap();
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
    let out = ast(&ws, "old($X)", Some("rust"), None, Some("new($X);\nnew2($X)"), false).unwrap();
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
