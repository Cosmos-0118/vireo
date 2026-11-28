use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Key -> `Some(value)` when persisted, `None` when explicitly cleared.
/// Keys absent from the map are left untouched in the browser's `localStorage`.
pub type PrefsMap = BTreeMap<String, Option<String>>;

pub const BOOTSTRAP_TAG: &str = r#"<script src="/api/prefs/bootstrap.js"></script>"#;
const THEME_META_KEY: &str = "vireo-theme-meta";
const DEFAULT_WINDOW_BG: (u8, u8, u8) = (27, 28, 31);
const MAX_KEY_LEN: usize = 128;

static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PREFS_CACHE: OnceLock<Mutex<Option<PrefsMap>>> = OnceLock::new();
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapWindow {
    pub native: bool,
    pub maximized: bool,
    pub platform: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapPayload {
    pub workspace: crate::files::WorkspaceInfo,
    pub window: BootstrapWindow,
    pub shell: String,
}

pub async fn build_bootstrap(state: &crate::state::AppState) -> BootstrapPayload {
    let ws = state.workspace_path().await;
    let name = ws
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(if ws.as_os_str().is_empty() { "Vireo" } else { "workspace" })
        .to_string();
    let workspace = crate::files::WorkspaceInfo {
        path: ws.to_string_lossy().to_string(),
        name,
    };
    let window = match state.main_window.get() {
        Some(win) => BootstrapWindow {
            native: true,
            maximized: win.is_maximized().unwrap_or(false),
            platform: std::env::consts::OS,
        },
        None => BootstrapWindow {
            native: false,
            maximized: false,
            platform: std::env::consts::OS,
        },
    };
    BootstrapPayload {
        workspace,
        window,
        shell: crate::pty::default_shell_name(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchPrefsReq {
    #[serde(default)]
    pub set: HashMap<String, String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Serialize)]
struct OkRes {
    ok: bool,
}

#[derive(Deserialize)]
struct ThemeMeta {
    #[serde(rename = "type")]
    theme_type: Option<String>,
    vars: Option<BTreeMap<String, String>>,
}

fn prefs_file() -> Option<PathBuf> {
    Some(crate::state::state_dir()?.join("prefs.json"))
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_LEN && !key.chars().any(|c| c.is_control())
}

fn load_from(path: &Path) -> PrefsMap {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return PrefsMap::new();
    };
    serde_json::from_str::<PrefsMap>(&raw).unwrap_or_default()
}

fn prefs_cache() -> &'static Mutex<Option<PrefsMap>> {
    PREFS_CACHE.get_or_init(|| Mutex::new(None))
}

fn load_prefs() -> PrefsMap {
    if let Some(cached) = prefs_cache().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return cached.clone();
    }
    let Some(path) = prefs_file() else {
        return PrefsMap::new();
    };
    let loaded = load_from(&path);
    *prefs_cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(loaded.clone());
    loaded
}

/// Re-reads the file under a lock, merges the patch, and atomically replaces it so multiple
/// Vireo windows do not clobber each other's unrelated preference changes.
fn patch_at(path: &Path, req: PatchPrefsReq) -> Result<PrefsMap, String> {
    let lock = WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    let mut map = load_from(path);
    for key in req.remove {
        if is_valid_key(&key) {
            map.insert(key, None);
        }
    }
    for (key, val) in req.set {
        if is_valid_key(&key) {
            map.insert(key, Some(val));
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create prefs dir: {e}"))?;
    }

    let bytes = serde_json::to_vec_pretty(&map).map_err(|e| format!("serialize prefs: {e}"))?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), seq));

    if let Err(e) = std::fs::write(&tmp, &bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write temp prefs: {e}"));
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        // On Windows a concurrent reader can momentarily hold the target open; fall back to
        // direct overwrite if atomic rename is rejected.
        let fallback = std::fs::write(path, &bytes);
        let _ = std::fs::remove_file(&tmp);
        fallback.map_err(|fe| format!("persist prefs ({e}; fallback: {fe})"))?;
    }

    if prefs_file().as_deref() == Some(path) {
        *prefs_cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(map.clone());
    }

    Ok(map)
}

fn parse_hex_rgb(raw: &str) -> Option<(u8, u8, u8)> {
    let hex = raw.trim().strip_prefix('#')?;
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some((r * 17, g * 17, b * 17))
        }
        6 | 8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

