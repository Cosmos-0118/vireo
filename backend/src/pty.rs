use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use std::io::{Read, Write};
use tokio::sync::mpsc;

#[derive(Deserialize)]
pub struct PtyQuery {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub cmd: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientPtyMsg {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

pub async fn ws_pty(
    ws: WebSocketUpgrade,
    Query(q): Query<PtyQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_pty_socket(socket, q, state))
}

async fn handle_pty_socket(socket: WebSocket, q: PtyQuery, state: AppState) {
    let workspace = state.workspace_path().await;
    let cols = q.cols.unwrap_or(110).max(20);
    let rows = q.rows.unwrap_or(28).max(5);

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let (mut tx, _) = socket.split();
            let _ = tx
                .send(Message::Text(format!("\r\n[PTY error: {}]\r\n", e).into()))
                .await;
            return;
        }
    };

    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("powershell.exe");
        c.arg("-NoLogo");
        if let Some(custom) = q.cmd.as_deref().filter(|s| !s.trim().is_empty()) {
            c.arg("-NoExit");
            c.arg("-Command");
            c.arg(custom);
        }
        c
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut c = CommandBuilder::new(shell);
        if let Some(custom) = q.cmd.as_deref().filter(|s| !s.trim().is_empty()) {
            c.arg("-lc");
            c.arg(custom);
        }
        c
    };
    cmd.cwd(&workspace);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            let (mut tx, _) = socket.split();
            let _ = tx
                .send(Message::Text(
                    format!("\r\n[Failed to spawn shell: {}]\r\n", e).into(),
                ))
                .await;
            return;
        }
    };
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(_) => return,
    };
    let master = pair.master;

    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, mut in_rx) = mpsc::channel::<ClientPtyMsg>(256);

    // Blocking reader thread -> async channel
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if out_tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Blocking writer / resizer thread
    std::thread::spawn(move || {
        while let Some(msg) = in_rx.blocking_recv() {
            match msg {
                ClientPtyMsg::Input { data } => {
                    if writer.write_all(data.as_bytes()).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                ClientPtyMsg::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows: rows.max(4),
                        cols: cols.max(10),
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }
        let _ = child.kill();
    });

    let (mut ws_tx, mut ws_rx) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Text(chunk.into())).await.is_err() {
                break;
            }
        }
    });

    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(txt) => {
                    let s = txt.to_string();
                    let parsed = serde_json::from_str::<ClientPtyMsg>(&s)
                        .unwrap_or(ClientPtyMsg::Input { data: s });
                    if in_tx.send(parsed).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}
