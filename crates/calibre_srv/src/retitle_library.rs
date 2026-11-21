//! Fix titles in one step ([`calibre_db::retitle`]).
//!
//! - `POST /retitle-library/{library_id}` `{dry_run?, scope, rename_template?}`
//!   -- `dry_run` defaults to **true** and answers `{rename_template, total,
//!   truncated, rows}` without touching anything. `scope` is `{"book_ids":
//!   [..]}` (every requested book is returned; ones that need no fix are
//!   `reason: "ok"`) or `{"all": true}` (only books that need fixing). A row
//!   is `{book_id, current_title, suggested_title, reason, current_file,
//!   formats, proposed_file}`; `reason` is `junk`, `template_leftover`, `ok`
//!   or `no_suggestion` (empty `suggested_title`: the user types one).
//!   `proposed_file` is the stem the template gives *with the suggested title*.
//!   At most 5000 rows; `total` is the number needing a fix, `truncated` says
//!   `rows` is shorter.
//!
//!   `dry_run: false` takes `{items: [{book_id, expected_title, title}],
//!   embed, rename_files, rename_template}` and starts a `retitle` job (202,
//!   progress and the report at `/library-jobs/{id}`; cancel there too).
//! - `POST /unretitle-library/{library_id}` `{run_id}` -- an `undo_retitle` job.
//! - `GET /retitle-runs/{library_id}` -- recorded runs, newest first.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::retitle::{self, ApplyOptions, Item, Scope};

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, RetitleJob, Started};
use crate::AppState;

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub struct ScopeBody {
    pub book_ids: Option<Vec<i32>>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize)]
pub struct RetitleBody {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub scope: Option<ScopeBody>,
    pub rename_template: Option<String>,
    pub items: Option<Vec<Item>>,
    #[serde(default)]
    pub embed: bool,
    #[serde(default)]
    pub rename_files: bool,
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

/// `POST /retitle-library/{library_id}`.
pub async fn retitle(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<RetitleBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let template = body.rename_template.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| retitle::DEFAULT_RENAME_TEMPLATE.to_string());

    if body.dry_run {
        let scope = match body.scope {
            Some(ScopeBody { book_ids: Some(_), all: true }) => return Err(ServerError::BadRequest("give book_ids or all, not both".into())),
            Some(ScopeBody { book_ids: Some(ids), .. }) => Scope::Ids(ids),
            Some(ScopeBody { all: true, .. }) => Scope::All,
            _ => return Err(ServerError::BadRequest("a scope is required: {\"book_ids\": [..]} or {\"all\": true}".into())),
        };
        let t = template.clone();
        let suggestions = tokio::task::spawn_blocking(move || retitle::suggest(&cache, &scope, &t)).await.map_err(internal)?.map_err(internal)?;
        return Ok(Json(json!({"rename_template": template, "total": suggestions.total, "truncated": suggestions.truncated, "rows": suggestions.rows})).into_response());
    }

    let items = body.items.unwrap_or_default();
    if items.is_empty() {
        return Err(ServerError::BadRequest("items is empty: nothing to retitle".into()));
    }
    let job = RetitleJob { items, options: ApplyOptions { embed: body.embed, rename_files: body.rename_files, rename_template: template } };
    let params = JobParams { retitle: Some(job), ..Default::default() };
    Ok(started("retitle", library_jobs::start_with(cache, JobKind::Retitle, None, params)))
}

/// `POST /unretitle-library/{library_id}`.
pub async fn unretitle(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<UndoBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let library = cache.backend.library_path.clone();
    let run_id = body.run_id.clone();
    let run = tokio::task::spawn_blocking(move || retitle::load_run(&library, &run_id)).await.map_err(internal)?.map_err(|e| ServerError::NotFound(format!("{e:#}")))?;
    if run.undone_at.is_some() {
        return Err(ServerError::BadRequest(format!("run {} was already undone", run.run_id)));
    }
    let params = JobParams { undo_run: Some(body.run_id), ..Default::default() };
    Ok(started("undo_retitle", library_jobs::start_with(cache, JobKind::UndoRetitle, None, params)))
}

