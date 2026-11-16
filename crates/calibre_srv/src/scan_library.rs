//! `POST /scan-library/{library_id}` -- reads the library folder and
//! brings the index up to date with it.
//!
//! The counterpart to [`crate::check_library`], which only *reports*.
//! This one writes: files nothing claims become books, and files the
//! user moved inside their own folder have their records corrected.
//! Nothing on disk is touched.
//!
//! Part of the tracked-in-place model (#889), where the folder is the
//! library and this route is how the folder's current contents become
//! the library's current contents.
//!
//! # Who calls it
//!
//! Not "when a library is opened", which an earlier version of this
//! comment claimed and the code never did: opening a folder creates the
//! database and does not look at the files in it. The callers are Check
//! Library's "add the untracked files" action, and the desktop app's
//! watcher when the folder it watches is inside the library.
//!
//! # A job, not a request
//!
//! A scan of a share with hundreds of thousands of files runs for a long
//! time, so the route starts a job ([`crate::library_jobs`]) and answers
//! `202` with its id; progress and cancel are on `/library-jobs/{id}`.
//! `?wait=true` holds the response until the job ends and answers with the
//! final report -- the shape this route had when it was synchronous, for
//! callers that want exactly that (the desktop watcher, tests).
//!
//! The job has two stages. The first reads each new file once, for its
//! content hash, and registers it as a book titled from its filename --
//! books appear as batches commit. The second reads embedded metadata and
//! renders covers, filling in only what the user has not already changed.

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::errors::ServerError;
use crate::library_jobs::{respond, JobKind, StartQuery};
use crate::AppState;

/// `POST /scan-library/{library_id}`.
pub async fn scan(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    Ok(respond(cache, JobKind::Scan, &query).await)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(
                crate::reader_profiles::ProfileStore::new_in_memory().unwrap(),
            ),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(
                4,
                std::time::Duration::from_secs(3600),
            )),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(
                crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap(),
            ),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(
                calibre_customize::registry::PluginRegistry::new(),
            )),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, value)
    }

    /// Past the scanner's settle window, which exists so a file still
    /// being copied is not indexed half-written.
    fn make_settled(path: &std::path::Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(old)).unwrap();
    }

    #[tokio::test]
    async fn an_empty_library_scans_to_nothing() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["added"], serde_json::json!([]));
        assert_eq!(body["relocated"], 0);
        assert_eq!(body["conclusive"], true);
    }

    /// The headline: a file dropped into the folder becomes a book,
    /// and stays exactly where it was put.
    #[tokio::test]
    async fn a_dropped_file_becomes_a_book_without_moving() {
        let (dir, router) = test_app();
        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        make_settled(&book);

        let (status, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["added"].as_array().unwrap().len(), 1, "{body}");
        assert!(book.exists(), "the scan must not move the file it indexed");
    }

    #[tokio::test]
    async fn scanning_twice_does_not_add_the_same_file_again() {
        let (dir, router) = test_app();
        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        make_settled(&book);

        post_json(&router, "/scan-library/default?wait=true").await;
        let (_, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(body["already_known"], 1, "{body}");
    }

    /// A file still being written is reported, not indexed.
    #[tokio::test]
    async fn a_settling_file_is_reported_rather_than_indexed() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("half-copied.pdf"), b"%PDF").unwrap();

        let (_, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(
            body["settling"],
            serde_json::json!(["half-copied.pdf"]),
            "{body}"
        );
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// Polls a job until it is no longer running.
    async fn wait_for_job(router: &axum::Router, id: u64) -> serde_json::Value {
        for _ in 0..600 {
            let (_, job) = get_json(router, &format!("/library-jobs/{id}")).await;
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    /// Without `?wait`, the route starts a job and answers at once with
    /// its id; the status route then reports progress and the report.
    #[tokio::test]
    async fn a_scan_is_a_job_with_status_and_a_final_report() {
        let (dir, router) = test_app();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            let book = dir.path().join(name);
            std::fs::write(&book, format!("%PDF-1.4 {name}")).unwrap();
            make_settled(&book);
        }

        let (status, started) = post_json(&router, "/scan-library/default").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let id = started["job_id"].as_u64().expect("a job id");

        let job = wait_for_job(&router, id).await;
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["kind"], "scan");
        assert_eq!(job["report"]["added"].as_array().unwrap().len(), 3, "{job}");
        assert_eq!(job["progress"]["added"], 3, "{job}");
        assert_eq!(job["progress"]["hashed"], 3, "{job}");
        assert_eq!(job["progress"]["errors"], 0, "{job}");
        assert_eq!(job["report"]["enrichment"]["remaining"], 0, "{job}");
    }

    #[tokio::test]
    async fn cancelling_a_finished_or_unknown_job_is_harmless() {
        let (_dir, router) = test_app();
        let (_, started) = post_json(&router, "/scan-library/default").await;
        let id = started["job_id"].as_u64().unwrap();
        wait_for_job(&router, id).await;

        let (status, job) = post_json(&router, &format!("/library-jobs/{id}/cancel")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(job["state"], "finished", "cancel must not rewrite a finished job");

        let (status, _) = post_json(&router, "/library-jobs/999999999/cancel").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The backfill is a job too, and it hashes files a library opened
    /// from real calibre never had hashes for.
    #[tokio::test]
    async fn the_hash_backfill_is_a_job_that_records_missing_hashes() {
        let (dir, router) = test_app();
        let book = dir.path().join("a.pdf");
        std::fs::write(&book, b"%PDF-1.4 a").unwrap();
        make_settled(&book);
        post_json(&router, "/scan-library/default?wait=true").await;

        let (status, body) = post_json(&router, "/hash-backfill/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // The scan already hashed it, so there is nothing left to do.
        assert_eq!(body["total"], 0, "{body}");
        assert_eq!(body["state"], "finished");
    }

    #[tokio::test]
    async fn an_unknown_library_is_a_404() {
        // Must use multi-library mode: in single-library mode
        // (`AppState::libraries == None`), `cache_for` always falls
        // back to the default library regardless of the requested
        // name -- an unknown name can only actually 404 once a real
        // `LibraryBroker` is involved (matches this project's own
        // established test pattern, e.g. cdb.rs).
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        Cache::new(src_dir.path()).unwrap();
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(
            crate::library_broker::LibraryBroker::new(&[
                src_dir.path().to_path_buf(),
                dest_dir.path().to_path_buf(),
            ])
            .unwrap(),
        );
        let default_cache = broker.get(None).unwrap();
        let state = crate::AppState {
            libraries: Some(broker),
            cache: default_cache,
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(
                crate::reader_profiles::ProfileStore::new_in_memory().unwrap(),
            ),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(
                4,
                std::time::Duration::from_secs(3600),
            )),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(
                crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap(),
            ),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(
                calibre_customize::registry::PluginRegistry::new(),
            )),
        };
        let router = crate::test_router(state);

        let (status, _) = post_json(&router, "/scan-library/no-such-library").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
