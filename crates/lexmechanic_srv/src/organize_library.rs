//! Organize a library's files by a template ([`lexmechanic_db::organize`]).
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
//! - `GET /organize-runs/{library_id}` -- the runs that can be undone, each with
//!   `template`: what it was asked to do (a template, `move to X`, `rename folder A to B`).
//!
//! The default template is `{author_sort}/{title}`; the scope is the selected
//! ids, a search, or a folder prefix (relative to the library), else every book.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::organize::{self, Scope};

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

pub(crate) fn started(kind: &str, started: Started) -> Response {
    match started {
        Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": kind, "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id, "kind": active.kind.name()}))).into_response(),
    }
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ServerError {
    ServerError::InternalServerError(e.to_string())
}

/// How long a previewed plan may be applied without planning again.
const PLAN_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Plans a dry run produced, so that applying one need not compute it again
/// (planning every book of a big library takes seconds, and the preview and
/// the apply would each pay it). Newest last; only a few are kept.
type RememberedPlan = (std::time::Instant, std::path::PathBuf, String, String, organize::OrganizePlan);

fn remembered_plans() -> &'static std::sync::Mutex<Vec<RememberedPlan>> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<Vec<RememberedPlan>>> = std::sync::OnceLock::new();
    PLANS.get_or_init(Default::default)
}

/// Remembers `plan`, made for the request `key` while the library's change
/// watermark was `watermark`.
fn remember_plan(cache: &lexmechanic_db::cache::Cache, key: &str, watermark: String, plan: &organize::OrganizePlan) {
    let mut plans = remembered_plans().lock().unwrap();
    plans.retain(|(at, ..)| at.elapsed() < PLAN_TTL);
    plans.push((std::time::Instant::now(), cache.backend.library_path.clone(), key.to_string(), watermark, plan.clone()));
    if plans.len() > 4 {
        plans.remove(0);
    }
}

/// The remembered plan for this very request (`key`) with this fingerprint, if it
/// is recent and the library has not changed since it was made; otherwise the
/// caller plans again (and the fingerprint comparison decides, as it always did).
pub(crate) fn recall_plan(cache: &lexmechanic_db::cache::Cache, key: &str, fingerprint: &str) -> Option<organize::OrganizePlan> {
    let hit = remembered_plans().lock().unwrap().iter().rev().find(|(at, library, k, _, plan)| at.elapsed() < PLAN_TTL && *library == cache.backend.library_path && k == key && plan.fingerprint == fingerprint).map(|(_, _, _, mark, plan)| (mark.clone(), plan.clone()));
    let (mark, plan) = hit?;
    (plan_watermark(cache) == mark).then_some(plan)
}

/// What a remembered plan is valid for: the library's change watermark, plus
/// the database's `data_version`, which moves when another process (calibre
/// desktop on the same `metadata.db`) writes -- a write the change log and the
/// row counts do not see.
fn plan_watermark(cache: &lexmechanic_db::cache::Cache) -> String {
    format!("{}#{}", cache.change_watermark(), cache.foreign_write_version())
}

/// Forgets the plans remembered for this request: an applied plan is spent, and
/// a second apply of it plans afresh.
fn forget_plan(cache: &lexmechanic_db::cache::Cache, key: &str) {
    remembered_plans().lock().unwrap().retain(|(_, library, k, ..)| !(*library == cache.backend.library_path && k == key));
}

/// What a dry run answers.
pub(crate) fn plan_json(plan: &organize::OrganizePlan) -> Value {
    let moves: Vec<Value> = plan.books.iter().flat_map(|b| b.files.iter().map(move |f| json!({"book_id": b.book_id, "title": b.title, "format": f.format, "from": f.from, "to": f.to, "suffixed": b.suffixed}))).take(MAX_MOVES_SHOWN).collect();
    json!({
        "plan_fingerprint": plan.fingerprint,
        "template": plan.template,
        "books": plan.books.len(),
        "files": plan.file_count(),
        "conflicts": plan.conflicts(),
        "already_organized": plan.already_organized,
        "skipped": plan.skipped,
        // Books whose `data/` folder of attached files moves with them, and what
        // could not be handled (attachments left behind, paths too long).
        "data_folders": plan.books.iter().filter(|b| b.data_dir.is_some()).count(),
        "warnings": plan.warnings,
        "moves": moves,
    })
}

