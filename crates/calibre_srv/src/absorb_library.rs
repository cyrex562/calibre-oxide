//! Bringing a calibre library that sits inside this library in, in place (the
//! engine is `calibre_db::absorb`; read its module docs for what is carried).
//!
//! - `GET /absorbable-libraries/{library_id}` -- the calibre libraries found
//!   inside the library by a bounded, time-budgeted walk, each with its book
//!   count and whether it has been absorbed (`state`: `available` or
//!   `absorbed`).
//! - `POST /absorb-library/{library_id}` with `{path}` -- a folder inside the
//!   library (relative to it, or absolute) holding a calibre `metadata.db`.
//!   Optional: `allow_failures` (finish, and rename the database, even though
//!   some books failed) and `keep_database_name`. Answers `202` with the job's
//!   id; progress, cancel and the report are on `/library-jobs/{id}`. Running
//!   it again resumes: books already in are skipped.
//! - `POST /unabsorb-library/{library_id}` with `{path}` -- the undo: puts
//!   `metadata.db` back and removes the folder's books from the library's
//!   database (rows only, never a file). A job too.
//!
//! Refusals come back as `400` before any job starts: a path outside the
//! library, the library itself, no `metadata.db`, a database that something
//! appears to have open, a library of this program's own.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::absorb::AbsorbOptions;

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct AbsorbBody {
    pub path: String,
    #[serde(default)]
    pub allow_failures: bool,
    #[serde(default)]
    pub keep_database_name: bool,
}

/// `GET /absorbable-libraries/{library_id}`.
pub async fn list(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let list = tokio::task::spawn_blocking(move || calibre_db::absorb::find_absorbable(&cache, calibre_db::absorb::FIND_BUDGET))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?;
    Ok(Json(json!({"libraries": list.libraries, "conclusive": list.conclusive})))
}

fn started(kind: &str, started: Started) -> Response {
    match started {
        Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id, "kind": active.kind.name()}))).into_response(),
    }
}

/// `POST /absorb-library/{library_id}`.
pub async fn absorb(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<AbsorbBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    // The refusals, as an answer rather than as a job that only fails.
    let folder = body.path.trim().to_string();
    {
        let (cache, folder) = (cache.clone(), folder.clone());
        tokio::task::spawn_blocking(move || calibre_db::absorb::preflight(&cache, &folder).map(|_| ()))
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    }
    let params = JobParams { absorb: Some((folder, AbsorbOptions { allow_failures: body.allow_failures, keep_database_name: body.keep_database_name })), ..Default::default() };
    Ok(started("absorb", library_jobs::start_with(cache, JobKind::Absorb, None, params)))
}

/// `POST /unabsorb-library/{library_id}`.
pub async fn unabsorb(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<AbsorbBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let folder = body.path.trim().to_string();
    {
        let (cache, folder) = (cache.clone(), folder.clone());
        tokio::task::spawn_blocking(move || calibre_db::absorb::resolve_folder(&cache, &folder).map(|_| ()))
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    }
    let params = JobParams { absorb: Some((folder, AbsorbOptions::default())), ..Default::default() };
    Ok(started("unabsorb", library_jobs::start_with(cache, JobKind::Unabsorb, None, params)))
}

#[cfg(test)]
mod tests;
