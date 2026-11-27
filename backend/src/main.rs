#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod agent_session;
mod askpass;
mod checkpoint;
mod cli_agent;
mod code_intel;
mod files;
mod git;
mod jobobject;
mod prefs;
mod pty;
mod release;
mod secrets;
mod state;
mod tasks;
mod watcher;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use state::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

#[derive(Embed)]
#[folder = "../frontend/dist"]
struct FrontendAssets;

/// Release builds embed the assets in the binary (`Cow::Borrowed`): serve them without copying.
fn embedded_body(data: std::borrow::Cow<'static, [u8]>) -> Body {
    match data {
        std::borrow::Cow::Borrowed(b) => Body::from(axum::body::Bytes::from_static(b)),
        std::borrow::Cow::Owned(v) => Body::from(v),
    }
}

async fn serve_embedded(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    let path = if raw_path.is_empty() {
        "index.html"
    } else {
        raw_path
    };

    if path == "index.html" {
        if let Some(index) = FrontendAssets::get("index.html") {
            let html = String::from_utf8_lossy(&index.data);
            let body = prefs::inject_into_html(&html);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(body))
                .unwrap();
        }
    }

    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Vite emits content-hashed files under assets/: safe to cache forever. Everything else
        // (index.html, icons) must revalidate or a new build would never load.
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache)
            .body(embedded_body(file.data))
            .unwrap();
    }

    // SPA fallback to index.html
    if let Some(index) = FrontendAssets::get("index.html") {
        let html = String::from_utf8_lossy(&index.data);
        let body = prefs::inject_into_html(&html);
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(body))
            .unwrap();
    }

    (StatusCode::NOT_FOUND, "Frontend assets not built").into_response()
}

async fn ws_events(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, state))
}

