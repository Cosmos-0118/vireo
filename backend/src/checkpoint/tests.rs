use super::*;

fn sh(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn setup(files: &[(&str, &str)]) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("vireo-cp-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    sh(&dir, &["init", "-q"]);
    sh(&dir, &["config", "core.autocrlf", "false"]);
    for (p, c) in files {
        std::fs::write(dir.join(p), c).unwrap();
    }
    let (tx, _rx) = mpsc::channel(1);
    (AppState::new(dir.clone(), tx), dir)
}

fn read(dir: &Path, p: &str) -> String {
    std::fs::read_to_string(dir.join(p)).unwrap()
}

fn write(dir: &Path, p: &str, c: &str) {
    std::fs::write(dir.join(p), c).unwrap();
}

async fn undo(state: &AppState, id: &str, force: bool) -> RollbackRes {
    rollback_checkpoint(State(state.clone()), Json(RollbackReq { id: id.into(), force }))
        .await
        .unwrap()
        .0
}

async fn redo(state: &AppState, id: &str) -> RollbackRes {
    redo_checkpoint(State(state.clone()), Json(RollbackReq { id: id.into(), force: false }))
        .await
        .unwrap()
        .0
}

fn lines(range: std::ops::RangeInclusive<u32>) -> String {
    range.map(|i| format!("line{i}\n")).collect()
}

async fn turn(state: &AppState, dir: &Path, edits: &[(&str, &str)]) -> Option<Checkpoint> {
    let mut t = TurnTracker::new(true);
    t.begin(state).await;
    for (p, c) in edits {
        t.note_tool("edit_file", &serde_json::json!({ "path": p }));
        write(dir, p, c);
    }
    t.finish(state, None).await
}

#[tokio::test]
async fn turn_without_edits_makes_no_checkpoint() {
    let (state, dir) = setup(&[("a.txt", "x\n")]);
    assert!(turn(&state, &dir, &[]).await.is_none());
    assert!(state.checkpoints.read().await.is_empty());
}

#[tokio::test]
async fn undo_then_redo_round_trips() {
    let (state, dir) = setup(&[("a.txt", "one\n"), ("keep.txt", "k\n")]);
    let cp = turn(&state, &dir, &[("a.txt", "two\n"), ("new.txt", "n\n")]).await.unwrap();
    assert_eq!(cp.files.len(), 2);

    let r = undo(&state, &cp.id, false).await;
    assert!(r.ok && r.restored == 2, "{:?}", r.stderr);
    assert_eq!(read(&dir, "a.txt"), "one\n");
    assert!(!dir.join("new.txt").exists());

    let r = redo(&state, &cp.id).await;
    assert!(r.ok && r.restored == 2);
    assert_eq!(read(&dir, "a.txt"), "two\n");
    assert_eq!(read(&dir, "new.txt"), "n\n");
    assert_eq!(read(&dir, "keep.txt"), "k\n");
}

#[tokio::test]
async fn other_writers_edit_in_another_hunk_survives() {
    let (state, dir) = setup(&[("a.txt", &lines(1..=10))]);
    let cp = turn(&state, &dir, &[("a.txt", &lines(1..=10).replacen("line1\n", "agent\n", 1))])
        .await
        .unwrap();
    // Someone else edits the far end of the same file afterwards.
    write(&dir, "a.txt", &read(&dir, "a.txt").replace("line10\n", "other\n"));

    let r = undo(&state, &cp.id, false).await;
    assert!(r.ok, "{:?}", r.conflicts);
    assert_eq!(read(&dir, "a.txt"), lines(1..=10).replace("line10\n", "other\n"));
}

#[tokio::test]
async fn overlapping_edit_conflicts_and_force_is_redoable() {
    let (state, dir) = setup(&[("a.txt", &lines(1..=5))]);
    let cp = turn(&state, &dir, &[("a.txt", &lines(1..=5).replace("line3\n", "agent\n"))])
        .await
        .unwrap();
    let theirs = lines(1..=5).replace("line3\n", "other\n");
    write(&dir, "a.txt", &theirs);

    let r = undo(&state, &cp.id, false).await;
    assert!(!r.ok);
    assert_eq!(r.conflicts, vec!["a.txt".to_string()]);
    assert_eq!(read(&dir, "a.txt"), theirs, "conflicted file must be untouched");

    let r = undo(&state, &cp.id, true).await;
    assert!(r.ok);
    assert_eq!(read(&dir, "a.txt"), lines(1..=5));

    // The overwritten content is recoverable.
    redo(&state, &cp.id).await;
    assert_eq!(read(&dir, "a.txt"), theirs);
}

#[tokio::test]
async fn files_outside_the_turn_are_never_touched() {
    let (state, dir) = setup(&[("a.txt", "a\n"), ("bg.txt", "bg\n")]);
    let mut t = TurnTracker::new(true);
    t.begin(&state).await;
    t.note_tool("edit_file", &serde_json::json!({ "path": "a.txt" }));
    write(&dir, "a.txt", "a2\n");
    write(&dir, "bg.txt", "bg-from-background-agent\n");
    let cp = t.finish(&state, None).await.unwrap();
    assert_eq!(cp.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);

    undo(&state, &cp.id, false).await;
    assert_eq!(read(&dir, "a.txt"), "a\n");
    assert_eq!(read(&dir, "bg.txt"), "bg-from-background-agent\n");
}

#[test]
fn read_only_commands_are_not_opaque() {
    for c in [
        "rg -il advisor src",
        "ls -la",
        "git status --short",
        "git diff HEAD",
        "vx view a.rs",
        "rg -o foo src",
        "Get-ChildItem -Recurse",
    ] {
        assert!(is_read_only_command(c), "{c}");
    }
    for c in [
        "rg -l x | xargs rm",
        "cat a > b",
        "git checkout .",
        "git diff --output=x",
        "find . -delete",
        "find . -exec rm {} +",
        "npm install",
        "ls; rm a",
        "echo $(rm a)",
        "",
        "vx edit a.rs 1#a",
    ] {
        assert!(!is_read_only_command(c), "{c}");
    }
}

#[test]
fn read_only_run_command_does_not_make_turn_opaque() {
    let mut t = TurnTracker::new(true);
    t.note_tool("run_command", &serde_json::json!({ "command": "rg foo" }));
    assert!(!t.opaque);
    t.note_tool("run_command", &serde_json::json!({ "command": "cargo build" }));
    assert!(t.opaque);
}

#[tokio::test]
async fn manual_restore_keeps_a_way_back() {
    let (state, dir) = setup(&[("a.txt", "v1\n")]);
    let snap = create_checkpoint_internal(&state, "snap".into()).await.unwrap();
    write(&dir, "a.txt", "v2\n");
    write(&dir, "extra.txt", "x\n");

    let r = undo(&state, &snap.id, false).await;
    assert!(r.ok);
    assert_eq!(read(&dir, "a.txt"), "v1\n");
    assert!(!dir.join("extra.txt").exists());

    let back = state.checkpoints.read().await.iter().find(|c| c.label == "Before restore").cloned().unwrap();
    undo(&state, &back.id, false).await;
    assert_eq!(read(&dir, "a.txt"), "v2\n");
    assert_eq!(read(&dir, "extra.txt"), "x\n");
}

#[tokio::test]
async fn crlf_files_round_trip_with_autocrlf() {
    let (state, dir) = setup(&[("a.txt", "a\r\nb\r\n")]);
    sh(&dir, &["config", "core.autocrlf", "true"]);
    let cp = turn(&state, &dir, &[("a.txt", "a\r\nc\r\n")]).await.unwrap();
    let r = undo(&state, &cp.id, false).await;
    assert!(r.ok && r.restored == 1, "{:?}", r.stderr);
    assert_eq!(read(&dir, "a.txt"), "a\r\nb\r\n");
    redo(&state, &cp.id).await;
    assert_eq!(read(&dir, "a.txt"), "a\r\nc\r\n");
}
