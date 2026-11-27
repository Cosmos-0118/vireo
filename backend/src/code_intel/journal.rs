//! Per-file edit journal: lets an anchor taken before earlier edits still find its line.
//!
//! An anchor is `N#hh`: a line number plus a 1-byte hash. After lines shift, hash-only relocation
//! has to pick among every line in a ±50 window that shares the hash (about one in three picks is
//! ambiguous, and `}` or blank lines are hopeless). The process that made the edits knows exactly
//! how rows moved, so it keeps session state per file, the approach tools like zerostack take:
//!
//! - every write is recorded as row splices, and a stale anchor is mapped forward through them and
//!   verified against its hash (candidates are a handful of mapped rows instead of a window);
//! - a write this process did not make (editor, git, `ast`) is found by comparing the file with the
//!   last text seen, diffed on line hashes, and becomes one more step instead of discarding history;
//! - every anchor printed to the caller is noted (per version of the file), so when two versions
//!   both explain an anchor, the one whose row the caller was actually shown wins.
//!
//! Nothing here can pick a wrong line silently: a candidate must match the anchor's hash(es), and
//! several distinct candidates that cannot be told apart are an error that lists them.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use xxhash_rust::xxh32::xxh32;

use super::doc::{line_hash, locate, Doc};

/// `(start row, rows removed, rows added)` in the coordinates of the text before the write.
pub(crate) type Splice = (usize, usize, usize);

const MAX_STEPS: usize = 48;
const MAX_FILES: usize = 256;
/// Files bigger than this are not journaled (snapshots are kept per file).
const MAX_ROWS: usize = 200_000;
/// How far apart (in rows) the two texts may drift before a diff gives up re-syncing.
const RESYNC_WINDOW: usize = 300;

#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    path: PathBuf,
    /// Fingerprint of the text the snapshot below describes.
    fp: u64,
    /// `(len, mtime)` of the file when the snapshot was taken: a cheap "did it change" check.
    meta: Option<(u64, Option<SystemTime>)>,
    /// Full hash per row of the last known text (for diffing foreign writes).
    last: Vec<u32>,
    /// Anchor hash per row of the last known text (for noting what was displayed).
    anchor_hashes: Vec<u8>,
    /// One entry per write, oldest first; entry `i` maps version `i` rows to version `i + 1`.
    steps: Vec<Vec<Splice>>,
    /// `shown[v]`: rows (version `v` coordinates) printed to the caller while the file was at
    /// version `v`.
    shown: Vec<HashSet<usize>>,
    /// The journal began when the file was first displayed, so `shown[0]` is real evidence. When it
    /// began at a write instead, version 0 may have been shown any time before: count it as shown.
    seen_from_start: bool,
}