async fn handle_events_socket(socket: WebSocket, state: AppState) {
    let mut rx = state.events_tx.subscribe();
    let (mut ws_tx, mut ws_rx) = socket.split();

    let send_task = tokio::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                // A burst overran the channel: tell the client to do one full refresh instead of
                // silently dropping the connection.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    state::WorkspaceEvent::FsChanged { paths: Vec::new() }
                }
                Err(_) => break,
            };
            if let Ok(json) = serde_json::to_string(&ev) {
                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct ZoomReq {
    factor: f64,
}

#[derive(Serialize)]
struct ZoomRes {
    factor: f64,
    native: bool,
}

async fn set_window_zoom(
    State(state): State<AppState>,
    Json(req): Json<ZoomReq>,
) -> Json<ZoomRes> {
    let clamped = req.factor.clamp(0.7, 2.0);
    let native = if let Some(win) = state.main_window.get() {
        win.set_zoom(clamped).is_ok()
    } else {
        false
    };
    Json(ZoomRes {
        factor: clamped,
        native,
    })
}

#[derive(Deserialize)]
struct WindowControlReq {
    /// One of: "state", "minimize", "toggle_maximize", "close", "drag".
    action: String,
}

#[derive(Serialize)]
struct WindowControlRes {
    native: bool,
    maximized: bool,
    /// `std::env::consts::OS` ("macos", "windows", ...): macOS uses the native traffic lights.
    platform: &'static str,
}

/// Window controls for the custom (frameless) title bar. The page is served
/// over HTTP rather than Tauri IPC, so the frontend drives the native window
/// through this endpoint. In server-only mode there is no window and every
/// action reports `native: false`.
async fn window_control(
    State(state): State<AppState>,
    Json(req): Json<WindowControlReq>,
) -> Json<WindowControlRes> {
    let Some(win) = state.main_window.get() else {
        return Json(WindowControlRes {
            native: false,
            maximized: false,
            platform: std::env::consts::OS,
        });
    };
    match req.action.as_str() {
        "minimize" => {
            let _ = win.minimize();
        }
        "toggle_maximize" => {
            if win.is_maximized().unwrap_or(false) {
                let _ = win.unmaximize();
            } else {
                let _ = win.maximize();
            }
        }
        "close" => {
            let _ = win.close();
        }
        "drag" => {
            let _ = win.start_dragging();
        }
        _ => {}
    }
    Json(WindowControlRes {
        native: true,
        maximized: win.is_maximized().unwrap_or(false),
        platform: std::env::consts::OS,
    })
}

struct CliArgs {
    port: u16,
    workspace: PathBuf,
    server_only: bool,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();
    let mut port: u16 = std::env::var("VIREO_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0); // 0 = pick any free local port automatically
    let mut dir: Option<PathBuf> = None;
    let mut server_only = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" | "-p" if i + 1 < args.len() => {
                if let Ok(p) = args[i + 1].parse() {
                    port = p;
                }
                i += 2;
            }
            "--dir" | "-d" if i + 1 < args.len() => {
                dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--server-only" => {
                server_only = true;
                i += 1;
            }
            arg if !arg.starts_with('-') && dir.is_none() => {
                let candidate = PathBuf::from(arg);
                if candidate.is_dir() {
                    dir = Some(candidate);
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    if server_only && port == 0 {
        port = 4317;
    }

    // The launch directory is the install dir (or System32) for a normally started app, so it is
    // never used as a workspace. Without an explicit folder the desktop app reopens the last one
    // it had open; otherwise the path stays empty ("no workspace") and the user opens one from
    // the UI. Server-only mode stays explicit.
    if dir.is_none() && !server_only {
        dir = state::load_last_workspace();
    }
    let Some(workspace) = dir else {
        return CliArgs {
            port,
            workspace: PathBuf::new(),
            server_only,
        };
    };
    let canonical = std::fs::canonicalize(&workspace).unwrap_or(workspace);
    let cleaned = PathBuf::from(
        canonical
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string(),
    );
    CliArgs {
        port,
        workspace: cleaned,
        server_only,
    }
}

fn build_router(
    initial_workspace: PathBuf,
    main_window: Arc<OnceLock<tauri::WebviewWindow>>,
) -> (Router, Arc<agent_session::SessionManager>) {
    let (events_tx, _) = tokio::sync::broadcast::channel(128);
    let watcher_tx = watcher::spawn_watcher(initial_workspace.clone(), events_tx.clone());
    let mut app_state = AppState::new(initial_workspace, watcher_tx);
    app_state.events_tx = events_tx;
    app_state.main_window = main_window;
    let sessions = app_state.sessions.clone();

    let router = Router::new()
        .route("/api/health", get(health))
        .route("/api/prefs", get(prefs::get_prefs).patch(prefs::patch_prefs))
        .route("/api/prefs/bootstrap.js", get(prefs::bootstrap_js))
        .route("/api/window/zoom", post(set_window_zoom))
        .route("/api/window/control", post(window_control))
        .route(
            "/api/workspace",
            get(files::get_workspace).post(files::set_workspace),
        )
        .route("/api/workspace/pick", post(files::pick_workspace))
        .route("/api/files/tree", get(files::get_tree))
        .route("/api/files/list", get(files::list_files))
        .route("/api/files/read", get(files::read_file))
        .route("/api/files/raw", get(files::read_raw))
        .route("/api/files/write", post(files::write_file))
        .route("/api/files/search", post(files::search_files))
        .route("/api/files/replace", post(files::replace_in_files))
        .route("/api/tasks", get(tasks::get_tasks))
        .route(
            "/api/tasks/custom",
            post(tasks::save_custom_task).delete(tasks::delete_custom_task),
        )
        .route("/api/release", post(release::run))
        .route("/api/git/status", get(git::get_status))
        .route("/api/git/diff", get(git::get_diff))
        .route("/api/git/stage", post(git::stage_files))
        .route("/api/git/unstage", post(git::unstage_files))
        .route("/api/git/discard", post(git::discard_file))
        .route("/api/git/discard-all", post(git::discard_all))
        .route("/api/git/commit", post(git::commit_changes))
        .route("/api/git/init", post(git::init_repo))
        .route("/api/git/github-status", get(git::github_status))
        .route("/api/git/publish-github", post(git::publish_github))
        .route("/api/git/generate-message", post(git::generate_commit_message))
        .route("/api/git/remote-add", post(git::add_remote))
        .route("/api/git/push", post(git::push_changes))
        .route("/api/git/pull", post(git::pull_changes))
        .route("/api/git/fetch", post(git::fetch_changes))
        .route("/api/git/auto-fetch", post(git::auto_fetch))
        .route("/api/git/branches", get(git::get_branches))
        .route("/api/git/checkout", post(git::checkout_branch))
        .route("/api/git/branch-delete", post(git::delete_branch))
        .route("/api/git/remote-prune", post(git::prune_remote_branches))
        .route("/api/git/log", get(git::get_log))
        .route("/api/git/commit-detail", get(git::get_commit_detail))
        .route("/api/git/commit-diff", get(git::get_commit_diff))
        .route("/api/git/commit-patch", get(git::get_commit_patch))
        .route("/api/git/compare-files", get(git::compare_files))
        .route("/api/git/compare-diff", get(git::compare_diff))
        .route("/api/git/cherry-pick", post(git::cherry_pick))
        .route("/api/git/revert", post(git::revert_commit))
        .route("/api/git/reset", post(git::reset_branch))
        .route("/api/git/merge", post(git::merge_ref))
        .route("/api/git/tag", post(git::create_tag))
        .route("/api/git/stash", post(git::git_stash))
        .route("/api/git/operation", post(git::git_operation))
        .route("/api/git/apply-patch", post(git::apply_patch))
        .route("/api/git/open-external", post(git::open_external))
        .route("/api/git/checkpoint", post(checkpoint::create_checkpoint))
        .route("/api/git/rollback", post(checkpoint::rollback_checkpoint))
        .route("/api/git/redo", post(checkpoint::redo_checkpoint))
        .route("/api/agent/chat", post(agent::chat))
        .route(
            "/api/agent/attachment",
            post(files::save_attachment)
                .layer(axum::extract::DefaultBodyLimit::max(files::ATTACHMENT_MAX_BYTES)),
        )
        .route("/api/agent/permission", post(cli_agent::answer_permission))
        .route("/api/agent/question", post(agent_session::answer_question))
        .route("/api/agent/steer", post(agent_session::steer_turn))
        .route("/api/agent/steer/cancel", post(agent_session::cancel_steer))
        .route("/api/agent/session/prepare", post(agent_session::prepare_session))
        .route("/api/agent/session/close", post(agent_session::close_session))
        .route("/api/agent/cli-tools", get(pty::list_cli_tools))
        .route("/api/agent/models", get(agent::list_models))
        .route("/api/agent/api-models", post(agent::list_api_models))
        .route("/api/secrets", get(secrets::get_secrets))
        .route("/api/secrets/{name}", axum::routing::put(secrets::put_secret))
        .route("/ws/pty", get(pty::ws_pty))
        .route("/api/pty/kill", post(pty::kill_pty))
        .route("/ws/events", get(ws_events))
        .route("/api/askpass", post(askpass::prompt))
        .route("/api/askpass/pending", get(askpass::list_pending))
        .route("/api/askpass/answer", post(askpass::answer))
        .layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_workspace,
        ))
        .with_state(app_state)
        .fallback(serve_embedded)
        .layer(
            CorsLayer::new()
                // Only pages served from this machine (the app itself, or a local dev server) may
                // call the workspace / git / PTY / agent API from a browser context.
                .allow_origin(AllowOrigin::predicate(|origin, _| {
                    origin
                        .to_str()
                        .ok()
                        .and_then(|o| url::Url::parse(o).ok())
                        .is_some_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
                }))
                .allow_methods(Any)
                .allow_headers(Any),
        );
    (router, sessions)
}

/// Routes that operate on the open folder are refused while no folder is open.
async fn require_workspace(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path();
    let needs_ws = path.starts_with("/api/files")
        || path.starts_with("/api/git")
        || path.starts_with("/api/tasks")
        || path == "/api/agent/chat"
        || path == "/api/agent/session/prepare"
        || path == "/ws/pty";
    if needs_ws && state.workspace_path().await.as_os_str().is_empty() {
        return (StatusCode::CONFLICT, "No folder is open").into_response();
    }
    next.run(req).await
}

/// WebView2 launch flags: switch off Edge's background services (updater, sync, telemetry,
/// translate, media router...) that a local workbench never uses. This replaces wry's default
/// `--disable-features` list, so that list is repeated here. Occlusion detection stays on so a
/// covered window still throttles.
const WEBVIEW2_ARGS: &str = "--disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --metrics-recording-only --disable-default-apps --no-first-run --disable-speech-api --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,Translate,OptimizationHints,MediaRouter";

/// How long app exit waits for shells and agent CLIs to stop before the OS-level job object
/// terminates whatever is left.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(2500);

/// Stops every shell and agent CLI process, once. Safe to call again (later calls do nothing).
fn shutdown(rt: &tokio::runtime::Runtime, sessions: &agent_session::SessionManager) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    rt.block_on(async {
        let shells = tokio::task::spawn_blocking(|| pty::shutdown_all(SHUTDOWN_GRACE));
        let _ = tokio::join!(sessions.shutdown_all(SHUTDOWN_GRACE), shells);
    });
}

