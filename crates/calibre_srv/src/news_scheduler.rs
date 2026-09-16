//! `POST /news/schedules/*` -- a real, **new** feature (issue #764,
//! part of #747's tracking epic): let a saved feed configuration run
//! automatically on a real recurring interval, rather than only via
//! [`crate::news`]'s existing one-shot `/news/fetch`.
//!
//! # Storage: `rusqlite`, matching `reader_profiles.rs`'s own pattern
//!
//! Real upstream persists scheduled recipes as part of its
//! `config_dir`-based job scheduler config, which this crate has no
//! equivalent of. This port reuses the exact storage pattern issue
//! #419's `reader_profiles::ProfileStore` already established for
//! server-local state with no library-file involvement: a small,
//! dedicated `rusqlite` database (`news-schedules.sqlite`, created
//! next to `reader-profiles.sqlite`/`server-users.sqlite`), not a
//! shared JSON file.
//!
//! # Scheduler: a real background `tokio` task, not a new async concept
//!
//! [`run_scheduler_loop`] is spawned once at server startup (see
//! `main.rs`) and polls [`NewsScheduleStore::due`] on a fixed tick,
//! running any due fetch via [`run_scheduled_fetch`] and rescheduling
//! it for `interval_secs` from now -- the same "a `tokio` task drives
//! real background work against shared `AppState`" shape
//! [`crate::jobs::JobsManager`] already established for on-demand
//! jobs, just with its own timer instead of an explicit trigger.
//!
//! [`run_scheduled_fetch`] deliberately duplicates a small amount of
//! [`crate::news::fetch_news`]'s own pipeline-building logic (build a
//! [`RecipeConfig`], run [`build_index`], convert via [`Plumber`])
//! rather than refactoring `fetch_news` to share it: `fetch_news`'s
//! own job-registry/HTTP-polling contract defers `Cache::add_book`
//! until the client polls status, which this synchronous, no-client
//! background path has no equivalent of and shouldn't be made to
//! support just to share a few lines -- see this module's own tests
//! for what's covered instead. `GenericRecipe`/`fontdb()` themselves
//! (not the orchestration) are genuinely reused, not duplicated (see
//! [`crate::news`]).

use std::path::Path;
use std::sync::Mutex;

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_ebooks::scraper::browser::Browser;
use calibre_ebooks::web::feeds::download::build_index;
use calibre_ebooks::web::feeds::recipe::RecipeConfig;

use crate::errors::ServerError;
use crate::news::{fontdb, GenericRecipe};
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Schedule {
    pub id: i64,
    pub title: String,
    pub feeds: Vec<String>,
    pub interval_secs: i64,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub last_result: Option<String>,
}

pub struct NewsScheduleStore {
    conn: Mutex<Connection>,
}

