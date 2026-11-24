//! Merging duplicate book records (the engine is `lexmechanic_db::merge`).
//!
//! - `POST /cdb/merge-books[/{library_id}]` -- merge `sources` into `dest`.
//!   Body `{dest, sources, mode, keep_duplicate_formats, dry_run}`;
//!   `mode` is `merge_and_delete` (default), `merge_keep_sources` or
//!   `formats_only`. Answers with a per-source report, the files sent to the
//!   library trash (.caltrash) and any warnings. With `dry_run` it only says what it would do.
//! - `POST /duplicates/merge-exact/{library_id}` -- the bulk operation for
//!   **exact-content groups only** (never title/author matches). Body
//!   `{dry_run, keep_duplicate_formats, plan_fingerprint}`. `dry_run`
//!   defaults to **true** and returns the plan with a fingerprint. Executing
//!   needs `dry_run: false` *and* the fingerprint of the plan that was shown,
//!   and is refused with 409 if the library has changed since, so what runs
//!   is what the user confirmed. It starts a cancellable `merge_exact` job in
//!   [`crate::library_jobs`] (`GET /library-jobs/{id}`, `.../cancel`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::cache::Cache;
use lexmechanic_db::merge::{self, MergeMode, MergeOptions};

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct MergeBody {
    pub dest: i32,
    pub sources: Vec<i32>,
    #[serde(default)]
    pub mode: MergeMode,
    #[serde(default)]
    pub keep_duplicate_formats: bool,
    #[serde(default)]
    pub dry_run: bool,
}

fn cache_for(state: &AppState, library_id: Option<&str>) -> Result<Arc<Cache>, ServerError> {
    state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or_default())))
}

pub async fn merge_books(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<MergeBody>) -> Result<Json<Value>, ServerError> {
    handle(state, Some(library_id), body).await
}

pub async fn merge_books_no_library(State(state): State<AppState>, Json(body): Json<MergeBody>) -> Result<Json<Value>, ServerError> {
    handle(state, None, body).await
}

async fn handle(state: AppState, library_id: Option<String>, body: MergeBody) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, library_id.as_deref())?;
    let MergeBody { dest, sources, mode, keep_duplicate_formats, dry_run } = body;
    if !dry_run {
        crate::library_jobs::ensure_no_file_mover(&cache)?;
    }
    if !sources.iter().any(|s| *s != dest) {
        return Err(ServerError::BadRequest("sources must name at least one book other than dest".to_string()));
    }
    let report = tokio::task::spawn_blocking(move || {
        for id in std::iter::once(&dest).chain(sources.iter()) {
            if cache.field_for(*id, "path")?.is_none() {
                return Ok(Err(*id));
            }
        }
        merge::merge_books(&cache, dest, &sources, &MergeOptions { mode, keep_duplicate_formats, dry_run }).map(Ok)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e: anyhow::Error| ServerError::InternalServerError(format!("{e:#}")))?;
    let report = report.map_err(|missing| ServerError::NotFound(format!("no book with id {missing}")))?;

    if !dry_run {
        let deleted: Vec<i32> = report.sources.iter().filter(|s| s.deleted).map(|s| s.book_id).collect();
        if !deleted.is_empty() {
            web_socket::publish(&state, ChangeEvent::BooksDeleted { book_ids: deleted });
        }
        if report.sources.iter().any(|s| !s.formats_moved.is_empty()) {
            web_socket::publish(&state, ChangeEvent::FormatsAdded { book_ids: vec![dest] });
        }
        web_socket::publish(&state, ChangeEvent::MetadataChanged { book_ids: vec![dest] });
    }

    let warnings: Vec<String> = report.warnings.iter().cloned().chain(report.sources.iter().flat_map(|s| s.warnings.iter().map(move |w| format!("book {}: {w}", s.book_id)))).collect();
    let failed: Vec<Value> = report.sources.iter().flat_map(|s| s.failed.iter().map(move |(path, why)| json!({"book_id": s.book_id, "path": path, "error": why}))).collect();
    let errors: Vec<Value> = report.sources.iter().filter_map(|s| s.error.as_ref().map(|e| json!({"book_id": s.book_id, "error": e}))).collect();
    Ok(Json(json!({
        "ok": errors.is_empty(),
        "dry_run": dry_run,
        "dest": dest,
        "mode": report.mode,
        "results": report.sources,
        "metadata_changed": report.metadata_changed,
        "metadata_preview": report.metadata_preview,
        "cover_from": report.cover_from,
        "trashed": report.trashed(),
        "failed": failed,
        "errors": errors,
        "warnings": warnings,
    })))
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct BulkBody {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub keep_duplicate_formats: bool,
    #[serde(default)]
    pub plan_fingerprint: Option<String>,
}

/// `POST /duplicates/merge-exact/{library_id}`.
pub async fn merge_exact(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<BulkBody>) -> Result<Response, ServerError> {
    let cache = cache_for(&state, Some(&library_id))?;
    let plan = {
        let cache = cache.clone();
        tokio::task::spawn_blocking(move || merge::plan_content_merges(&cache))
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?
    };
    let fingerprint = merge::plan_fingerprint(&plan);

    if body.dry_run {
        let groups: Vec<Value> = {
            let cache = cache.clone();
            let plan = plan.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
                plan.iter()
                    .map(|g| {
                        let mut books = Vec::new();
                        for id in std::iter::once(&g.dest).chain(g.sources.iter()) {
                            let (title, authors) = lexmechanic_db::copy_to_library::book_title_and_authors(&cache, *id)?;
                            let s = merge::book_summary(&cache, *id)?;
                            books.push(json!({"book_id": id, "title": title, "authors": authors, "formats": s.formats, "metadata": s.metadata, "has_cover": s.has_cover}));
                        }
                        Ok(json!({"dest": g.dest, "sources": g.sources, "books": books}))
                    })
                    .collect()
            })
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?
        };
        let books_to_merge: usize = plan.iter().map(|g| g.sources.len()).sum();
        return Ok(Json(json!({"dry_run": true, "groups": groups, "group_count": plan.len(), "books_to_merge": books_to_merge, "plan_fingerprint": fingerprint, "mode": "merge_and_delete"})).into_response());
    }

    let Some(confirmed) = body.plan_fingerprint else {
        return Err(ServerError::BadRequest("executing needs the plan_fingerprint of the dry run the user confirmed".to_string()));
    };
    if confirmed != fingerprint {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the library has changed since that plan was made; run the dry run again", "plan_fingerprint": fingerprint}))).into_response());
    }
    let params = JobParams { merge_plan: plan, keep_duplicate_formats: body.keep_duplicate_formats, ..Default::default() };
    Ok(match library_jobs::start_with(cache, JobKind::MergeExact, None, params) {
        Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id}))).into_response(),
    })
}

#[cfg(test)]
mod tests;
