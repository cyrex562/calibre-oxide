//! Real, **new** routes -- no upstream `calibre.srv` route exists for
//! fetching news/recipes either (same story as [`crate::lists`] and
//! [`crate::catalog`]: it's a desktop-GUI-only feature there). This
//! is the first HTTP-reachable use of the real, already-ported
//! `calibre_ebooks::web::feeds` pipeline (issue #81's cluster) --
//! confirmed via a full grep of `calibre_srv` finding zero prior
//! references to that module.
//!
//! # Scope: a real, generic feed reader, not a recipe catalog
//!
//! Real upstream ships ~1077 hand-written, site-specific `.recipe`
//! Python scripts (custom scraping/cleanup logic per news source).
//! Building or porting a catalog of those is real, separate,
//! substantial scope, not attempted here. What this route offers
//! instead is exactly [`calibre_ebooks::web::feeds::recipe::RecipeConfig`]'s
//! own generic path: a user-supplied title plus one or more plain
//! RSS/Atom feed URLs, using every one of `NewsRecipeHooks`/
//! `NewsRecipePostprocessHooks`'s real default implementations (both
//! traits' only non-defaulted item is `config()`) -- a real, working
//! "custom news source" matching what real upstream's own GUI offers
//! for a feed URL with no hand-written recipe, not a stub.
//!
//! # Pipeline
//!
//! `POST /news/fetch` queues a real background job (mirroring
//! [`crate::convert`]'s own job-registry pattern) that: builds a
//! [`RecipeConfig`] from the request, runs
//! [`calibre_ebooks::web::feeds::download::build_index`] (real
//! network fetches against the given feed URLs -- this route
//! genuinely needs internet access to do anything, unlike the rest of
//! this crate), converts the resulting OEB directory to a real EPUB
//! via [`calibre_ebooks::conversion::plumber::Plumber`] (the same
//! engine `convert.rs` already uses), and adds it to the library via
//! `Cache::add_book` on success -- publishing the same
//! [`crate::web_socket::ChangeEvent::BooksAdded`] event `cdb::add_book`
//! does, so the new issue shows up in a connected UI without a manual
//! refresh.
//!
//! `GET /news/status/{job_id}` polls it, matching `convert.rs`'s own
//! one-shot status contract (a job's status can only be read once).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_ebooks::scraper::browser::Browser;
use calibre_ebooks::web::feeds::download::build_index;
use calibre_ebooks::web::feeds::postprocess::NewsRecipePostprocessHooks;
use calibre_ebooks::web::feeds::recipe::{NewsRecipeHooks, RecipeConfig};

use crate::errors::ServerError;
use crate::jobs::{JobId, JobStatus};
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

struct GenericRecipe(RecipeConfig);

impl NewsRecipeHooks for GenericRecipe {
    fn config(&self) -> &RecipeConfig {
        &self.0
    }
}

impl NewsRecipePostprocessHooks for GenericRecipe {}

/// Lazily loads the system font database once per process -- matches
/// `calibre_ebooks::web::feeds::download`'s own test helper exactly
/// (`test_db`), needed by `build_index` for real default-cover/
/// masthead-image generation.
fn fontdb() -> &'static std::sync::Arc<fontdb::Database> {
    static DB: OnceLock<std::sync::Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
}

struct NewsJobMeta {
    title: String,
    output_path: PathBuf,
    tdir: PathBuf,
}

#[derive(Default)]
pub struct NewsJobRegistry {
    jobs: Mutex<HashMap<JobId, NewsJobMeta>>,
}

impl NewsJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, job_id: JobId, meta: NewsJobMeta) {
        self.jobs.lock().unwrap().insert(job_id, meta);
    }

    fn contains(&self, job_id: JobId) -> bool {
        self.jobs.lock().unwrap().contains_key(&job_id)
    }

    fn take(&self, job_id: JobId) -> Option<NewsJobMeta> {
        self.jobs.lock().unwrap().remove(&job_id)
    }
}