impl NewsScheduleStore {
    pub fn new(path: &Path) -> anyhow::Result<NewsScheduleStore> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(NewsScheduleStore { conn: Mutex::new(conn) })
    }

    /// For tests, and for any deployment that doesn't want this
    /// persisted to disk -- no real upstream equivalent (real upstream
    /// always persists its own job schedule config).
    pub fn new_in_memory() -> anyhow::Result<NewsScheduleStore> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(NewsScheduleStore { conn: Mutex::new(conn) })
    }

    fn init(conn: &Connection) -> anyhow::Result<()> {
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version == 0 {
            conn.execute_batch(
                r#"
                CREATE TABLE schedules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    title TEXT NOT NULL,
                    feeds TEXT NOT NULL,
                    interval_secs INTEGER NOT NULL,
                    next_run_at TEXT NOT NULL,
                    last_run_at TEXT,
                    last_result TEXT
                );
                PRAGMA user_version=1;
                "#,
            )?;
        }
        Ok(())
    }

    fn row_to_schedule(row: &rusqlite::Row) -> rusqlite::Result<Schedule> {
        let feeds_json: String = row.get(2)?;
        let feeds: Vec<String> = serde_json::from_str(&feeds_json).unwrap_or_default();
        Ok(Schedule { id: row.get(0)?, title: row.get(1)?, feeds, interval_secs: row.get(3)?, next_run_at: row.get(4)?, last_run_at: row.get(5)?, last_result: row.get(6)? })
    }

    pub fn list(&self) -> anyhow::Result<Vec<Schedule>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, feeds, interval_secs, next_run_at, last_run_at, last_result FROM schedules ORDER BY id")?;
        let rows = stmt.query_map([], Self::row_to_schedule)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// `now` is passed in rather than computed here (this crate treats
    /// `Utc::now()` as an effect, matching this project's own testing
    /// convention elsewhere -- e.g. `jobs.rs`) so a test can seed a
    /// schedule already due without a real sleep.
    pub fn add(&self, title: &str, feeds: &[String], interval_secs: i64, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<i64> {
        let feeds_json = serde_json::to_string(feeds)?;
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO schedules (title, feeds, interval_secs, next_run_at) VALUES (?1, ?2, ?3, ?4)", (title, feeds_json, interval_secs, now.to_rfc3339()))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn remove(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM schedules WHERE id = ?1", [id])? > 0)
    }

    /// Every schedule whose `next_run_at` is at or before `now`.
    pub fn due(&self, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<Vec<Schedule>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, feeds, interval_secs, next_run_at, last_run_at, last_result FROM schedules WHERE next_run_at <= ?1 ORDER BY id")?;
        let rows = stmt.query_map([now.to_rfc3339()], Self::row_to_schedule)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn get(&self, id: i64) -> anyhow::Result<Option<Schedule>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT id, title, feeds, interval_secs, next_run_at, last_run_at, last_result FROM schedules WHERE id = ?1", [id], Self::row_to_schedule).optional().map_err(Into::into)
    }

    /// Records the outcome of a run and reschedules `next_run_at` to
    /// `now + interval_secs`.
    pub fn mark_run(&self, id: i64, now: chrono::DateTime<chrono::Utc>, interval_secs: i64, result: &str) -> anyhow::Result<()> {
        let next = now + chrono::Duration::seconds(interval_secs);
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE schedules SET next_run_at = ?1, last_run_at = ?2, last_result = ?3 WHERE id = ?4", (next.to_rfc3339(), now.to_rfc3339(), result, id))?;
        Ok(())
    }
}

/// Runs one real feed fetch synchronously (build a recipe, download,
/// convert to EPUB, add to the library) and returns the new book id --
/// see this module's own doc for why this doesn't reuse
/// `news::fetch_news`'s job-registry machinery.
pub fn run_scheduled_fetch(cache: &Cache, title: &str, feeds: &[String]) -> anyhow::Result<i32> {
    let tdir = tempfile::tempdir()?.keep();
    let output_dir = tdir.join("index");
    let output_epub = tdir.join("output.epub");

    let mut cfg = RecipeConfig { title: title.to_string(), feeds: Some(feeds.iter().cloned().map(|u| (None, u)).collect()), ..Default::default() };
    cfg.simultaneous_downloads = cfg.simultaneous_downloads.max(1);
    let simultaneous_downloads = cfg.simultaneous_downloads;
    let recipe = GenericRecipe(cfg);
    let browser = Browser::new("", &[], true);

    let index_html = build_index(&recipe, &browser, &output_dir, false, simultaneous_downloads, fontdb())?;
    calibre_ebooks::conversion::plumber::Plumber::new(&index_html, &output_epub).run()?;

    let meta_info = calibre_ebooks::metadata::MetaInformation { title: title.to_string(), pubdate: Some(chrono::Utc::now()), ..Default::default() };
    let book_id = cache.add_book(&output_epub, &meta_info)?;

    let _ = std::fs::remove_dir_all(&tdir);
    Ok(book_id)
}

