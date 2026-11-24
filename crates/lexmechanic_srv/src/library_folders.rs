//! The folder tree and the naming convention (tranche 19b).
//!
//! - `GET /library-folders/{library_id}` -> `{total, folders: [{path, books,
//!   books_here, empty}]}` (one query, no disk walk; counts include
//!   sub-folders). `POST` `{path}` makes an empty folder; `POST
//!   /library-folders/delete/{id}` `{path}` removes an empty one.
//! - `POST /rename-folder/{library_id}` `{from, to, dry_run?, plan_fingerprint?}`
//!   -- the dry run answers the organize plan; `dry_run: false` starts an
//!   `organize` job (undo with `POST /unorganize-library`). A `to` that exists,
//!   or inside `from`, is 400.
//! - `GET|PUT /naming/{library_id}` -- the convention `{folder_template,
//!   file_template}` (plus `combined` and the `defaults`).
//! - `POST /naming-preview/{library_id}` `{folder_template, file_template,
//!   book_ids?}` -> `{previews: [...]}` for up to 5 books (those given, else
//!   spread across the library).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::naming::{self, Naming};
use lexmechanic_db::{library_folders, organize};

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams};
use crate::organize_library::{internal, plan_json, started};
use crate::AppState;

fn cache(state: &AppState, id: &str) -> Result<std::sync::Arc<lexmechanic_db::cache::Cache>, ServerError> {
    state.cache_for(Some(id)).ok_or_else(|| ServerError::NotFound(format!("no library named {id:?}")))
}

/// `GET /library-folders/{library_id}`.
pub async fn list(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    let tree = tokio::task::spawn_blocking(move || library_folders::list(&cache)).await.map_err(internal)?.map_err(internal)?;
    Ok(Json(serde_json::to_value(tree).map_err(internal)?))
}

#[derive(Debug, Deserialize)]
pub struct PathBody {
    pub path: String,
}

/// `POST /library-folders/{library_id}`.
pub async fn create(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<PathBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    let path = tokio::task::spawn_blocking(move || library_folders::create_folder(&cache, &body.path)).await.map_err(internal)?.map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    Ok(Json(json!({"path": path})))
}

/// `POST /library-folders/delete/{library_id}`.
pub async fn delete(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<PathBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    tokio::task::spawn_blocking(move || library_folders::delete_empty_folder(&cache, &body.path)).await.map_err(internal)?.map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    pub from: String,
    pub to: String,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub plan_fingerprint: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `POST /rename-folder/{library_id}`.
pub async fn rename(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<RenameBody>) -> Result<Response, ServerError> {
    let cache = cache(&state, &id)?;
    let (c, from, to) = (cache.clone(), body.from.clone(), body.to.clone());
    let plan = tokio::task::spawn_blocking(move || library_folders::plan_rename(&c, &from, &to)).await.map_err(internal)?.map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    if body.dry_run {
        let mut out = plan_json(&plan);
        out["from"] = json!(body.from);
        out["to"] = json!(body.to);
        return Ok(Json(out).into_response());
    }
    if body.plan_fingerprint.as_deref() != Some(plan.fingerprint.as_str()) {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the plan is no longer what was previewed; run the dry run again", "plan_fingerprint": plan.fingerprint}))).into_response());
    }
    let (c, from, to) = (cache.clone(), body.from.clone(), body.to.clone());
    tokio::task::spawn_blocking(move || {
        let (from, to) = (organize::validate_folder(&from).unwrap_or(from), organize::validate_folder(&to).unwrap_or(to));
        library_folders::move_remembered(&c, &from, &to)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    if plan.books.is_empty() {
        return Ok(Json(json!({"books": 0, "moved_empty_folder": true})).into_response());
    }
    let params = JobParams { organize: Some(plan), ..Default::default() };
    Ok(started("organize", library_jobs::start_with(cache, JobKind::Organize, None, params)))
}

fn naming_json(n: &Naming) -> Value {
    json!({
        "folder_template": n.folder_template,
        "file_template": n.file_template,
        "combined": n.combined(),
        "defaults": {"folder_template": naming::DEFAULT_FOLDER_TEMPLATE, "file_template": naming::DEFAULT_FILE_TEMPLATE},
    })
}

/// `GET /naming/{library_id}`.
pub async fn naming_get(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    let n = tokio::task::spawn_blocking(move || naming::load(&cache)).await.map_err(internal)?;
    Ok(Json(naming_json(&n)))
}

/// `PUT /naming/{library_id}`.
pub async fn naming_put(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<Naming>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    let n = tokio::task::spawn_blocking(move || naming::save(&cache, &body).map(|_| naming::load(&cache))).await.map_err(internal)?.map_err(|e| ServerError::BadRequest(format!("{e:#}")))?;
    Ok(Json(naming_json(&n)))
}

#[derive(Debug, Deserialize)]
pub struct PreviewBody {
    #[serde(default)]
    pub folder_template: String,
    #[serde(default)]
    pub file_template: String,
    pub book_ids: Option<Vec<i32>>,
}

/// `POST /naming-preview/{library_id}`.
pub async fn naming_preview(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<PreviewBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache(&state, &id)?;
    let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        if body.file_template.trim().is_empty() {
            return Err("the file name template is required".into());
        }
        let ids = match body.book_ids.filter(|b| !b.is_empty()) {
            Some(mut ids) => {
                ids.truncate(5);
                ids
            }
            None => {
                let all = cache.all_book_ids().map_err(|e| e.to_string())?;
                let step = (all.len() / 5).max(1);
                all.into_iter().step_by(step).take(5).collect()
            }
        };
        let n = Naming { folder_template: body.folder_template, file_template: body.file_template };
        Ok(json!({"previews": naming::preview(&cache, &n, &ids), "combined": n.combined()}))
    })
    .await
    .map_err(internal)?
    .map_err(ServerError::BadRequest)?;
    Ok(Json(out))
}
