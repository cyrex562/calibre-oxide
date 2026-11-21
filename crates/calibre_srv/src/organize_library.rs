//! Organize a library's files by a template ([`calibre_db::organize`]).
//!
//! - `POST /organize-library/{library_id}` `{template?, book_ids? | search? |
//!   path_prefix?, dry_run?, plan_fingerprint?}` -- `dry_run` defaults to
//!   **true** and answers `{plan_fingerprint, template, books, files,
//!   conflicts, already_organized, skipped, moves}` without touching a file
//!   (`moves` is capped at 500 entries; `books` and `files` are the totals).
//!   `dry_run: false` needs the `plan_fingerprint` of the dry run the user
//!   confirmed, re-plans, and answers `409` if the library no longer produces
//!   that plan; otherwise it starts an `organize` job (202, progress and the
//!   report at `/library-jobs/{id}`).
//! - `POST /move-books/{library_id}` `{book_ids, folder, dry_run?, plan_fingerprint?}`
//!   -- moves those books' files to `folder`, relative to the library root
//!   (created if missing; absolute paths, `..` and anything leaving the library
//!   are 400, see [`organize::validate_folder`]). The same plan and executor as
//!   organize with an explicit destination ([`organize::Target::Folder`]):
//!   each file keeps its name (a taken name gets ` (2)`), a book's files move
//!   all or none, empty old folders go, and the run is recorded and undone with
//!   `POST /unorganize-library`. `dry_run` defaults to **true** and answers the
//!   plan, like organize; `dry_run: false` starts an `organize` job (202), and
//!   with a `plan_fingerprint` is refused (409) if the plan changed.
//! - `POST /unorganize-library/{library_id}` `{run_id}` -- an `undo_organize` job.
//! - `GET /organize-runs/{library_id}` -- the runs that can be undone.
//!
//! The default template is `{author_sort}/{title}`; the scope is the selected
//! ids, a search, or a folder prefix (relative to the library), else every book.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::organize::{self, Scope};

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::AppState;

const MAX_MOVES_SHOWN: usize = 500;

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct OrganizeBody {
    pub template: Option<String>,
    pub book_ids: Option<Vec<i32>>,
    pub search: Option<String>,
    pub path_prefix: Option<String>,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub plan_fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UndoBody {
    pub run_id: String,
}

fn started(kind: &str, started: Started) -> Response {
    match started {
        Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id, "kind": active.kind.name()}))).into_response(),
    }
}

fn internal(e: impl std::fmt::Display) -> ServerError {
    ServerError::InternalServerError(e.to_string())
}

/// What a dry run answers.
fn plan_json(plan: &organize::OrganizePlan) -> Value {
    let moves: Vec<Value> = plan.books.iter().flat_map(|b| b.files.iter().map(move |f| json!({"book_id": b.book_id, "title": b.title, "format": f.format, "from": f.from, "to": f.to, "suffixed": b.suffixed}))).take(MAX_MOVES_SHOWN).collect();
    json!({
        "plan_fingerprint": plan.fingerprint,
        "template": plan.template,
        "books": plan.books.len(),
        "files": plan.file_count(),
        "conflicts": plan.conflicts(),
        "already_organized": plan.already_organized,
        "skipped": plan.skipped,
        "moves": moves,
    })
}

/// `POST /organize-library/{library_id}`.
pub async fn organize(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<OrganizeBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let template = body.template.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| organize::DEFAULT_TEMPLATE.to_string());
    let scopes = [body.book_ids.is_some(), body.search.is_some(), body.path_prefix.is_some()].iter().filter(|b| **b).count();
    if scopes > 1 {
        return Err(ServerError::BadRequest("give one of book_ids, search or path_prefix (or none for every book)".into()));
    }
    if body.dry_run == false && body.plan_fingerprint.is_none() {
        return Err(ServerError::BadRequest("executing needs the plan_fingerprint of the dry run the user confirmed".into()));
    }

    let (plan_cache, plan_template) = (cache.clone(), template.clone());
    let plan = tokio::task::spawn_blocking(move || -> Result<organize::OrganizePlan, ServerError> {
        let scope = if let Some(ids) = body.book_ids {
            Scope::Ids(ids)
        } else if let Some(query) = body.search {
            Scope::Ids(calibre_db::search::search(&plan_cache, &query).map_err(|e| ServerError::BadRequest(format!("{e:#}")))?)
        } else if let Some(prefix) = body.path_prefix {
            Scope::PathPrefix(prefix)
        } else {
            Scope::All
        };
        organize::plan(&plan_cache, &plan_template, &scope).map_err(internal)
    })
    .await
    .map_err(internal)??;

    if body.dry_run {
        return Ok(Json(plan_json(&plan)).into_response());
    }

    if body.plan_fingerprint.as_deref() != Some(plan.fingerprint.as_str()) {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the plan is no longer what was previewed (the library or the selection changed); run the dry run again", "plan_fingerprint": plan.fingerprint}))).into_response());
    }
    if plan.books.is_empty() {
        return Ok(Json(json!({"error": "nothing to organize", "books": 0})).into_response());
    }
    let params = JobParams { organize: Some(plan), ..Default::default() };
    Ok(started("organize", library_jobs::start_with(cache, JobKind::Organize, None, params)))
}