fn table() -> &'static Mutex<HashMap<String, Journal>> {
    static T: OnceLock<Mutex<HashMap<String, Journal>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(full: &Path) -> String {
    full.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn fingerprint(text: &str) -> u64 {
    ((text.len() as u64) << 32) | xxh32(text.as_bytes(), 0) as u64
}

fn row_hashes(lines: &[String]) -> Vec<u32> {
    lines.iter().map(|l| xxh32(l.as_bytes(), 0)).collect()
}

fn file_meta(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

impl Journal {
    fn new(path: &Path) -> Journal {
        Journal {
            path: path.to_path_buf(),
            fp: 0,
            meta: None,
            last: Vec::new(),
            anchor_hashes: Vec::new(),
            steps: Vec::new(),
            shown: vec![HashSet::new()],
            seen_from_start: false,
        }
    }

    fn push_step(&mut self, sp: Vec<Splice>) {
        if sp.is_empty() {
            return;
        }
        self.steps.push(sp);
        self.shown.push(HashSet::new());
        if self.steps.len() > MAX_STEPS {
            self.steps.remove(0);
            self.shown.remove(0);
        }
    }

    fn snapshot(&mut self, lines: &[String], fp: u64) {
        self.last = row_hashes(lines);
        self.anchor_hashes = lines.iter().map(|l| u8::from_str_radix(&line_hash(l), 16).unwrap_or(0)).collect();
        self.fp = fp;
        self.meta = file_meta(&self.path);
    }

    /// The file is `text` now. If that is not what this journal last saw, someone else wrote it:
    /// diff against the last snapshot and keep the history.
    fn sync(&mut self, text: &str) {
        let fp = fingerprint(text);
        if fp == self.fp {
            self.meta = file_meta(&self.path);
            return;
        }
        let lines = Doc::parse(text).lines;
        if lines.len() > MAX_ROWS {
            self.steps.clear();
            self.shown = vec![HashSet::new()];
        } else {
            let sp = diff_splices(&self.last, &row_hashes(&lines));
            self.push_step(sp);
        }
        self.snapshot(&lines, fp);
    }

    /// Cheap check (one stat) that a foreign write has not happened since the snapshot.
    fn freshen(&mut self) {
        if file_meta(&self.path) == self.meta {
            return;
        }
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            self.sync(&text);
        }
    }
}

/// Where row `p` ends up after one write, or `None` when that write replaced or removed it.
fn map_step(step: &[Splice], p: usize) -> Option<usize> {
    let mut delta = 0isize;
    for &(s, removed, added) in step {
        if p < s {
            break;
        }
        if p < s + removed {
            return None;
        }
        delta += added as isize - removed as isize;
    }
    Some((p as isize + delta) as usize)
}

/// Change list between two versions of a file (rows, or row hashes) as ascending splices. Exact for
/// one contiguous change; several far-apart changes are found by re-syncing on matching rows.
pub(crate) fn diff_splices<T: PartialEq>(old: &[T], new: &[T]) -> Vec<Splice> {
    let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suf = old.len().min(new.len()) - pre;
    let suf = old.iter().rev().zip(new.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
    let (o, n) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < o.len() || j < n.len() {
        if i < o.len() && j < n.len() && o[i] == n[j] {
            i += 1;
            j += 1;
            continue;
        }
        // Nearest pair of rows (i+a, j+b) where both texts agree again (two rows in a row, or both end).
        let synced = |x: usize, y: usize| -> bool {
            if x == o.len() && y == n.len() {
                return true;
            }
            x < o.len() && y < n.len() && o[x] == n[y] && (x + 1 >= o.len() || y + 1 >= n.len() || o[x + 1] == n[y + 1])
        };
        let mut best = None;
        'find: for total in 1..=2 * RESYNC_WINDOW {
            for a in total.saturating_sub(RESYNC_WINDOW)..=total.min(RESYNC_WINDOW) {
                let b = total - a;
                if synced(i + a, j + b) {
                    best = Some((a, b));
                    break 'find;
                }
            }
        }
        match best {
            Some((a, b)) => {
                out.push((pre + i, a, b));
                i += a;
                j += b;
            }
            None => {
                out.push((pre + i, o.len() - i, n.len() - j));
                break;
            }
        }
    }
    out
}

/// Note that `full` changed from `old` to `new` (the write has happened). `splices` are exact when
/// the caller knows them (a patch); otherwise they are derived by diffing.
pub(crate) fn record(full: &Path, old: &str, new: &str, splices: Option<Vec<Splice>>) {
    if old == new {
        return;
    }
    let (old_lines, new_lines) = (Doc::parse(old).lines, Doc::parse(new).lines);
    if new_lines.len() > MAX_ROWS {
        forget(full);
        return;
    }
    let sp = splices.unwrap_or_else(|| diff_splices(&row_hashes(&old_lines), &row_hashes(&new_lines)));
    let Ok(mut t) = table().lock() else { return };
    let k = key(full);
    if t.len() >= MAX_FILES && !t.contains_key(&k) {
        t.clear();
    }
    let j = t.entry(k).or_insert_with(|| {
        let mut j = Journal::new(full);
        j.snapshot(&old_lines, fingerprint(old));
        j
    });
    // A foreign write between our last one and this edit's read.
    if j.fp != fingerprint(old) {
        j.sync(old);
    }
    j.push_step(sp);
    j.snapshot(&new_lines, fingerprint(new));
}

/// The file was deleted or moved away.
pub(crate) fn forget(full: &Path) {
    if let Ok(mut t) = table().lock() {
        t.remove(&key(full));
    }
}

// ---------------------------------------------------------------------------------------------
// Persistence across one-shot processes (the `vx` CLI)
// ---------------------------------------------------------------------------------------------
//
// The MCP server and the app are long-lived, so the in-memory table above sees every write and
// display. Each `vx` CLI command is a fresh process: without this, anchors printed by one command
// could not be mapped through the writes of the next, and a stale `N#hh` would either be refused
// as ambiguous or, worse, land on whatever line now sits at row N with the same hash.
//
// Each run loads the workspace's journal file, runs, and merges back only the entries it changed,
// so parallel runs on different files keep each other's history. A lost update (two runs on the
// same file at once) only drops evidence: lookups still verify every candidate's hash and fall
// back to `locate` or an error, never a silent guess.

const PERSIST_VERSION: u32 = 1;
/// Entries nobody touched for this long are dropped on load.
const PERSIST_TTL_SECS: u64 = 24 * 3600;
/// Bigger files are kept in memory only (one row hash each would bloat the file).
const PERSIST_MAX_ROWS: usize = 50_000;

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    version: u32,
    /// key -> (unix seconds when last saved, journal)
    entries: HashMap<String, (u64, Journal)>,
}