/// `POST /organize-library/{library_id}`.
pub async fn organize(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<OrganizeBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let template = body.template.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| lexmechanic_db::naming::load(&cache).combined());
    let scopes = [body.book_ids.is_some(), body.search.is_some(), body.path_prefix.is_some()].iter().filter(|b| **b).count();
    if scopes > 1 {
        return Err(ServerError::BadRequest("give one of book_ids, search or path_prefix (or none for every book)".into()));
    }
    if body.dry_run == false && body.plan_fingerprint.is_none() {
        return Err(ServerError::BadRequest("executing needs the plan_fingerprint of the dry run the user confirmed".into()));
    }

    // Applying a plan that was just previewed, for this same request, with nothing changed since: use it.
    let key = json!({"organize": template, "book_ids": body.book_ids, "search": body.search, "path_prefix": body.path_prefix}).to_string();
    let reused = match (body.dry_run, body.plan_fingerprint.as_deref()) {
        (false, Some(fp)) => {
            let (c, key, fp) = (cache.clone(), key.clone(), fp.to_string());
            tokio::task::spawn_blocking(move || recall_plan(&c, &key, &fp)).await.map_err(internal)?
        }
        _ => None,
    };
    let (plan_cache, plan_template, plan_key) = (cache.clone(), template.clone(), key.clone());
    let plan = if let Some(plan) = reused {
        plan
    } else {
        let dry_run = body.dry_run;
        tokio::task::spawn_blocking(move || -> Result<organize::OrganizePlan, ServerError> {
        let watermark = plan_watermark(&plan_cache);
        let scope = if let Some(ids) = body.book_ids {
            Scope::Ids(ids)
        } else if let Some(query) = body.search {
            Scope::Ids(lexmechanic_db::search::search(&plan_cache, &query).map_err(|e| ServerError::BadRequest(format!("{e:#}")))?)
        } else if let Some(prefix) = body.path_prefix {
            Scope::PathPrefix(prefix)
        } else {
            Scope::All
        };
        let plan = organize::plan(&plan_cache, &plan_template, &scope).map_err(internal)?;
        if dry_run {
            remember_plan(&plan_cache, &plan_key, watermark, &plan);
        }
        Ok(plan)
        })
        .await
        .map_err(internal)??
    };

    if body.dry_run {
        return Ok(Json(plan_json(&plan)).into_response());
    }

    if body.plan_fingerprint.as_deref() != Some(plan.fingerprint.as_str()) {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the plan is no longer what was previewed (the library or the selection changed); run the dry run again", "plan_fingerprint": plan.fingerprint}))).into_response());
    }
    if plan.books.is_empty() {
        return Ok(Json(json!({"error": "nothing to organize", "books": 0})).into_response());
    }
    forget_plan(&cache, &key);
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
    let runs = tokio::task::spawn_blocking(move || describe_runs(&library)).await.map_err(internal)?;
    Ok(Json(json!({"runs": runs})))
}

/// How many of the newest runs are opened to say what each one did. The dialog
/// shows a handful; the rest are listed bare rather than read from disk.
const DESCRIBED_RUNS: usize = 20;

/// The undoable runs, newest first. Each carries `template`: what the run was
/// asked to do, as the plan labelled it (a template, `move to Fiction` or
/// `rename folder Fiction to Novels`), so two runs of one book can be told apart.
fn describe_runs(library: &std::path::Path) -> Vec<Value> {
    organize::list_runs(library)
        .into_iter()
        .enumerate()
        .map(|(i, (run_id, created_at, books, undone))| {
            let template = if i < DESCRIBED_RUNS { organize::load_run(library, &run_id).map(|r| r.template).unwrap_or_default() } else { String::new() };
            json!({"run_id": run_id, "created_at": created_at, "books": books, "undone": undone, "template": template})
        })
        .collect()
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
    if !body.dry_run && body.plan_fingerprint.is_none() {
        return Err(ServerError::BadRequest("executing needs the plan_fingerprint of the dry run the user confirmed".into()));
    }
    let mut ids = body.book_ids.clone();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(ServerError::BadRequest("book_ids is empty".into()));
    }
    if ids.len() > MAX_MOVE_BOOKS {
        return Err(ServerError::BadRequest(format!("at most {MAX_MOVE_BOOKS} books can be moved in one request (got {})", ids.len())));
    }

    let key = json!({"move": folder, "book_ids": ids}).to_string();
    let reused = match (body.dry_run, body.plan_fingerprint.as_deref()) {
        (false, Some(fp)) => {
            let (c, key, fp) = (cache.clone(), key.clone(), fp.to_string());
            tokio::task::spawn_blocking(move || recall_plan(&c, &key, &fp)).await.map_err(internal)?
        }
        _ => None,
    };
    let (plan_cache, plan_folder, plan_key) = (cache.clone(), folder.clone(), key.clone());
    let plan = if let Some(plan) = reused {
        plan
    } else {
        let dry_run = body.dry_run;
        tokio::task::spawn_blocking(move || -> Result<organize::OrganizePlan, ServerError> {
        let watermark = plan_watermark(&plan_cache);
        for id in &ids {
            if plan_cache.field_for(*id, "path").map_err(internal)?.is_none() {
                return Err(ServerError::NotFound(format!("no book with id {id}")));
            }
        }
        let plan = organize::plan_to(&plan_cache, organize::Target::Folder(&plan_folder), &Scope::Ids(ids)).map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
        if dry_run {
            remember_plan(&plan_cache, &plan_key, watermark, &plan);
        }
        Ok(plan)
        })
        .await
        .map_err(internal)??
    };

    if body.dry_run {
        let mut out = plan_json(&plan);
        out["folder"] = json!(folder);
        return Ok(Json(out).into_response());
    }
    if body.plan_fingerprint.as_deref() != Some(plan.fingerprint.as_str()) {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the plan is no longer what was previewed (the library or the selection changed); run the dry run again", "plan_fingerprint": plan.fingerprint}))).into_response());
    }
    if plan.books.is_empty() {
        return Err(ServerError::BadRequest("nothing to move: the books are already in that folder, or cannot be moved (see skipped in the dry run)".into()));
    }
    forget_plan(&cache, &key);
    let params = JobParams { organize: Some(plan), ..Default::default() };
    Ok(started("organize", library_jobs::start_with(cache, JobKind::Organize, None, params)))
}
