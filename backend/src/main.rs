mod agent;
mod files;
mod git;
mod pty;
mod state;
mod watcher;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use state::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};

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

fn parse_args() -> (u16, PathBuf, Option<PathBuf>) {
    let args: Vec<String> = std::env::args().collect();
    let mut port: u16 = std::env::var("VIREO_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4317);
    let mut dir: Option<PathBuf> = None;
    let mut static_dir: Option<PathBuf> = None;

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
            "--static" if i + 1 < args.len() => {
                static_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
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
    (port, cleaned, static_dir)
}

#[tokio::main]
async fn main() {
    let (port, initial_workspace, explicit_static) = parse_args();

    let (events_tx, _) = tokio::sync::broadcast::channel(128);
    let watcher_tx = watcher::spawn_watcher(initial_workspace.clone(), events_tx.clone());
    let mut app_state = AppState::new(initial_workspace.clone(), watcher_tx);
    app_state.events_tx = events_tx;

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/workspace", get(files::get_workspace).post(files::set_workspace))
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
        .route("/ws/pty", get(pty::ws_pty))
        .route("/ws/events", get(ws_events))
        .with_state(app_state);

    // Resolve frontend dist folder automatically
    let dist_dir = explicit_static.unwrap_or_else(|| {
        let exe_parent = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let candidates = [
            PathBuf::from("../frontend/dist"),
            PathBuf::from("frontend/dist"),
            exe_parent
                .as_ref()
                .map(|p| p.join("../../../frontend/dist"))
                .unwrap_or_default(),
            exe_parent
                .as_ref()
                .map(|p| p.join("dist"))
                .unwrap_or_default(),
        ];
        candidates
            .into_iter()
            .find(|p| p.join("index.html").exists())
            .unwrap_or_else(|| PathBuf::from("frontend/dist"))
    });

    let app = if dist_dir.join("index.html").exists() {
        let index = dist_dir.join("index.html");
        api.fallback_service(ServeDir::new(dist_dir).not_found_service(ServeFile::new(index)))
    } else {
        api
    }
    .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("Vireo server running at http://{}", addr);
    println!("Workspace: {}", initial_workspace.display());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind port");
    axum::serve(listener, app).await.expect("Server error");
}