/// Server-only mode: resolves on Ctrl+C / Ctrl+Break (and closing the console window on Windows).
async fn shutdown_signal() {
    #[cfg(windows)]
    {
        let close = async {
            match tokio::signal::windows::ctrl_close() {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = close => {}
        }
    }
    #[cfg(not(windows))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
fn main() {
    // Started by git/ssh as GIT_ASKPASS / SSH_ASKPASS: relay the prompt to the running app.
    if let Some(code) = askpass::helper_exit_code() {
        std::process::exit(code);
    }
    // First thing, before any child process exists: children must inherit the kill-on-close job
    // so no shell/dev server/agent can outlive the app and keep the workspace folder locked.
    jobobject::init();
    // Bundled git/rg/gh (installer `tools/` folder) for machines that lack them. Must run before
    // any thread is spawned, since it edits this process's environment.
    code_intel::use_bundled_tools();
    // Registered lazily: the (cached) PATH scan must not delay startup.
    code_intel::set_path_provider(pty::augmented_path_env);
    std::thread::spawn(|| {
        let _ = pty::augmented_path_env();
    });
    let cli = parse_args();
    let main_window: Arc<OnceLock<tauri::WebviewWindow>> = Arc::new(OnceLock::new());
    let (app_router, sessions) = build_router(cli.workspace.clone(), main_window.clone());

    // A single-user local app: a handful of workers is plenty (blocking work has its own pool).
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    // Desktop mode prefers a dedicated local port (4318, distinct from server-only/dev 4317)
    // so origin-scoped browser storage stays stable for single-window launches, falling back to
    // an ephemeral port when another instance is already running. Global preferences also persist
    // on disk via `/api/prefs` so every window shares the same settings regardless of port.
    let (listener, bound_addr) = rt.block_on(async {
        let listener = if !cli.server_only && cli.port == 0 {
            const DESKTOP_PORT: u16 = 4318;
            match tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], DESKTOP_PORT)))
                .await
            {
                Ok(l) => l,
                Err(_) => tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
                    .await
                    .expect("Failed to bind local port"),
            }
        } else {
            tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], cli.port)))
                .await
                .expect("Failed to bind local port")
        };
        let local = listener.local_addr().expect("Failed to read bound address");
        (listener, local)
    });
    askpass::init(bound_addr.port());

    if cli.server_only {
        println!("Vireo server running at http://{}", bound_addr);
        if cli.workspace.as_os_str().is_empty() {
            println!("Workspace: (none - open a folder from the UI)");
        } else {
            println!("Workspace: {}", cli.workspace.display());
        }
        rt.block_on(async {
            let _ = axum::serve(listener, app_router)
                .with_graceful_shutdown(shutdown_signal())
                .await;
        });
        shutdown(&rt, &sessions);
        return;
    }

    // Run embedded Rust server inside Tokio thread pool
    rt.spawn(async move {
        let _ = axum::serve(listener, app_router).await;
    });

    let app_url: url::Url = format!("http://{}", bound_addr)
        .parse()
        .expect("Invalid local URL");
    let ws_title = cli
        .workspace
        .file_name()
        .and_then(|s| s.to_str())
        .map(|n| format!("{} - Vireo", n))
        .unwrap_or_else(|| "Vireo".to_string());

    let win_slot = main_window.clone();
    tauri::Builder::default()
        .setup(move |app| {
            let (bg_r, bg_g, bg_b) = prefs::initial_window_bg();
            let builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(app_url),
            )
            .title(ws_title)
            .inner_size(1440.0, 900.0)
            .min_inner_size(960.0, 620.0);
            // macOS keeps its native traffic lights (close/minimize/zoom, full screen, window
            // snapping) over the page's title bar; the page leaves room for them on the left.
            #[cfg(target_os = "macos")]
            let builder = builder
                .decorations(true)
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true)
                .traffic_light_position(tauri::LogicalPosition::new(14.0, 19.0));
            #[cfg(not(target_os = "macos"))]
            let builder = builder.decorations(false);
            let win = builder
            .shadow(true)
            .zoom_hotkeys_enabled(false)
            // Tauri's native file-drop handler swallows HTML5 drag events on Windows, which
            // breaks in-app drags (explorer -> agent/terminal) and OS file drops in the webview.
            .disable_drag_drop_handler()
            .background_color(tauri::window::Color(bg_r, bg_g, bg_b, 255))
            .additional_browser_args(WEBVIEW2_ARGS)
            .center()
            .build()?;
            let _ = win_slot.set(win);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Error building Vireo desktop app")
        .run(move |_app, event| {
            // The window is already gone here; stop shells and agent CLIs cleanly before the
            // process ends instead of leaving them to be terminated by the OS.
            if matches!(event, tauri::RunEvent::Exit) {
                shutdown(&rt, &sessions);
            }
        });
}
