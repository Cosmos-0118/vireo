#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod files;
mod git;
mod pty;
mod state;
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
use tower_http::cors::{Any, CorsLayer};

#[derive(Embed)]
#[folder = "../frontend/dist"]
struct FrontendAssets;

async fn serve_embedded(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    let path = if raw_path.is_empty() {
        "index.html"
    } else {
        raw_path
    };

    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(file.data.into_owned()))
            .unwrap();
    }

    // SPA fallback to index.html
    if let Some(index) = FrontendAssets::get("index.html") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(index.data.into_owned()))
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
        while let Ok(ev) = rx.recv().await {
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

    let workspace = dir.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    });
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
) -> Router {
    let (events_tx, _) = tokio::sync::broadcast::channel(128);
    let watcher_tx = watcher::spawn_watcher(initial_workspace.clone(), events_tx.clone());
    let mut app_state = AppState::new(initial_workspace, watcher_tx);
    app_state.events_tx = events_tx;
    app_state.main_window = main_window;

    Router::new()
        .route("/api/health", get(health))
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
        .route("/api/files/write", post(files::write_file))
        .route("/api/files/search", get(files::search_files))
        .route("/api/tasks", get(files::get_tasks))
        .route("/api/git/status", get(git::get_status))
        .route("/api/git/diff", get(git::get_diff))
        .route("/api/git/stage", post(git::stage_files))
        .route("/api/git/unstage", post(git::unstage_files))
        .route("/api/git/discard", post(git::discard_file))
        .route("/api/git/commit", post(git::commit_changes))
        .route("/api/git/push", post(git::push_changes))
        .route("/api/git/pull", post(git::pull_changes))
        .route("/api/git/branches", get(git::get_branches))
        .route("/api/git/checkout", post(git::checkout_branch))
        .route("/api/git/log", get(git::get_log))
        .route("/api/git/checkpoint", post(git::create_checkpoint))
        .route("/api/git/rollback", post(git::rollback_checkpoint))
        .route("/api/agent/chat", post(agent::chat))
        .route("/api/agent/cli-tools", get(pty::list_cli_tools))
        .route("/ws/pty", get(pty::ws_pty))
        .route("/ws/events", get(ws_events))
        .with_state(app_state)
        .fallback(serve_embedded)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

fn main() {
    let cli = parse_args();
    let main_window: Arc<OnceLock<tauri::WebviewWindow>> = Arc::new(OnceLock::new());
    let app_router = build_router(cli.workspace.clone(), main_window.clone());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    let (listener, bound_addr) = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("Failed to bind local port");
        let local = listener.local_addr().expect("Failed to read bound address");
        (listener, local)
    });

    if cli.server_only {
        println!("Vireo server running at http://{}", bound_addr);
        println!("Workspace: {}", cli.workspace.display());
        rt.block_on(async move {
            axum::serve(listener, app_router)
                .await
                .expect("Server error");
        });
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
            let win = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(app_url),
            )
            .title(ws_title)
            .inner_size(1440.0, 900.0)
            .min_inner_size(960.0, 620.0)
            .decorations(false)
            .shadow(true)
            .zoom_hotkeys_enabled(false)
            .center()
            .build()?;
            let _ = win_slot.set(win);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Error running Vireo desktop app");
}