/// Per journal file: digest of each entry as loaded, to tell which ones this process changed.
fn baselines() -> &'static Mutex<HashMap<PathBuf, HashMap<String, u64>>> {
    static B: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, u64>>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn digest(j: &Journal) -> u64 {
    serde_json::to_vec(j).map_or(0, |b| ((b.len() as u64) << 32) | xxh32(&b, 0) as u64)
}

fn read_persisted(file: &Path) -> Persisted {
    let Ok(bytes) = std::fs::read(file) else { return Persisted::default() };
    match serde_json::from_slice::<Persisted>(&bytes) {
        Ok(mut p) if p.version == PERSIST_VERSION => {
            let cutoff = now_secs().saturating_sub(PERSIST_TTL_SECS);
            p.entries.retain(|_, (at, _)| *at >= cutoff);
            p
        }
        // Unknown version or a torn/corrupt file: start over (it is only a cache).
        _ => Persisted::default(),
    }
}

/// The journal file for `ws` in the user's cache folder, or `None` when disabled
/// (`VX_NO_JOURNAL=1`) or no cache folder is known. `VX_JOURNAL_DIR` overrides the folder.
pub(crate) fn persist_file(ws: &Path) -> Option<PathBuf> {
    if std::env::var_os("VX_NO_JOURNAL").is_some_and(|v| v != "0") {
        return None;
    }
    let dir = if let Some(d) = std::env::var_os("VX_JOURNAL_DIR") {
        PathBuf::from(d)
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("vx").join("cache")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library").join("Caches").join("vx")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?
            .join("vx")
    };
    Some(dir.join(format!("journal-{:08x}.json", xxh32(key(ws).as_bytes(), 0))))
}

/// Adds the entries saved in `file` to the in-memory table (entries already there win).
pub(crate) fn load_persisted(file: &Path) {
    let p = read_persisted(file);
    let mut base = HashMap::new();
    if let Ok(mut t) = table().lock() {
        for (k, (_, j)) in p.entries {
            base.insert(k.clone(), digest(&j));
            t.entry(k).or_insert(j);
        }
    }
    if let Ok(mut b) = baselines().lock() {
        b.insert(file.to_path_buf(), base);
    }
}

