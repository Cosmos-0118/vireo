//! API-key storage in the OS credential store (Windows Credential Manager, macOS Keychain,
//! Linux kernel keyring) so keys never sit as clear text in the webview's localStorage.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::collections::HashMap;

const SERVICE: &str = "vireo-api-keys";

/// Provider ids are short slugs ("openai", "anthropic", ...); reject anything else.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, name).map_err(|e| e.to_string())
}

fn read(name: &str) -> Result<Option<String>, String> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn write(name: &str, value: &str) -> Result<(), String> {
    let e = entry(name)?;
    if value.is_empty() {
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    } else {
        e.set_password(value).map_err(|err| err.to_string())
    }
}

#[derive(Deserialize)]
pub struct NamesQuery {
    /// Comma-separated provider ids.
    names: String,
}

#[derive(Deserialize)]
pub struct SecretBody {
    value: String,
}

type ApiErr = (StatusCode, String);

fn internal(e: String) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e)
}

/// `GET /api/secrets?names=openai,anthropic` -> `{ "openai": "sk-..." }` (only names that are set).
pub async fn get_secrets(
    Query(q): Query<NamesQuery>,
) -> Result<Json<HashMap<String, String>>, ApiErr> {
    let names: Vec<String> = q
        .names
        .split(',')
        .map(str::trim)
        .filter(|n| valid_name(n))
        .map(String::from)
        .collect();
    let out = tokio::task::spawn_blocking(move || {
        let mut map = HashMap::new();
        for n in names {
            if let Some(v) = read(&n)? {
                map.insert(n, v);
            }
        }
        Ok::<_, String>(map)
    })
    .await
    .map_err(|e| internal(e.to_string()))?
    .map_err(internal)?;
    Ok(Json(out))
}

/// `PUT /api/secrets/{name}` with `{ "value": "..." }`; an empty value deletes the credential.
pub async fn put_secret(
    Path(name): Path<String>,
    Json(body): Json<SecretBody>,
) -> Result<StatusCode, ApiErr> {
    if !valid_name(&name) {
        return Err((StatusCode::BAD_REQUEST, "invalid secret name".into()));
    }
    tokio::task::spawn_blocking(move || write(&name, &body.value))
        .await
        .map_err(|e| internal(e.to_string()))?
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}