/// `POST /unorganize-library/{library_id}`.
pub async fn unorganize(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<UndoBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let library = cache.backend.library_path.clone();
    let run_id = body.run_id.clone();
    let run = tokio::task::spawn_blocking(move || organize::load_run(&library, &run_id)).await.map_err(internal)?.map_err(|e| ServerError::NotFound(format!("{e:#}")))?;
    if run.undone_at.is_some() {
        return Err(ServerError::BadRequest(format!("run {} was already undone", run.run_id)));
    }
    let params = JobParams { undo_run: Some(body.run_id), ..Default::default() };
    Ok(started("undo_organize", library_jobs::start_with(cache, JobKind::UndoOrganize, None, params)))
}

/// `GET /organize-runs/{library_id}`.
pub async fn runs(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let library = cache.backend.library_path.clone();
    let runs = tokio::task::spawn_blocking(move || organize::list_runs(&library)).await.map_err(internal)?;
    Ok(Json(json!({"runs": runs.into_iter().map(|(run_id, created_at, books, undone)| json!({"run_id": run_id, "created_at": created_at, "books": books, "undone": undone})).collect::<Vec<_>>()})))
}


const MAX_MOVE_BOOKS: usize = 500;

#[derive(Debug, Deserialize)]
pub struct MoveBody {
    pub book_ids: Vec<i32>,
    pub folder: String,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub plan_fingerprint: Option<String>,
}

/// `POST /move-books/{library_id}`.
pub async fn move_books(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<MoveBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let folder = organize::validate_folder(&body.folder).map_err(ServerError::BadRequest)?;
    let mut ids = body.book_ids.clone();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(ServerError::BadRequest("book_ids is empty".into()));
    }
    if ids.len() > MAX_MOVE_BOOKS {
        return Err(ServerError::BadRequest(format!("at most {MAX_MOVE_BOOKS} books can be moved in one request (got {})", ids.len())));
    }

    let (plan_cache, plan_folder) = (cache.clone(), folder.clone());
    let plan = tokio::task::spawn_blocking(move || -> Result<organize::OrganizePlan, ServerError> {
        for id in &ids {
            if plan_cache.field_for(*id, "path").map_err(internal)?.is_none() {
                return Err(ServerError::NotFound(format!("no book with id {id}")));
            }
        }
        organize::plan_to(&plan_cache, organize::Target::Folder(&plan_folder), &Scope::Ids(ids)).map_err(|e| ServerError::BadRequest(format!("{e:#}")))
    })
    .await
    .map_err(internal)??;

    if body.dry_run {
        let mut out = plan_json(&plan);
        out["folder"] = json!(folder);
        return Ok(Json(out).into_response());
    }
    if let Some(confirmed) = body.plan_fingerprint.as_deref() {
        if confirmed != plan.fingerprint {
            return Ok((StatusCode::CONFLICT, Json(json!({"error": "the plan is no longer what was previewed (the library or the selection changed); run the dry run again", "plan_fingerprint": plan.fingerprint}))).into_response());
        }
    }
    if plan.books.is_empty() {
        return Err(ServerError::BadRequest("nothing to move: the books are already in that folder, or cannot be moved (see skipped in the dry run)".into()));
    }
    let params = JobParams { organize: Some(plan), ..Default::default() };
    Ok(started("organize", library_jobs::start_with(cache, JobKind::Organize, None, params)))
}
