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
//! the library's current contents. Called when a library is opened, on
//! an explicit refresh, and by the desktop app's watcher when the folder
//! it watches is inside the library.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

/// `POST /scan-library/{library_id}`.
pub async fn scan(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let rescan = calibre_db::scan::rescan(&cache, std::time::SystemTime::now())?;
        Ok(json!({
            "added": rescan.indexed.added,
            "already_known": rescan.indexed.already_known,
            // Books whose recorded path was corrected. Reported so a
            // refresh can say what it did, but not as a problem: moving
            // a file inside your own folder is allowed.
            "relocated": rescan.relocated,
            // Skipped because the user removed the book and kept the
            // file (#896). Surfaced rather than omitted -- a scan that
            // silently refuses to add a file the user can see is
            // indistinguishable from a broken one.
            "ignored": rescan.indexed.ignored,
            // Still being written. Not an error: the next scan gets them.
            "settling": rescan.scanned.settling,
            // Cloud-sync placeholders. Reading one would download it.
            "offline": rescan.scanned.offline,
            "nested_libraries": rescan.scanned.nested_libraries,
            "failed": rescan.indexed.failed.iter().map(|(path, why)| json!({"path": path, "error": why})).collect::<Vec<_>>(),
            // False means a directory could not be read, so the folder
            // was only partly seen. What was added is still correct;
            // what is absent is unknown. The UI has to say so rather
            // than implying the library is now in sync.
            "conclusive": rescan.scanned.is_complete(),
            "unreadable": rescan.scanned.unreadable,
        }))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(result))
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
        let (status, body) = post_json(&router, "/scan-library/default").await;
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

        let (status, body) = post_json(&router, "/scan-library/default").await;
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

        post_json(&router, "/scan-library/default").await;
        let (_, body) = post_json(&router, "/scan-library/default").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(body["already_known"], 1, "{body}");
    }

    /// A file still being written is reported, not indexed.
    #[tokio::test]
    async fn a_settling_file_is_reported_rather_than_indexed() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("half-copied.pdf"), b"%PDF").unwrap();

        let (_, body) = post_json(&router, "/scan-library/default").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(
            body["settling"],
            serde_json::json!(["half-copied.pdf"]),
            "{body}"
        );
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
