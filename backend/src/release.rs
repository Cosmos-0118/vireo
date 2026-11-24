//! HTTP bridge to the release engine (`code_intel::release`), used by the Releases page.

use axum::{extract::State, http::StatusCode, Json};
use serde_json::Value;

use crate::state::AppState;

/// `POST /api/release` with `{ action, version?, dry_run?, ... }` (same arguments as `vx release`).
/// Always answers with the engine's JSON. Long stages (preflight with a build, publish) block on a
/// worker thread, never on the async runtime.
pub async fn run(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let ws = state.workspace_path().await;
    if let Some(o) = body.as_object_mut() {
        o.insert("json".into(), Value::Bool(true));
        o.insert("source".into(), Value::String("ui".into()));
    }
    let out = tokio::task::spawn_blocking(move || crate::code_intel::run_op(&ws, "release", &body))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    serde_json::from_str(&out)
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