/// `GET /retitle-runs/{library_id}`.
pub async fn runs(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let library = cache.backend.library_path.clone();
    let runs = tokio::task::spawn_blocking(move || retitle::list_runs(&library)).await.map_err(internal)?;
    Ok(Json(json!({"runs": runs.into_iter().map(|(run_id, created_at, books, undone)| json!({"run_id": run_id, "created_at": created_at, "books": books, "undone": undone})).collect::<Vec<_>>()})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use tower::ServiceExt;

    fn app(dir: &std::path::Path) -> (std::sync::Arc<Cache>, axum::Router) {
        let cache = std::sync::Arc::new(Cache::new(dir).unwrap());
        let state = crate::AppState {
            libraries: None,
            cache: cache.clone(),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (cache, crate::test_router(state))
    }

    fn add(dir: &std::path::Path, cache: &Cache, stem: &str, title: &str) -> i32 {
        std::fs::write(dir.join(format!("{stem}.pdf")), format!("%PDF-1.4 {stem}")).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        cache.register_book_in_place(&format!("{stem}.pdf"), &meta).unwrap()
    }

    async fn call(router: &axum::Router, method: &str, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn get(router: &axum::Router, uri: &str) -> serde_json::Value {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    async fn finished(router: &axum::Router, started: &serde_json::Value) -> serde_json::Value {
        let id = started["job_id"].as_u64().unwrap_or_else(|| panic!("no job id in {started}"));
        for _ in 0..600 {
            let job = get(router, &format!("/library-jobs/{id}")).await;
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    #[tokio::test]
    async fn a_dry_run_suggests_for_every_scope_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let junk = add(dir.path(), &cache, "Dragon_Lore", "Untitled");
        let fine = add(dir.path(), &cache, "fine", "A Fine Title");

        let (status, all) = call(&router, "POST", "/retitle-library/default", serde_json::json!({"scope": {"all": true}, "rename_template": "Fixed {title}"})).await;
        assert_eq!(status, StatusCode::OK, "{all}");
        assert_eq!(all["total"], 1, "{all}");
        assert_eq!(all["truncated"], false);
        assert_eq!(all["rows"].as_array().unwrap().len(), 1, "an all scope lists only what needs fixing: {all}");
        let row = &all["rows"][0];
        assert_eq!((row["book_id"].as_i64().unwrap() as i32, row["reason"].as_str().unwrap()), (junk, "junk"));
        assert_eq!(row["suggested_title"], "Dragon Lore");
        assert_eq!(row["current_title"], "Untitled");
        assert_eq!(row["current_file"], "Dragon_Lore");
        assert_eq!(row["proposed_file"], "Fixed Dragon Lore");
        assert_eq!(row["formats"], serde_json::json!(["PDF"]));

        let (_, picked) = call(&router, "POST", "/retitle-library/default", serde_json::json!({"scope": {"book_ids": [fine, junk]}})).await;
        assert_eq!(picked["rows"].as_array().unwrap().len(), 2, "{picked}");
        let ok = picked["rows"].as_array().unwrap().iter().find(|r| r["book_id"] == fine).unwrap();
        assert_eq!((ok["reason"].as_str().unwrap(), ok["suggested_title"].as_str().unwrap()), ("ok", "A Fine Title"));

        assert_eq!(cache.field_for(junk, "title").unwrap().unwrap(), "Untitled", "a dry run changes nothing");
        assert!(dir.path().join("Dragon_Lore.pdf").is_file());
    }

    #[tokio::test]
    async fn a_scope_is_required_and_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let (_cache, router) = app(dir.path());
        let (status, _) = call(&router, "POST", "/retitle-library/default", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&router, "POST", "/retitle-library/default", serde_json::json!({"scope": {"all": true, "book_ids": [1]}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&router, "POST", "/retitle-library/default", serde_json::json!({"dry_run": false, "items": []})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "nothing to retitle");
    }

    #[tokio::test]
    async fn applying_is_a_job_that_sets_renames_and_skips_stale_books_then_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let a = add(dir.path(), &cache, "Dragon_Lore", "Untitled");
        let b = add(dir.path(), &cache, "other", "Untitled");

        let body = serde_json::json!({
            "dry_run": false, "embed": false, "rename_files": true, "rename_template": "{title}",
            "items": [
                {"book_id": a, "expected_title": "Untitled", "title": "Dragon Lore"},
                {"book_id": b, "expected_title": "Not What Is Stored", "title": "Other"},
            ],
        });
        let (status, started) = call(&router, "POST", "/retitle-library/default", body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        assert_eq!(started["kind"], "retitle");
        let job = finished(&router, &started).await;
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["kind"], "retitle");
        assert_eq!((job["report"]["retitled"].as_i64(), job["report"]["renamed"].as_i64()), (Some(1), Some(1)), "{job}");
        assert_eq!(job["report"]["skipped"][0]["reason"], "changed_since_preview", "{job}");
        assert_eq!(cache.field_for(a, "title").unwrap().unwrap(), "Dragon Lore");
        assert_eq!(cache.field_for(b, "title").unwrap().unwrap(), "Untitled");
        assert!(dir.path().join("Dragon Lore.pdf").is_file());

        let runs = get(&router, "/retitle-runs/default").await;
        assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "{runs}");
        assert_eq!((runs["runs"][0]["books"].as_i64(), runs["runs"][0]["undone"].as_bool()), (Some(1), Some(false)));
        let run_id = runs["runs"][0]["run_id"].as_str().unwrap().to_string();

        let (status, started) = call(&router, "POST", "/unretitle-library/default", serde_json::json!({"run_id": run_id})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let job = finished(&router, &started).await;
        assert_eq!((job["report"]["titles_restored"].as_i64(), job["report"]["files_restored"].as_i64()), (Some(1), Some(1)), "{job}");
        assert_eq!(cache.field_for(a, "title").unwrap().unwrap(), "Untitled");
        assert!(dir.path().join("Dragon_Lore.pdf").is_file());

        let (status, again) = call(&router, "POST", "/unretitle-library/default", serde_json::json!({"run_id": run_id})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a double undo is refused: {again}");
        let (status, _) = call(&router, "POST", "/unretitle-library/default", serde_json::json!({"run_id": "ret-nope"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let runs = get(&router, "/retitle-runs/default").await;
        assert_eq!(runs["runs"][0]["undone"], true);
    }
}