fn parse_theme_meta(prefs: &PrefsMap) -> Option<ThemeMeta> {
    let raw = prefs.get(THEME_META_KEY)?.as_deref()?;
    serde_json::from_str::<ThemeMeta>(raw).ok()
}

fn initial_window_bg_from(prefs: &PrefsMap) -> (u8, u8, u8) {
    parse_theme_meta(prefs)
        .and_then(|meta| meta.vars)
        .and_then(|vars| vars.get("--bg-base").and_then(|v| parse_hex_rgb(v)))
        .unwrap_or(DEFAULT_WINDOW_BG)
}

/// Returns the saved theme's base background color for the native window so reopening does not
/// flash a mismatched dark/light frame before HTML paints.
pub fn initial_window_bg() -> (u8, u8, u8) {
    initial_window_bg_from(&load_prefs())
}

fn is_safe_css_var_name(name: &str) -> bool {
    name.starts_with("--")
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_safe_css_value(val: &str) -> bool {
    !val.is_empty()
        && val.len() <= 128
        && val.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'#' | b'(' | b')' | b',' | b'.' | b'%' | b' ' | b'-' | b'_')
        })
}

fn render_theme_style(prefs: &PrefsMap) -> String {
    let Some(meta) = parse_theme_meta(prefs) else {
        return String::new();
    };
    let mut decls = String::new();
    if let Some(t) = meta.theme_type.as_deref() {
        if t == "dark" || t == "light" {
            decls.push_str("color-scheme:");
            decls.push_str(t);
            decls.push(';');
        }
    }
    if let Some(vars) = meta.vars {
        for (k, v) in vars {
            if is_safe_css_var_name(&k) && is_safe_css_value(&v) {
                decls.push_str(&k);
                decls.push(':');
                decls.push_str(&v);
                decls.push(';');
            }
        }
    }
    if decls.is_empty() {
        String::new()
    } else {
        format!(":root{{{decls}}}")
    }
}

fn escape_inline_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_bootstrap_script_with(prefs: &PrefsMap, bootstrap: Option<&BootstrapPayload>) -> String {
    let json = serde_json::to_string(prefs).unwrap_or_else(|_| "{}".to_string());
    let safe_json = escape_inline_json(&json);
    let boot_assign = match bootstrap {
        Some(b) => {
            let b_json = serde_json::to_string(b).unwrap_or_else(|_| "null".to_string());
            format!("window.__VIREO_BOOTSTRAP__={};", escape_inline_json(&b_json))
        }
        None => String::new(),
    };
    format!(
        "(function(){{var p={safe_json};window.__VIREO_PREFS__=p;{boot_assign}try{{for(var k in p){{if(!Object.prototype.hasOwnProperty.call(p,k))continue;var v=p[k];if(v===null)localStorage.removeItem(k);else if(typeof v==='string')localStorage.setItem(k,v);}}}}catch(e){{}}try{{var m=p['{THEME_META_KEY}'];if(typeof m==='string'){{var meta=JSON.parse(m);var r=document.documentElement;if(meta&&(meta.type==='dark'||meta.type==='light')){{r.dataset.themeType=meta.type;r.style.colorScheme=meta.type;}}if(meta&&meta.vars&&typeof meta.vars==='object'){{for(var vk in meta.vars){{if(vk.indexOf('--')===0&&typeof meta.vars[vk]==='string')r.style.setProperty(vk,meta.vars[vk]);}}}}}}}}catch(e){{}}}})();"
    )
}

fn inject_into_html_for(html: &str, prefs: &PrefsMap) -> String {
    inject_into_html_with(html, prefs, None)
}

fn inject_into_html_with(
    html: &str,
    prefs: &PrefsMap,
    bootstrap: Option<&BootstrapPayload>,
) -> String {
    if !html.contains(BOOTSTRAP_TAG) {
        return html.to_string();
    }
    let css = render_theme_style(prefs);
    let script = render_bootstrap_script_with(prefs, bootstrap);
    let replacement = if css.is_empty() {
        format!("<script>{script}</script>")
    } else {
        format!("<style>{css}</style><script>{script}</script>")
    };
    html.replacen(BOOTSTRAP_TAG, &replacement, 1)
}