/// Merges what this process changed since [`load_persisted`] into `file` (re-read first, so
/// other processes' entries survive) and writes it atomically. Best effort: errors are ignored.
pub(crate) fn save_persisted(file: &Path) {
    let Some(base) = baselines().lock().ok().and_then(|mut b| b.remove(file)) else { return };
    let mut disk = read_persisted(file);
    let now = now_secs();
    {
        let Ok(t) = table().lock() else { return };
        for (k, j) in t.iter() {
            let d = digest(j);
            if base.get(k) == Some(&d) {
                continue; // untouched here: keep whatever is on disk now
            }
            if j.last.len() > PERSIST_MAX_ROWS {
                disk.entries.remove(k);
            } else {
                disk.entries.insert(k.clone(), (now, j.clone()));
            }
        }
        // Forgotten here (file deleted/moved): drop it, unless another process updated it since.
        for (k, d0) in &base {
            if !t.contains_key(k) && disk.entries.get(k).is_some_and(|(_, j)| digest(j) == *d0) {
                disk.entries.remove(k);
            }
        }
    }
    if disk.entries.len() > MAX_FILES {
        let mut by_age: Vec<(u64, String)> = disk.entries.iter().map(|(k, (at, _))| (*at, k.clone())).collect();
        by_age.sort();
        for (_, k) in by_age.into_iter().take(disk.entries.len() - MAX_FILES) {
            disk.entries.remove(&k);
        }
    }
    disk.version = PERSIST_VERSION;
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(&disk) else { return };
    // Write-then-rename so a concurrent reader never sees a half-written file.
    let tmp = dir.join(format!(".{}.{}.tmp", file.file_name().and_then(|n| n.to_str()).unwrap_or("journal"), std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Start journaling `full` at its current text, because it is being displayed (a write would start
/// it too, but then everything printed before is unknown).
fn begin_at_display(full: &Path) {
    let k = key(full);
    if table().lock().map_or(true, |t| t.contains_key(&k)) {
        return;
    }
    if std::fs::metadata(full).map_or(true, |m| !m.is_file() || m.len() > 4_000_000) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(full) else { return };
    let lines = Doc::parse(&text).lines;
    if lines.len() > MAX_ROWS {
        return;
    }
    let mut j = Journal::new(full);
    j.snapshot(&lines, fingerprint(&text));
    j.seen_from_start = true;
    if let Ok(mut t) = table().lock() {
        if t.len() >= MAX_FILES {
            t.clear();
        }
        t.entry(k).or_insert(j);
    }
}

/// Everything printed to the caller (`N#hh` tokens) counts as shown for each journaled file whose
/// current row `N` has hash `hh`. Over-marking is harmless; missing a display would not be, so this
/// runs on the text of every tool result, whichever tool printed it. Files the call names (or that
/// a `path:N#hh` hit names) start a journal if they have none, so version 0 is tracked too.
pub(crate) fn note_display(ws: &Path, args: &serde_json::Value, output: &str) {
    let re = lazy_re!(r"(\d+)#([0-9a-f]{2})");
    let toks: Vec<(usize, u8)> = re
        .captures_iter(output)
        .take(4000)
        .filter_map(|c| Some((c[1].parse::<usize>().ok()?, u8::from_str_radix(&c[2], 16).ok()?)))
        .filter(|(n, _)| *n >= 1)
        .collect();
    if toks.is_empty() {
        return;
    }
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from);
    let mut named: Vec<String> = Vec::new();
    if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        named.push(p.to_string());
    }
    for spec in args.get("specs").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        named.push(super::view::split_view_spec(spec).0.to_string());
    }
    for e in args.get("edits").and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(p) = e.get("path").and_then(|v| v.as_str()) {
            named.push(p.to_string());
        }
    }
    // `search` / `ast` hits: `dir/file.rs:12#ab|...`
    for c in lazy_re!(r"(?m)^\s*([^\s:#|]+\.[A-Za-z0-9]+):\d+#[0-9a-f]{2}").captures_iter(output).take(200) {
        if !named.iter().any(|n| *n == c[1]) {
            named.push(c[1].to_string());
        }
    }
    for p in named.iter().take(60) {
        if let Ok((full, _)) = super::util::resolve_existing(ws, cwd.as_deref(), p, true, false) {
            begin_at_display(&full);
        }
    }
    let Ok(mut t) = table().lock() else { return };
    for j in t.values_mut() {
        j.freshen();
        for (n, h) in &toks {
            if j.anchor_hashes.get(n - 1) == Some(h) {
                if let Some(s) = j.shown.last_mut() {
                    s.insert(n - 1);
                }
            }
        }
    }
}

