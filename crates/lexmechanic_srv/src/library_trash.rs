//! The library's trash over HTTP (the engine is `lexmechanic_db::trash`).
//!
//! calibre's "Trash" dialog (`gui2/trash.py`): what was deleted, with its
//! title, author, size and when; Restore; Delete permanently; Empty trash.
//!
//! - `GET /trash/{library_id}` -- `{entries, expire_after_seconds, count,
//!   total_size}`. Read-only: expiry (calibre's `expire_old_trash`) runs
//!   when the server starts and on `POST /trash/expire`, never as a side
//!   effect of a GET.
//! - `POST /trash/restore/{library_id}` -- body `{kind, key, format?}`.
//!   `kind` is `book` or `format`; a format entry needs the `format` to put back.
//! - `POST /trash/delete/{library_id}` -- body `{kind, key, format?}`; for ever.
//! - `POST /trash/empty/{library_id}` -- everything, for ever.
//! - `POST /trash/expire/{library_id}` -- expire now, whatever the hour.
//!
//! Deleting and emptying are irreversible, so the UI asks first; the routes
//! do exactly what they are told.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::cache::Cache;
use lexmechanic_db::trash::TrashKind;

use crate::errors::ServerError;
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

fn cache_for(state: &AppState, library_id: &str) -> Result<Arc<Cache>, ServerError> {
    state.cache_for(Some(library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::BadRequest(format!("{e:#}")))
}

#[derive(Debug, Deserialize)]
pub struct EntryBody {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub format: Option<String>,
}

fn kind_of(body: &EntryBody) -> Result<TrashKind, ServerError> {
    TrashKind::parse(&body.kind).ok_or_else(|| ServerError::BadRequest(format!("kind must be \"book\" or \"format\", not {:?}", body.kind)))
}

/// `GET /trash/{library_id}`.
pub async fn list(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    // Read-only: a GET must not delete anything. Expiry runs when the server
    // starts and on `POST /trash/expire`.
    let cache = cache_for(&state, &library_id)?;
    let (entries, expiry) = blocking(move || Ok((cache.list_trash()?, cache.trash_expiry_seconds()))).await?;
    let total_size: i64 = entries.iter().map(|e| e.size).sum();
    Ok(Json(json!({
        "count": entries.len(),
        "total_size": total_size,
        "expire_after_seconds": expiry,
        // Kept for the web client's type; a listing expires nothing any more.
        "expired": Vec::<String>::new(),
        "entries": entries,
    })))
}

/// `POST /trash/restore/{library_id}`.
pub async fn restore(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<EntryBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    crate::library_jobs::ensure_no_file_mover(&cache)?;
    let kind = kind_of(&body)?;
    let result = blocking(move || -> anyhow::Result<Value> {
        match kind {
            TrashKind::Book => Ok(serde_json::to_value(cache.restore_trashed_book(&body.key)?)?),
            TrashKind::Format => {
                let format = body.format.clone().ok_or_else(|| anyhow::anyhow!("restoring a format needs `format`"))?;
                Ok(serde_json::to_value(cache.restore_trashed_format(&body.key, &format)?)?)
            }
        }
    })
    .await?;
    match kind {
        TrashKind::Book => {
            if let Some(id) = result["book_id"].as_i64() {
                web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: vec![id as i32] });
            }
        }
        TrashKind::Format => {
            if let Some(id) = result["book_id"].as_i64() {
                web_socket::publish(&state, ChangeEvent::FormatsAdded { book_ids: vec![id as i32] });
            }
        }
    }
    Ok(Json(result))
}

/// `POST /trash/delete/{library_id}`.
pub async fn delete(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<EntryBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let kind = kind_of(&body)?;
    blocking(move || cache.delete_trash_entry(kind, &body.key, body.format.as_deref())).await?;
    Ok(Json(json!({})))
}

/// `POST /trash/empty/{library_id}`.
pub async fn empty(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let removed = blocking(move || cache.empty_trash()).await?;
    Ok(Json(json!({"removed": removed})))
}

/// `POST /trash/expire/{library_id}`.
pub async fn expire(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let report = blocking(move || cache.expire_old_trash()).await?;
    Ok(Json(json!(report)))
}

#[cfg(test)]
pub(crate) mod tests;
