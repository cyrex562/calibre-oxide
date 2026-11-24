//! `POST /import-folder/{library_id}` -- copy a folder tree **into** the
//! library, as a job (the engine is `lexmechanic_db::import`).
//!
//! Body: `{source, mode, policy, recursive}`.
//!
//! - `source` -- an absolute path to a folder **on the machine running the
//!   server** (the desktop app's folder picker supplies it). Never modified.
//! - `mode` -- `one_book_per_folder`, or `multiple_books_per_folder`
//!   (default): calibre's two ways of reading a folder of files.
//! - `policy` -- what to do with duplicates: `skip_exact_content` (default),
//!   `skip_title_author`, `add_formats_to_existing` (calibre's "automerge")
//!   or `add_anyway`.
//! - `recursive` -- descend into subfolders (default true).
//! - `keep_structure` -- put each book where it was in the source
//!   (`<library>/<folder in source>/<file>`) instead of calibre's
//!   `Author/Title (id)/Title - Author.ext` (default false). A name already
//!   taken gets a number; nothing is replaced.
//!
//! Answers `202` with the job's id; progress, cancel and the final report are
//! on `/library-jobs/{id}` ([`crate::library_jobs`]). A rerun on the same
//! folder skips what was imported already, so a cancelled import is resumed by
//! starting it again.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use lexmechanic_db::import::{DuplicatePolicy, ImportMode, ImportOptions};

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ImportBody {
    pub source: String,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default)]
    pub policy: DuplicatePolicy,
    #[serde(default = "default_true")]
    pub recursive: bool,
    /// Keep the source's folder structure instead of calibre's .
    #[serde(default)]
    pub keep_structure: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /import-folder/{library_id}`.
pub async fn import(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<ImportBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let source = PathBuf::from(body.source.trim());
    if body.source.trim().is_empty() || !source.is_absolute() {
        return Err(ServerError::BadRequest("source must be the absolute path of a folder".to_string()));
    }
    if !source.is_dir() {
        return Err(ServerError::BadRequest(format!("{} is not a folder this server can read", source.display())));
    }
    // Refuse a source inside the library here, with the message, rather than
    // starting a job whose only output is that error.
    if let (Ok(src), Ok(lib)) = (std::fs::canonicalize(&source), std::fs::canonicalize(&cache.backend.library_path)) {
        if src.starts_with(&lib) {
            return Err(ServerError::BadRequest("that folder is inside the library already; scan the library to add the books in it".to_string()));
        }
    }
    let options = ImportOptions { mode: body.mode, policy: body.policy, recursive: body.recursive, keep_structure: body.keep_structure };
    let params = JobParams { import: Some((source, options)), ..Default::default() };
    Ok(match library_jobs::start_with(cache, JobKind::Import, None, params) {
        Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": "import", "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": "import", "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id, "kind": active.kind.name()}))).into_response(),
    })
}

#[cfg(test)]
mod tests;