/// Like `locate`, but first maps the anchors forward through the writes seen on the file. A unique
/// hash-verified result wins; several that display history cannot separate is an error listing them
/// (never a guess); none leaves it to `locate`.
pub(crate) fn locate_at(
    full: &Path,
    doc: &Doc,
    rel: &str,
    s: &(usize, String),
    e: &(usize, String),
    hint: Option<&str>,
) -> Result<(usize, usize), String> {
    match journal_lookup(full, doc, s, e, hint) {
        Lookup::Found(r) => Ok(r),
        Lookup::Ambiguous(pairs) => {
            // Show each candidate's first line, so one retry with the right anchor is enough.
            let list: Vec<String> = pairs.iter().take(6).map(|(a, _)| doc.snippet_near(a + 1, 0)).collect();
            Err(format!(
                "anchor {}#{}{} in {rel} fits several lines after earlier edits; which one did you mean?\n{}",
                s.0,
                s.1,
                if e == s { String::new() } else { format!("..{}#{}", e.0, e.1) },
                list.join("\n")
            ))
        }
        Lookup::Unknown => locate(doc, rel, s, e),
    }
}

enum Lookup {
    Found((usize, usize)),
    Ambiguous(Vec<(usize, usize)>),
    Unknown,
}

fn journal_lookup(full: &Path, doc: &Doc, s: &(usize, String), e: &(usize, String), hint: Option<&str>) -> Lookup {
    if s.0 == 0 || e.0 < s.0 || doc.lines.is_empty() {
        return Lookup::Unknown;
    }
    let Ok(mut t) = table().lock() else { return Lookup::Unknown };
    let Some(j) = t.get_mut(&key(full)) else { return Lookup::Unknown };
    // `doc` is the file as it is on disk right now: fold in any write we did not see.
    j.sync(&doc.to_text());
    if j.steps.is_empty() {
        return Lookup::Unknown;
    }
    let n = doc.lines.len();
    let (sr, er) = (s.0 - 1, e.0 - 1);
    // (start, end, versions the anchor could have been printed at that lead there)
    let mut cands: Vec<((usize, usize), bool)> = Vec::new();
    for v in (0..=j.steps.len()).rev() {
        let fwd = |row: usize| j.steps[v..].iter().try_fold(row, |p, st| map_step(st, p));
        let (Some(a), Some(b)) = (fwd(sr), fwd(er)) else { continue };
        if !(a <= b && b < n && line_hash(&doc.lines[a]) == s.1 && line_hash(&doc.lines[b]) == e.1) {
            continue;
        }
        // Was this version's row actually printed to the caller?
        let seen = (v == 0 && !j.seen_from_start) || (j.shown[v].contains(&sr) && j.shown[v].contains(&er));
        match cands.iter_mut().find(|(p, _)| *p == (a, b)) {
            Some(c) => c.1 |= seen,
            None => cands.push(((a, b), seen)),
        }
    }
    match cands.len() {
        0 => Lookup::Unknown,
        1 => Lookup::Found(cands[0].0),
        _ => {
            let seen: Vec<(usize, usize)> = cands.iter().filter(|c| c.1).map(|c| c.0).collect();
            let pool = if seen.is_empty() { cands.iter().map(|c| c.0).collect() } else { seen };
            // The caller copied the printed line too (`12#ab|text`): it must be the line holding that text.
            let by_text: Vec<(usize, usize)> = match hint {
                Some(h) => pool.iter().copied().filter(|(a, _)| doc.lines[*a].contains(h)).collect(),
                None => Vec::new(),
            };
            match (pool.len(), by_text.len()) {
                (1, _) => Lookup::Found(pool[0]),
                (_, 1) => Lookup::Found(by_text[0]),
                _ => Lookup::Ambiguous(sorted(pool)),
            }
        }
    }
}

fn sorted(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    v.sort();
    v
}