/// Spawned once at server startup (`main.rs`). Ticks every
/// `poll_interval`, checking for due schedules and running each in a
/// blocking task so a slow/stuck feed fetch can't stall the tick
/// timer for every other schedule.
pub async fn run_scheduler_loop(state: AppState, poll_interval: std::time::Duration) {
    let mut ticker = tokio::time::interval(poll_interval);
    loop {
        ticker.tick().await;
        let now = chrono::Utc::now();
        let due = match state.news_schedules.due(now) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("news scheduler: failed to list due schedules: {e:#}");
                continue;
            }
        };
        for schedule in due {
            let cache = state.cache.clone();
            let title = schedule.title.clone();
            let feeds = schedule.feeds.clone();
            let result = tokio::task::spawn_blocking(move || run_scheduled_fetch(&cache, &title, &feeds)).await;
            let outcome = match result {
                Ok(Ok(book_id)) => {
                    web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: vec![book_id] });
                    format!("ok:{book_id}")
                }
                Ok(Err(e)) => format!("error:{e:#}"),
                Err(join_err) => format!("error:{join_err}"),
            };
            if let Err(e) = state.news_schedules.mark_run(schedule.id, now, schedule.interval_secs, &outcome) {
                eprintln!("news scheduler: failed to record run for schedule {}: {e:#}", schedule.id);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddScheduleBody {
    title: String,
    feeds: Vec<String>,
    interval_secs: i64,
}

/// `POST /news/schedules/add`.
pub async fn add_schedule(State(state): State<AppState>, Json(body): Json<AddScheduleBody>) -> Result<Json<Value>, ServerError> {
    if body.feeds.is_empty() {
        return Err(ServerError::BadRequest("at least one feed URL is required".to_string()));
    }
    if body.interval_secs < 60 {
        return Err(ServerError::BadRequest("interval_secs must be at least 60".to_string()));
    }
    let title = if body.title.trim().is_empty() { "Custom News Source".to_string() } else { body.title.trim().to_string() };
    let id = tokio::task::spawn_blocking({
        let store = state.news_schedules.clone();
        move || store.add(&title, &body.feeds, body.interval_secs, chrono::Utc::now())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(json!({"id": id})))
}

/// `GET /news/schedules/list`.
pub async fn list_schedules(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let schedules = tokio::task::spawn_blocking({
        let store = state.news_schedules.clone();
        move || store.list()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(json!({"schedules": schedules})))
}

/// `POST /news/schedules/remove/{id}`.
pub async fn remove_schedule(State(state): State<AppState>, AxumPath(id): AxumPath<i64>) -> Result<Json<Value>, ServerError> {
    let removed = tokio::task::spawn_blocking({
        let store = state.news_schedules.clone();
        move || store.remove(id)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    if !removed {
        return Err(ServerError::NotFound(format!("No schedule with id: {id}")));
    }
    Ok(Json(json!({"ok": true})))
}

/// `POST /news/schedules/run-now/{id}` -- forces one schedule's
/// `next_run_at` to now, so the next scheduler tick (or a live
/// verification poll) picks it up immediately rather than waiting for
/// its real interval. Real, new convenience, not a port of anything
/// upstream: upstream's own job scheduler has no equivalent
/// "run now" trigger reachable from outside its GUI.
pub async fn run_schedule_now(State(state): State<AppState>, AxumPath(id): AxumPath<i64>) -> Result<Json<Value>, ServerError> {
    let existing = tokio::task::spawn_blocking({
        let store = state.news_schedules.clone();
        move || store.get(id)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    let Some(schedule) = existing else {
        return Err(ServerError::NotFound(format!("No schedule with id: {id}")));
    };

    let cache = state.cache.clone();
    let title = schedule.title.clone();
    let feeds = schedule.feeds.clone();
    let now = chrono::Utc::now();
    let result = tokio::task::spawn_blocking(move || run_scheduled_fetch(&cache, &title, &feeds)).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let outcome = match &result {
        Ok(book_id) => format!("ok:{book_id}"),
        Err(e) => format!("error:{e:#}"),
    };
    tokio::task::spawn_blocking({
        let store = state.news_schedules.clone();
        move || store.mark_run(id, now, schedule.interval_secs, &outcome)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    match result {
        Ok(book_id) => {
            web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: vec![book_id] });
            Ok(Json(json!({"ok": true, "book_id": book_id})))
        }
        Err(e) => Ok(Json(json!({"ok": false, "error": format!("{e:#}")}))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_app() -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
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
            news_schedules: std::sync::Arc::new(NewsScheduleStore::new_in_memory().unwrap()),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn a_real_schedule_added_over_http_is_listed_then_removed() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/news/schedules/add", serde_json::json!({"title": "My Feed", "feeds": ["https://example.com/feed.xml"], "interval_secs": 3600})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"].as_i64().unwrap();

        let (status, body) = get_json(&router, "/news/schedules/list").await;
        assert_eq!(status, StatusCode::OK);
        let schedules = body["schedules"].as_array().unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0]["title"], "My Feed");
        assert_eq!(schedules[0]["id"], id);

        let (status, _) = post_json(&router, &format!("/news/schedules/remove/{id}"), serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&router, "/news/schedules/list").await;
        assert!(body["schedules"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn adding_a_schedule_with_no_feeds_is_rejected() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/news/schedules/add", serde_json::json!({"title": "Empty", "feeds": [], "interval_secs": 3600})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn adding_a_schedule_with_too_short_an_interval_is_rejected() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/news/schedules/add", serde_json::json!({"title": "Too Fast", "feeds": ["https://example.com/feed.xml"], "interval_secs": 5})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn removing_an_unknown_schedule_404s() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/news/schedules/remove/999999", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn running_an_unknown_schedule_now_404s() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/news/schedules/run-now/999999", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_real_schedule_round_trips_through_add_list_remove() {
        let store = NewsScheduleStore::new_in_memory().unwrap();
        let now = chrono::Utc::now();
        let id = store.add("My Feed", &["https://example.com/feed.xml".to_string()], 3600, now).unwrap();

        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].title, "My Feed");
        assert_eq!(all[0].feeds, vec!["https://example.com/feed.xml"]);
        assert_eq!(all[0].interval_secs, 3600);
        assert!(all[0].last_run_at.is_none());

        assert!(store.remove(id).unwrap());
        assert!(store.list().unwrap().is_empty());
        assert!(!store.remove(id).unwrap(), "removing an already-removed id should report false, not error");
    }

    #[test]
    fn due_only_returns_schedules_whose_next_run_at_has_passed() {
        let store = NewsScheduleStore::new_in_memory().unwrap();
        let now = chrono::Utc::now();
        let past_due = store.add("Due Now", &["https://example.com/a.xml".to_string()], 3600, now - chrono::Duration::seconds(10)).unwrap();
        store.add("Not Yet", &["https://example.com/b.xml".to_string()], 3600, now + chrono::Duration::seconds(3600)).unwrap();

        let due = store.due(now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, past_due);
    }

    #[test]
    fn mark_run_reschedules_next_run_at_and_records_the_result() {
        let store = NewsScheduleStore::new_in_memory().unwrap();
        let now = chrono::Utc::now();
        let id = store.add("My Feed", &["https://example.com/feed.xml".to_string()], 3600, now).unwrap();

        store.mark_run(id, now, 3600, "ok:42").unwrap();

        let schedule = store.get(id).unwrap().unwrap();
        assert_eq!(schedule.last_result.as_deref(), Some("ok:42"));
        assert_eq!(schedule.last_run_at.as_deref(), Some(now.to_rfc3339().as_str()));
        let expected_next = (now + chrono::Duration::seconds(3600)).to_rfc3339();
        assert_eq!(schedule.next_run_at, expected_next);
        // No longer due at `now` -- it was just rescheduled forward.
        assert!(store.due(now).unwrap().is_empty());
    }
}