/// Replaces the `<script src="/api/prefs/bootstrap.js"></script>` placeholder in embedded
/// `index.html` with inline theme styles and synchronous `localStorage` hydration.
#[allow(dead_code)]
pub fn inject_into_html(html: &str) -> String {
    inject_into_html_for(html, &load_prefs())
}

pub fn inject_into_html_with_bootstrap(html: &str, bootstrap: &BootstrapPayload) -> String {
    inject_into_html_with(html, &load_prefs(), Some(bootstrap))
}

pub async fn get_prefs() -> Response {
    let map = load_prefs();
    let body = serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(body))
        .unwrap()
}

pub async fn patch_prefs(Json(req): Json<PatchPrefsReq>) -> Response {
    let Some(path) = prefs_file() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "State directory unavailable").into_response();
    };
    match tokio::task::spawn_blocking(move || patch_at(&path, req)).await {
        Ok(Ok(_)) => (StatusCode::OK, Json(OkRes { ok: true })).into_response(),
        Ok(Err(err)) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn bootstrap_js(State(state): State<crate::state::AppState>) -> Response {
    let boot = build_bootstrap(&state).await;
    let script = render_bootstrap_script_with(&load_prefs(), Some(&boot));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(script))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_prefs_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vireo-prefs-test-{}-{}-{}",
            name,
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        dir.join("prefs.json")
    }

    #[test]
    fn patch_merges_across_writes_and_records_removals() {
        let path = temp_prefs_path("merge");
        let _ = patch_at(
            &path,
            PatchPrefsReq {
                set: HashMap::from([
                    ("vireo-theme".into(), "tokyo-night".into()),
                    ("vireo-zoom".into(), "1.1".into()),
                ]),
                remove: vec![],
            },
        )
        .unwrap();

        let merged = patch_at(
            &path,
            PatchPrefsReq {
                set: HashMap::from([("vireo-scm-tree".into(), "false".into())]),
                remove: vec!["vireo-zoom".into()],
            },
        )
        .unwrap();

        assert_eq!(merged.get("vireo-theme"), Some(&Some("tokyo-night".into())));
        assert_eq!(merged.get("vireo-scm-tree"), Some(&Some("false".into())));
        assert_eq!(merged.get("vireo-zoom"), Some(&None));

        let reloaded = load_from(&path);
        assert_eq!(reloaded, merged);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parses_hex_window_background_colors() {
        assert_eq!(parse_hex_rgb("#1a1b26"), Some((0x1a, 0x1b, 0x26)));
        assert_eq!(parse_hex_rgb("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_hex_rgb("#1e1e2eff"), Some((0x1e, 0x1e, 0x2e)));
        assert_eq!(parse_hex_rgb("rgb(0,0,0)"), None);

        let mut prefs = PrefsMap::new();
        assert_eq!(initial_window_bg_from(&prefs), DEFAULT_WINDOW_BG);

        prefs.insert(
            THEME_META_KEY.into(),
            Some(r##"{"type":"light","vars":{"--bg-base":"#ffffff"}}"##.into()),
        );
        assert_eq!(initial_window_bg_from(&prefs), (255, 255, 255));
    }

    #[test]
    fn html_injection_includes_style_and_escapes_script_breakout() {
        let mut prefs = PrefsMap::new();
        prefs.insert("vireo-theme".into(), Some("custom-x".into()));
        prefs.insert(
            "vireo-custom-themes".into(),
            Some(r#"[{"id":"custom-x","name":"</script><img src=x>"}]"#.into()),
        );
        prefs.insert(
            THEME_META_KEY.into(),
            Some(
                r##"{"type":"light","vars":{"--bg-base":"#ffffff","--bad":"red;</style><script>"}}"##
                    .into(),
            ),
        );

        let html = format!("<html><head>{BOOTSTRAP_TAG}</head><body></body></html>");
        let injected = inject_into_html_for(&html, &prefs);

        assert!(!injected.contains(BOOTSTRAP_TAG));
        assert!(injected.contains("<style>:root{color-scheme:light;--bg-base:#ffffff;}</style>"));
        assert!(!render_theme_style(&prefs).contains("--bad"));
        assert!(!injected.contains("</script><img"));
        assert!(!injected.contains("red;</style><script>"));
        assert_eq!(injected.matches("<script>").count(), 1);
        assert_eq!(injected.matches("</script>").count(), 1);
        assert!(injected.contains("\\u003c/script\\u003e"));
    }
}