#[derive(Debug, Deserialize)]
pub struct FetchNewsBody {
    title: String,
    /// Plain feed URLs -- titles default to each feed's own (matching
    /// `RecipeConfig::feeds`'s own `None`-title convention).
    feeds: Vec<String>,
}

/// `POST /news/fetch`.
pub async fn fetch_news(State(state): State<AppState>, Json(body): Json<FetchNewsBody>) -> Result<Json<Value>, ServerError> {
    if body.feeds.is_empty() {
        return Err(ServerError::BadRequest("at least one feed URL is required".to_string()));
    }
    let title = if body.title.trim().is_empty() { "Custom News Source".to_string() } else { body.title.trim().to_string() };

    let tdir = tempfile::tempdir().map_err(|e| ServerError::InternalServerError(e.to_string()))?.keep();
    let output_dir = tdir.join("index");
    let output_epub = tdir.join("output.epub");

    let job_id = {
        let output_dir = output_dir.clone();
        let output_epub = output_epub.clone();
        let feeds = body.feeds.clone();
        let title_for_job = title.clone();
        state
            .jobs
            .start_job(move || async move {
                let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    let mut cfg = RecipeConfig { title: title_for_job, feeds: Some(feeds.into_iter().map(|u| (None, u)).collect()), ..Default::default() };
                    cfg.simultaneous_downloads = cfg.simultaneous_downloads.max(1);
                    let simultaneous_downloads = cfg.simultaneous_downloads;
                    let recipe = GenericRecipe(cfg);
                    let browser = Browser::new("", &[], true);

                    let index_html = build_index(&recipe, &browser, &output_dir, false, simultaneous_downloads, fontdb())?;
                    calibre_ebooks::conversion::plumber::Plumber::new(&index_html, &output_epub).run()?;
                    Ok(())
                })
                .await;
                match result {
                    Ok(Ok(())) => Ok("ok".to_string()),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(join_err) => Err(join_err.to_string()),
                }
            })
            .await
    };

    state.news_jobs.insert(job_id, NewsJobMeta { title, output_path: output_epub, tdir });
    Ok(Json(json!(job_id)))
}

