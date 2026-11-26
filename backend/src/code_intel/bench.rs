//! Ignored micro-benchmarks: `cargo test --release bench_ -- --ignored --nocapture`.
use super::*;
use std::time::Instant;

fn best_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn tmpdir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vireo-bench-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
#[ignore]
fn bench_closest_hint() {
    let text: String = (0..5000).map(|i| format!("    let value_{i} = compute({i}) + other_{i};\n")).collect();
    let doc = doc::Doc::parse(&text);
    let old: Vec<String> = (0..100).map(|i| format!("    let missing_{i} = nothing({i});")).collect();
    let ms = best_ms(3, || {
        std::hint::black_box(patch::closest_hint(&doc, &old));
    });
    println!("BENCH closest_hint(5000 lines, 100-line hunk) {ms:.2} ms");
}

#[test]
#[ignore]
fn bench_replace_scan() {
    let ws = tmpdir("replace");
    let body: String = (0..800).map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n")).collect();
    for d in 0..30 {
        std::fs::create_dir_all(ws.join(format!("d{d}"))).unwrap();
        for f in 0..60 {
            std::fs::write(ws.join(format!("d{d}/f{f}.txt")), &body).unwrap();
        }
    }
    let opts = replace::ReplaceOpts { dry_run: true, all_files: true, ..Default::default() };
    let ms = best_ms(5, || {
        let _ = std::hint::black_box(replace::replace_text(&ws, "NOT_PRESENT_ANYWHERE", "x", &opts));
    });
    println!("BENCH replace_text(1800 files x 40KB, 0 hits) {ms:.2} ms");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
#[ignore]
fn bench_ast_rewrite() {
    let ws = tmpdir("ast");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    let mut src = String::new();
    for i in 0..30000 {
        src.push_str(&format!("fn f{i}() {{ foo({i}, {i}); }}\n"));
    }
    std::fs::write(ws.join("src/big.rs"), &src).unwrap();
    let ms = best_ms(3, || {
        let _ = std::hint::black_box(edit::ast(&ws, "foo($A, $B)", Some("rust"), Some("src"), Some("bar($B, $A)"), true, None));
    });
    println!("BENCH ast rewrite(30000 edits, 1.1MB file, dry-run) {ms:.2} ms");
    let _ = std::fs::remove_dir_all(&ws);
}
