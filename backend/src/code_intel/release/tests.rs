use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vireo-rel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(ws: &Path, rel: &str, text: &str) {
    let p = ws.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn sh(ws: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(ws).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn init(ws: &Path) {
    sh(ws, &["init", "-q", "-b", "main"]);
    sh(ws, &["config", "user.email", "t@example.com"]);
    sh(ws, &["config", "user.name", "Tester"]);
    sh(ws, &["config", "commit.gpgsign", "false"]);
    sh(ws, &["config", "core.autocrlf", "false"]);
}

fn v(s: &str) -> Ver {
    Ver::parse(s).unwrap()
}

#[test]
fn semver_parse_order_and_resolve() {
    assert_eq!(v("v1.2.3-rc.1").to_string(), "1.2.3-rc.1");
    assert!(Ver::parse("1.2").is_none() && Ver::parse("1.2.x").is_none());
    assert!(v("1.0.0-rc.2") < v("1.0.0-rc.10") && v("1.0.0-rc.1") < v("1.0.0") && v("0.9.9") < v("1.0.0-alpha"));
    let b = v("0.4.0");
    assert_eq!(b.resolve("patch").unwrap().to_string(), "0.4.1");
    assert_eq!(b.resolve("minor").unwrap().to_string(), "0.5.0");
    assert_eq!(b.resolve("major").unwrap().to_string(), "1.0.0");
    assert_eq!(b.resolve("prerelease").unwrap().to_string(), "0.4.1-rc.1");
    assert_eq!(v("0.5.0-rc.1").resolve("prerelease").unwrap().to_string(), "0.5.0-rc.2");
    assert_eq!(v("0.5.0-rc.2").resolve("minor").unwrap().to_string(), "0.5.0");
    assert_eq!(v("0.5.0-rc.2").resolve("patch").unwrap().to_string(), "0.5.0");
    assert_eq!(b.resolve("2.0.0-beta").unwrap().to_string(), "2.0.0-beta");
    assert!(b.resolve("sideways").is_err());
}

#[test]
fn bumps_every_stack_losslessly() {
    let ws = tmp("stacks");
    // CRLF must survive; only the version bytes change.
    write(&ws, "rs/Cargo.toml", "[package]\r\nname = \"app\"\r\nversion = \"1.0.0\"\r\n\r\n[dependencies]\r\nserde = { version = \"1\" }\r\n");
    write(&ws, "rs/Cargo.lock", "[[package]]\nname = \"app\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n");
    write(&ws, "web/package.json", "{\n  \"name\": \"w\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": { \"version\": \"9.9.9\" }\n}\n");
    write(&ws, "web/package-lock.json", "{\n  \"name\": \"w\",\n  \"version\": \"1.0.0\",\n  \"packages\": {\n    \"\": { \"name\": \"w\", \"version\": \"1.0.0\" },\n    \"node_modules/x\": { \"version\": \"3.0.0\" }\n  }\n}\n");
    write(&ws, "py/pyproject.toml", "[project]\nname = \"p\"\nversion = \"1.0.0\"\n");
    write(&ws, "py/pkg/__init__.py", "__version__ = \"1.0.0\"\n");
    write(&ws, "dn/Directory.Build.props", "<Project>\n  <PropertyGroup>\n    <!-- <Version>0.0.1</Version> -->\n    <Version>1.0.0</Version>\n    <AssemblyVersion>1.0.0.0</AssemblyVersion>\n    <FileVersion>1.0.0.0</FileVersion>\n    <InformationalVersion>$(Version)</InformationalVersion>\n  </PropertyGroup>\n</Project>\n");
    write(&ws, "dn/Lib/Lib.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><PackageReference Include=\"MinVer\" /></PropertyGroup></Project>");
    write(&ws, "jv/pom.xml", "<project>\n  <parent><groupId>g</groupId><version>7.0.0</version></parent>\n  <artifactId>a</artifactId>\n  <version>1.0.0</version>\n  <dependencies><dependency><version>2.0.0</version></dependency></dependencies>\n</project>\n");
    write(&ws, "jv/gradle.properties", "org.x=1\nversion=1.0.0\n");
    write(&ws, "go/version.go", "package v\n\nconst Version = \"v1.0.0\"\n");
    write(&ws, "inst/App.iss", "#ifndef MyAppVersion\n  #define MyAppVersion \"0.0.0\"\n#endif\n");
    write(&ws, "app/tauri.conf.json", "{ \"productName\": \"x\", \"version\": \"1.0.0\" }");
    write(&ws, "node_modules/p/package.json", "{ \"version\": \"5.5.5\" }");

    let m = manifest::scan(&ws, &Config::default());
    let paths: Vec<&str> = m.iter().map(|x| x.path.as_str()).collect();
    assert!(!paths.iter().any(|p| p.contains("node_modules") || p.ends_with(".iss")), "{paths:?}");
    assert!(m.iter().any(|x| x.path == "dn/Lib/Lib.csproj" && x.note.as_deref().is_some_and(|n| n.contains("tag-driven"))));

    let changes = manifest::plan(&m, &[], &v("1.1.0-rc.1"));
    let get = |p: &str| changes.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("no change for {p}: {:?}", changes.iter().map(|c| &c.path).collect::<Vec<_>>())).new.clone();

    assert_eq!(get("rs/Cargo.toml"), "[package]\r\nname = \"app\"\r\nversion = \"1.1.0-rc.1\"\r\n\r\n[dependencies]\r\nserde = { version = \"1\" }\r\n");
    let lock = get("rs/Cargo.lock");
    assert!(lock.contains("name = \"app\"\nversion = \"1.1.0-rc.1\"") && lock.contains("name = \"serde\"\nversion = \"1.0.0\""), "{lock}");
    assert!(get("web/package.json").contains("\"version\": \"1.1.0-rc.1\",") && get("web/package.json").contains("\"version\": \"9.9.9\""));
    let pl = get("web/package-lock.json");
    assert_eq!(pl.matches("1.1.0-rc.1").count(), 2);
    assert!(pl.contains("\"node_modules/x\": { \"version\": \"3.0.0\" }"));
    assert!(get("py/pyproject.toml").contains("version = \"1.1.0-rc.1\""));
    assert!(get("py/pkg/__init__.py").contains("\"1.1.0-rc.1\""));
    let props = get("dn/Directory.Build.props");
    assert!(props.contains("<!-- <Version>0.0.1</Version> -->"), "comment untouched");
    assert!(props.contains("<Version>1.1.0-rc.1</Version>") && props.contains("<AssemblyVersion>1.1.0.0</AssemblyVersion>"));
    assert!(props.contains("<InformationalVersion>$(Version)</InformationalVersion>"));
    let pom = get("jv/pom.xml");
    assert!(pom.contains("<artifactId>a</artifactId>\n  <version>1.1.0-rc.1</version>") && pom.contains("<version>7.0.0</version>") && pom.contains("<version>2.0.0</version>"), "{pom}");
    assert!(get("jv/gradle.properties").contains("version=1.1.0-rc.1"));
    assert!(get("go/version.go").contains("\"v1.1.0-rc.1\""));
    assert!(get("app/tauri.conf.json").contains("\"version\": \"1.1.0-rc.1\""));

    // `only` limits the bump; a lockfile follows only its own manifest.
    let only = manifest::plan(&m, &["rs/Cargo.toml".to_string()], &v("1.1.0"));
    let names: Vec<&str> = only.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(names, ["rs/Cargo.toml", "rs/Cargo.lock"]);

    // Writing then rescanning is stable and idempotent.
    manifest::write_all(&ws, &changes).unwrap();
    let again = manifest::scan(&ws, &Config::default());
    assert!(manifest::plan(&again, &[], &v("1.1.0-rc.1")).is_empty());
    assert_eq!(Ctx::load(&ws).unwrap().base().to_string(), "1.1.0-rc.1");
}

#[test]
fn custom_files_and_excludes_come_from_config() {
    let ws = tmp("custom");
    write(&ws, "CMakeLists.txt", "project(Foo VERSION 2.3.4 LANGUAGES CXX)\n");
    write(&ws, "vendor/package.json", "{ \"version\": \"9.0.0\" }");
    write(&ws, ".vireo/release.json", r#"{ "files": [{ "path": "CMakeLists.txt", "pattern": "VERSION ([0-9.]+)" }], "exclude": ["vendor"] }"#);
    let ctx = Ctx::load(&ws).unwrap();
    assert_eq!(ctx.manifests.len(), 1, "vendor excluded");
    let c = manifest::plan(&ctx.manifests, &[], &v("2.4.0"));
    assert_eq!(c[0].new, "project(Foo VERSION 2.4.0 LANGUAGES CXX)\n");
    write(&ws, ".vireo/release.json", "{ nope");
    assert!(Ctx::load(&ws).is_err());
}

#[test]
fn conventional_commits_drive_suggestion_and_notes() {
    let ws = tmp("notes");
    init(&ws);
    write(&ws, "a.txt", "1");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "chore: init"]);
    sh(&ws, &["tag", "-a", "v0.1.0", "-m", "v0.1.0"]);
    for (i, msg) in ["feat(ui): add pane (#12)", "fix: crash on save", "refactor!: drop old api", "docs: tweak", "chore(release): v0.1.0", "random words"].iter().enumerate() {
        write(&ws, "a.txt", &i.to_string());
        sh(&ws, &["commit", "-qam", msg]);
    }
    let tags = notes::tags(&ws);
    assert_eq!(tags[0].name, "v0.1.0");
    let cs = notes::commits(&ws, Some("v0.1.0"));
    assert_eq!(cs.len(), 5, "release bookkeeping commit dropped");
    assert!(cs.iter().any(|c| c.kind == "feat" && c.scope.as_deref() == Some("ui") && c.pr == Some(12) && c.subject == "add pane"));
    assert!(cs.iter().any(|c| c.breaking && c.kind == "refactor"));
    // Pre-1.0 a breaking change bumps the minor; after 1.0 it bumps the major.
    assert_eq!(notes::suggest(Some(&v("0.1.0")), &cs).version.as_deref(), Some("0.2.0"));
    assert_eq!(notes::suggest(Some(&v("1.4.0")), &cs).version.as_deref(), Some("2.0.0"));
    assert_eq!(notes::suggest(Some(&v("1.4.0")), &[]).bump, "none");
    let md = notes::build_notes(&ws, "v0.2.0", tags.first(), &cs).markdown;
    assert!(md.starts_with("## v0.2.0\n") && md.contains("### Breaking Changes\n\n- Drop old api"), "{md}");
    assert!(md.contains("### Features\n\n- **ui:** Add pane (#12)") && md.contains("### Bug Fixes") && md.contains("### Documentation"), "{md}");
    assert!(md.contains("### Other Changes\n\n- Random words"), "{md}");
}

#[test]
fn changelog_is_prepended_once_and_keeps_crlf() {
    let ws = tmp("changelog");
    write(&ws, "CHANGELOG.md", "# Changelog\r\n\r\n## [Unreleased]\r\n\r\n## v0.1.0\r\n\r\n- first\r\n");
    prepend_changelog(&ws, "## v0.2.0\n\n- second\n", "v0.2.0", &v("0.2.0")).unwrap();
    let t = std::fs::read_to_string(ws.join("CHANGELOG.md")).unwrap();
    assert_eq!(t, "# Changelog\r\n\r\n## [Unreleased]\r\n\r\n## v0.2.0\r\n\r\n- second\r\n\r\n## v0.1.0\r\n\r\n- first\r\n");
    let msg = prepend_changelog(&ws, "## v0.2.0\n\n- second\n", "v0.2.0", &v("0.2.0")).unwrap();
    assert!(msg.contains("already has"), "{msg}");
}

#[test]
fn publish_commits_bumps_tags_and_refuses_unsafe_states() {
    let ws = tmp("publish");
    init(&ws);
    write(&ws, "Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
    write(&ws, ".gitignore", ".vireo/\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "chore: init"]);
    sh(&ws, &["tag", "-a", "v0.1.0", "-m", "v0.1.0"]);
    write(&ws, "src.rs", "x");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "feat: add thing"]);

    let status: Value = serde_json::from_str(&run(&ws, &json!({ "action": "status", "json": true })).unwrap()).unwrap();
    assert_eq!(status["suggested"]["version"], "0.2.0");
    assert_eq!(status["mode"], "local");

    // Dry run touches nothing.
    let dry = run(&ws, &json!({ "action": "publish", "dry_run": true, "skip_tests": true })).unwrap();
    assert!(dry.contains("DRY RUN") && dry.contains("Bump version files"), "{dry}");
    assert!(std::fs::read_to_string(ws.join("Cargo.toml")).unwrap().contains("0.1.0"));
    assert_eq!(sh(&ws, &["tag", "--list"]), "v0.1.0");

    // A dirty tree is refused and nothing changes.
    write(&ws, "src.rs", "dirty");
    let refused = run(&ws, &json!({ "action": "publish", "skip_tests": true })).unwrap();
    assert!(refused.contains("✖ Working tree clean") && refused.contains("not ready"), "{refused}");
    assert!(std::fs::read_to_string(ws.join("Cargo.toml")).unwrap().contains("0.1.0"));
    sh(&ws, &["checkout", "--", "src.rs"]);

    // Real run: bump + release commit + annotated tag, no remote so it stays local.
    let out = run(&ws, &json!({ "action": "publish", "skip_tests": true, "version": "minor" })).unwrap();
    assert!(!out.contains('✖'), "{out}");
    assert!(std::fs::read_to_string(ws.join("Cargo.toml")).unwrap().contains("version = \"0.2.0\""));
    assert_eq!(sh(&ws, &["log", "-1", "--format=%s"]), "chore(release): v0.2.0");
    assert_eq!(sh(&ws, &["cat-file", "-t", "v0.2.0"]), "tag");
    assert_eq!(sh(&ws, &["status", "--porcelain"]), "");

    // The tag now exists, so releasing it again is refused.
    let again = run(&ws, &json!({ "action": "publish", "skip_tests": true, "version": "0.2.0" })).unwrap();
    assert!(again.contains("✖ Tag available") && again.contains("✖ Version increases"), "{again}");
    assert!(run(&ws, &json!({ "action": "bump" })).unwrap_err().contains("usage"));
    assert!(run(&ws, &json!({ "action": "nope" })).unwrap_err().contains("unknown release action"));
}

#[test]
fn activity_log_tracks_source_and_collapses_running_entries() {
    let ws = tmp("activity");
    init(&ws);
    write(&ws, "package.json", "{\n  \"name\": \"a\",\n  \"version\": \"0.1.0\"\n}\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-q", "-m", "feat: first"]);

    // An agent writes notes; the UI runs checks. Both land in the log with their source.
    run(&ws, &json!({ "action": "notes", "version": "0.2.0", "write": true, "source": "agent" })).unwrap();
    run(&ws, &json!({ "action": "preflight", "version": "0.2.0", "skip_tests": true, "source": "ui" })).unwrap();
    let items: Value = serde_json::from_str(&run(&ws, &json!({ "action": "activity", "json": true })).unwrap()).unwrap();
    let items = items["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!((items[0]["action"].as_str(), items[0]["source"].as_str()), (Some("preflight"), Some("ui")));
    assert_eq!((items[1]["action"].as_str(), items[1]["source"].as_str()), (Some("notes"), Some("agent")));
    assert_eq!(items[1]["tag"], "v0.2.0");
    // The log lives in the git dir, so it never dirties the working tree.
    assert_eq!(sh(&ws, &["status", "--porcelain", "--", "package.json"]), "");
    assert!(ws.join(".git").join("vireo-release-activity.jsonl").is_file());

    // A `running` entry is replaced by the entry that finishes it.
    activity::record(&ws, 7, "agent", "publish", json!({ "phase": "running", "tag": "v0.2.0" }));
    assert_eq!(activity::read(&ws, 10)[0]["phase"], "running");
    activity::record(&ws, 7, "agent", "publish", json!({ "ok": true, "tag": "v0.2.0" }));
    let items = activity::read(&ws, 10);
    assert_eq!(items.len(), 3);
    assert_eq!((items[0]["phase"].as_str(), items[0]["ok"].as_bool()), (Some("done"), Some(true)));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn non_conventional_commits_categorize_and_include_diff_context() {
    let ws = tmp("nonconv");
    init(&ws);
    write(&ws, "Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.4.0\"\n");
    write(&ws, "README.md", "# App\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "Initial commit"]);
    sh(&ws, &["tag", "-a", "v0.4.0", "-m", "v0.4.0"]);

    let commits = [
        ("src/theme.rs", "pub fn dark() {}\n", "Add dark mode support\n\nAdds a toggle in settings.\n\nCo-Authored-By: Bot <b@x.com>"),
        ("src/path.rs", "pub fn ok() {}\n", "Fix crash on empty workspace"),
        ("src/tests.rs", "fn t() {}\n", "Add unit tests for parser"),
        ("README.md", "# App\nInstall steps\n", "Update README install steps"),
        ("src/ui.rs", "pub fn btn() {}\n", "UI: fix button alignment"),
        ("src/prev.rs", "pub fn p() {}\n", "Feature: live preview"),
        ("src/sp.rs", "pub fn s() {}\n", "[fix] handle spaces in path"),
        ("src/em.rs", "pub fn e() {}\n", "✨ Emoji feature"),
        ("src/misc.rs", "pub fn m() {}\n", "miscellaneous tweak"),
    ];
    for (file, body, msg) in commits {
        write(&ws, file, body);
        sh(&ws, &["add", "."]);
        sh(&ws, &["commit", "-qm", msg]);
    }

    let st = run(&ws, &json!({ "action": "status" })).unwrap();
    assert!(st.contains("non-conventional"), "{st}");

    let out = run(&ws, &json!({ "action": "notes", "version": "0.5.0", "write": true })).unwrap();
    assert!(out.contains("⚠ "), "{out}");
    assert!(out.contains("Change context (not written to the notes file):"), "{out}");
    assert!(out.contains("src/theme.rs"), "{out}");
    assert!(out.contains("Adds a toggle in settings."), "{out}");
    assert!(!out.contains("Co-Authored-By"), "trailers stripped: {out}");

    // Saved markdown file must remain clean (no warnings, no diffstat footer).
    let saved = std::fs::read_to_string(ws.join(".vireo/release-notes/v0.5.0.md")).unwrap();
    assert!(!saved.contains("⚠") && !saved.contains("Change context"), "{saved}");
    assert!(saved.contains("### Features\n\n") && saved.contains("Add dark mode support") && saved.contains("Live preview") && saved.contains("Emoji feature"), "{saved}");
    assert!(saved.contains("### Bug Fixes\n\n") && saved.contains("Fix crash on empty workspace") && saved.contains("UI: fix button alignment") && saved.contains("Handle spaces in path"), "{saved}");
    assert!(saved.contains("### Documentation\n\n- Update README install steps"), "{saved}");
    assert!(saved.contains("### Dependencies & Maintenance\n\n- Add unit tests for parser"), "{saved}");
    assert!(saved.contains("### Other Changes\n\n- Miscellaneous tweak"), "{saved}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn notes_for_existing_tag_scopes_to_tag_warns_and_protects_file() {
    let ws = tmp("existing-tag");
    init(&ws);
    write(&ws, "Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.4.2\"\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "chore: init"]);
    sh(&ws, &["tag", "-a", "v0.4.1", "-m", "v0.4.1"]);

    write(&ws, "a.rs", "pub fn a() {}\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "fix: bug shipped in 0.4.2"]);
    sh(&ws, &["tag", "-a", "v0.4.2", "-m", "v0.4.2"]);

    // Two newer, unreleased commits after v0.4.2.
    write(&ws, "b.rs", "pub fn b() {}\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "feat: unreleased feature one"]);
    write(&ws, "c.rs", "pub fn c() {}\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "fix: unreleased fix two"]);

    let text = run(&ws, &json!({ "action": "notes", "version": "0.4.2" })).unwrap();
    assert!(text.contains("⚠ tag `v0.4.2` already exists"), "{text}");
    assert!(text.contains("2 unreleased commit(s)"), "{text}");
    assert!(text.contains("Bug shipped in 0.4.2"), "{text}");
    assert!(!text.contains("Unreleased feature one") && !text.contains("Unreleased fix two"), "{text}");

    let j: Value = serde_json::from_str(&run(&ws, &json!({ "action": "notes", "version": "0.4.2", "json": true })).unwrap()).unwrap();
    assert_eq!(j["tag_exists"], true);
    assert_eq!(j["range"], "v0.4.1..v0.4.2");
    assert_eq!(j["commits"], 1);

    // Writing once backfills cleanly, but a second write refuses to overwrite released notes.
    write(&ws, ".github/release-notes/v0.4.2.md", "## v0.4.2\n\nHand-written notes.\n");
    let err = run(&ws, &json!({ "action": "notes", "version": "0.4.2", "write": true })).unwrap_err();
    assert!(err.contains("already exists") && err.contains("refusing to overwrite"), "{err}");
    assert!(std::fs::read_to_string(ws.join(".github/release-notes/v0.4.2.md")).unwrap().contains("Hand-written notes."));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn ci_workflow_inspection_flags_notes_gap_and_validates_tags() {
    let ws = tmp("ci-gap");
    init(&ws);
    write(&ws, "Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
    // A non-release CI workflow with `tags:` under workflow_dispatch must NOT be misclassified.
    write(
        &ws,
        ".github/workflows/ci.yml",
        "name: CI\non:\n  push:\n    branches: [main]\n  workflow_dispatch:\n    inputs:\n      tags:\n        type: string\njobs:\n  test:\n    runs-on: ubuntu-latest\n",
    );
    // A tag-triggered release workflow that writes a temp notes file + --generate-notes,
    // without reading any repo release-notes file.
    write(
        &ws,
        ".github/workflows/release.yml",
        "name: Release\non:\n  push:\n    tags:\n      - \"v[0-9]+.[0-9]+.[0-9]+*\"\njobs:\n  release:\n    steps:\n      - run: |\n          Set-Content \"$env:RUNNER_TEMP/release-notes.md\" \"checksums\"\n          gh release create $env:TAG --notes-file \"$env:RUNNER_TEMP/release-notes.md\" --generate-notes\n",
    );
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "feat: initial"]);

    let st_json: Value = serde_json::from_str(&run(&ws, &json!({ "action": "status", "json": true })).unwrap()).unwrap();
    assert_eq!(st_json["ci_workflows"], json!(["release.yml"]));
    assert_eq!(st_json["ci_reads_notes"], false);
    assert!(st_json["ci_notes_warning"].as_str().unwrap().contains("does not appear to read"));

    let st_text = run(&ws, &json!({ "action": "status" })).unwrap();
    assert!(st_text.contains("⚠ CI workflow (release.yml) does not appear to read"), "{st_text}");

    let pre = run(&ws, &json!({ "action": "preflight", "version": "0.1.0", "skip_tests": true })).unwrap();
    assert!(pre.contains("⚠ CI release notes"), "{pre}");

    let dry = run(&ws, &json!({ "action": "publish", "version": "0.1.0", "dry_run": true, "skip_tests": true })).unwrap();
    assert!(dry.contains("⚠ Release notes") && dry.contains("not read by release.yml"), "{dry}");

    // Now update the workflow to read `.github/release-notes/<tag>.md` (before the dir even exists).
    write(
        &ws,
        ".github/workflows/release.yml",
        "name: Release\non:\n  push:\n    tags:\n      - \"v[0-9]+.[0-9]+.[0-9]+*\"\njobs:\n  release:\n    steps:\n      - run: |\n          $notesFile = \".github/release-notes/$env:TAG.md\"\n          gh release create $env:TAG --notes-file \"$env:RUNNER_TEMP/release-notes.md\"\n",
    );
    let ctx = Ctx::load(&ws).unwrap();
    assert_eq!(ctx.notes_path("v0.1.0"), ".github/release-notes/v0.1.0.md");

    let st2: Value = serde_json::from_str(&run(&ws, &json!({ "action": "status", "json": true })).unwrap()).unwrap();
    assert_eq!(st2["ci_reads_notes"], true);
    assert!(st2["ci_notes_warning"].is_null(), "{st2}");

    let pre2 = run(&ws, &json!({ "action": "preflight", "version": "0.1.0", "skip_tests": true })).unwrap();
    assert!(pre2.contains("✔ CI release notes — `release.yml` reads `.github/release-notes/<tag>.md`"), "{pre2}");

    // Tag pattern mismatch is flagged if tag_prefix doesn't match `v*`.
    assert!(ci::tag_matches_patterns("v0.7.0", &["v[0-9]+.[0-9]+.[0-9]+*".to_string()]));
    assert!(ci::tag_matches_patterns("v0.7.0-rc.1", &["v[0-9]+.[0-9]+.[0-9]+*".to_string()]));
    assert!(!ci::tag_matches_patterns("0.7.0", &["v[0-9]+.[0-9]+.[0-9]+*".to_string()]));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn cargo_workspace_lockfile_sync_and_precommitted_as_is_publish() {
    let ws = tmp("ws-cargo");
    init(&ws);
    write(&ws, "Cargo.toml", "[workspace]\nmembers = [\"crates/foo\"]\n\n[workspace.package]\nversion = \"0.1.0\"\n");
    write(&ws, "crates/foo/Cargo.toml", "[package]\nname = \"foo\"\nversion.workspace = true\n");
    write(&ws, "Cargo.lock", "[[package]]\nname = \"foo\"\nversion = \"0.1.0\"\n");
    let m = manifest::scan(&ws, &Config::default());
    let changes = manifest::plan(&m, &[], &v("0.2.0"));
    assert!(changes.iter().any(|c| c.path == "Cargo.toml" && c.new.contains("version = \"0.2.0\"")));
    assert!(changes.iter().any(|c| c.path == "Cargo.lock" && c.new.contains("name = \"foo\"\nversion = \"0.2.0\"")));

    // Pre-bumped version + pre-committed notes file in `.github/release-notes/` must tag cleanly
    // instead of failing on an empty `git commit`.
    manifest::write_all(&ws, &changes).unwrap();
    write(&ws, ".github/release-notes/v0.2.0.md", "## v0.2.0\n\n- Ready\n");
    sh(&ws, &["add", "."]);
    sh(&ws, &["commit", "-qm", "chore: prepare v0.2.0"]);
    let out = run(&ws, &json!({ "action": "publish", "version": "0.2.0", "skip_tests": true })).unwrap();
    assert!(!out.contains('✖'), "{out}");
    assert_eq!(sh(&ws, &["tag", "--list"]), "v0.2.0");
    let _ = std::fs::remove_dir_all(&ws);
}