/// `GET/POST /news/status/{job_id}`.
pub async fn news_status(State(state): State<AppState>, AxumPath(job_id): AxumPath<JobId>) -> Result<Json<Value>, ServerError> {
    if !state.news_jobs.contains(job_id) {
        return Err(ServerError::NotFound(format!("No job with id: {job_id}")));
    }

    let status = state.jobs.status(job_id).await;
    if matches!(status, JobStatus::Waiting | JobStatus::Running) {
        return Ok(Json(json!({"running": true})));
    }

    let Some(meta) = state.news_jobs.take(job_id) else {
        return Ok(Json(json!({"running": false, "ok": false, "error": "job status is no longer known"})));
    };
    let (ok, error) = match &status {
        JobStatus::Finished { .. } => (true, String::new()),
        JobStatus::Failed { error, .. } => (false, error.clone()),
        _ => (false, "job status is no longer known".to_string()),
    };

    let mut ans = json!({"running": false, "ok": ok, "error": error});
    if ok {
        let book_id = tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            let output_path = meta.output_path.clone();
            // Not calibre_ebooks::metadata::get_metadata: the built
            // EPUB carries no real `<dc:title>` (its own OEBBook came
            // from parsing generated HTML with no metadata of its
            // own), so format-sniffing would silently fall back to
            // MetaInformation::default()'s "Unknown" -- the recipe's
            // real title is already known here, use it directly
            // rather than trying to re-derive it from the file.
            let title = meta.title.clone();
            move || -> anyhow::Result<i32> {
                let meta_info = calibre_ebooks::metadata::MetaInformation { title, pubdate: Some(chrono::Utc::now()), ..Default::default() };
                cache.add_book(&output_path, &meta_info)
            }
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: vec![book_id] });
        ans["book_id"] = json!(book_id);
    }

    let _ = tokio::fs::remove_dir_all(&meta.tdir).await;
    Ok(Json(ans))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    /// A tiny multi-route single-threaded HTTP/1.1 test server --
    /// matches `calibre_ebooks::web::feeds::download`'s own private
    /// `TestSite` helper exactly (test helpers aren't shared across
    /// crates' private `#[cfg(test)]` blocks).
    struct TestSite {
        addr: std::net::SocketAddr,
    }

    impl TestSite {
        fn start(routes: HashMap<&'static str, (&'static str, &'static [u8])>) -> TestSite {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream: TcpStream = stream;
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut l = String::new();
                        if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                    }
                    match routes.get(path.as_str()) {
                        Some((ctype, body)) => {
                            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.write_all(body);
                        }
                        None => {
                            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                }
            });
            TestSite { addr }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }
    }

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

    async fn poll_until_done(router: &axum::Router, job_id: i64) -> serde_json::Value {
        for _ in 0..200 {
            let (status, body) = get_json(router, &format!("/news/status/{job_id}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["running"] == false {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("news job never finished within the polling budget");
    }

    fn rss_feed() -> &'static [u8] {
        br#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Test Feed</title><description>d</description>
<item><title>Article One</title><link>/article1</link><description>Summary one</description></item>
</channel></rss>"#
    }

    #[tokio::test]
    async fn fetch_news_downloads_a_real_feed_and_adds_it_as_a_real_book() {
        let mut routes = HashMap::new();
        routes.insert("/feed.xml", ("application/rss+xml", rss_feed()));
        routes.insert("/article1", ("text/html", b"<html><body><p>Full text of article one.</p></body></html>".as_slice()));
        let site = TestSite::start(routes);

        let (_dir, router) = test_app();
        let (status, job_id) = post_json(&router, "/news/fetch", serde_json::json!({"title": "My Weekly", "feeds": [site.url("/feed.xml")]})).await;
        assert_eq!(status, StatusCode::OK, "{job_id}");
        let job_id = job_id.as_i64().unwrap();

        let result = poll_until_done(&router, job_id).await;
        assert_eq!(result["ok"], true, "{result}");
        let book_id = result["book_id"].as_i64().expect("expected a real book_id");

        let (_, book) = get_json(&router, &format!("/ajax/book/{book_id}")).await;
        assert_eq!(book["title"], "My Weekly");
    }

    #[tokio::test]
    async fn fetch_news_rejects_an_empty_feed_list() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/news/fetch", serde_json::json!({"title": "x", "feeds": []})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn news_status_404s_for_an_unknown_job_id() {
        let (_dir, router) = test_app();
        let (status, _) = get_json(&router, "/news/status/999999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_feed_url_that_404s_degrades_to_a_real_empty_issue_rather_than_erroring() {
        // Found while writing this test: a feed URL that 404s does
        // NOT hit build_index's "No articles found, aborting" bail --
        // that only fires when the *feed list itself* ends up empty,
        // not when a listed feed's own fetch fails and yields zero
        // articles. calibre_ebooks::web::feeds::download's own real,
        // pre-existing behavior tolerates an unfetchable feed as an
        // empty one, so this route's job still reports success. Not a
        // bug in this route (or in that module) to fix -- documented
        // here so a future reader doesn't assume the opposite.
        let site = TestSite::start(HashMap::new());
        let (_dir, router) = test_app();
        let (status, job_id) = post_json(&router, "/news/fetch", serde_json::json!({"title": "Empty Issue", "feeds": [site.url("/missing.xml")]})).await;
        assert_eq!(status, StatusCode::OK);
        let job_id = job_id.as_i64().unwrap();

        let result = poll_until_done(&router, job_id).await;
        assert_eq!(result["ok"], true, "{result}");
        let book_id = result["book_id"].as_i64().expect("expected a real book_id even for an empty issue");
        let (_, book) = get_json(&router, &format!("/ajax/book/{book_id}")).await;
        assert_eq!(book["title"], "Empty Issue");
    }
}
