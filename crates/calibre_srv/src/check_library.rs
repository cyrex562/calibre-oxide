//! `POST /check-library/{library_id}` -- a real, **new** route (issue
//! #748), not a port (real upstream's "Check Library" dialog is Qt
//! GUI-only, never exposed over HTTP). Runs
//! `calibre_db::check_library::CheckLibrary`'s real integrity scan
//! against a live library and returns every finding as structured
//! JSON.
//!
//! `CheckLibrary` was originally written against `calibre_db::Library`
//! (the pre-`Cache` API) -- `calibre_srv` only ever holds a `Cache`,
//! and unlike `legacy::LegacyDb` (issue #749's own finding), `Library`
//! and `Cache` are sibling structs that each independently own their
//! own connection, not wrapper/wrapped, so this needed a real port
//! (`Cache::get_book`/`all_authors`/`format_files`/`is_case_sensitive`,
//! `CheckLibrary` itself now taking `&Cache`) rather than a one-line
//! delegation. See that module's own doc for the details, including a
//! real pre-existing false-positive found and fixed along the way
//! (WAL-mode SQLite sidecar files were reported as "invalid authors").

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use calibre_db::check_library::CheckLibrary;

use crate::errors::ServerError;
use crate::AppState;

fn encode_triples(items: &[(String, String, i32)]) -> Value {
    json!(items.iter().map(|(a, b, id)| json!({"a": a, "b": b, "book_id": id})).collect::<Vec<_>>())
}

/// `POST /check-library/{library_id}`.
pub async fn check(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;

    let result = tokio::task::spawn_blocking(move || {
        let library_path = cache.backend.library_path.clone();
        let mut checker = CheckLibrary::new(library_path, &cache);
        checker.scan_library(vec![], vec![]);
        json!({
            "invalid_titles": encode_triples(&checker.invalid_titles),
            "extra_titles": encode_triples(&checker.extra_titles),
            "invalid_authors": encode_triples(&checker.invalid_authors),
            "extra_authors": encode_triples(&checker.extra_authors),
            "missing_formats": encode_triples(&checker.missing_formats),
            "extra_formats": encode_triples(&checker.extra_formats),
            "extra_files": encode_triples(&checker.extra_files),
            "missing_covers": encode_triples(&checker.missing_covers),
            "extra_covers": encode_triples(&checker.extra_covers),
            "malformed_formats": encode_triples(&checker.malformed_formats),
            "malformed_paths": encode_triples(&checker.malformed_paths),
            "corrupted_formats": encode_triples(&checker.corrupted_formats),
            "corrupted_covers": encode_triples(&checker.corrupted_covers),
        })
    })
    .await
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
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn a_real_clean_library_reports_no_findings() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/check-library/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["invalid_authors"], serde_json::json!([]));
        assert_eq!(body["extra_formats"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_real_extra_unrecognized_top_level_entry_is_reported() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("not_a_real_calibre_file.bin"), b"junk").unwrap();

        let (status, body) = post_json(&router, "/check-library/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let invalid_authors = body["invalid_authors"].as_array().unwrap();
        assert!(invalid_authors.iter().any(|v| v["a"] == "not_a_real_calibre_file.bin"), "{body}");
    }

    #[tokio::test]
    async fn check_404s_for_an_unknown_library_id() {
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
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).unwrap();
        let state = crate::AppState {
            libraries: Some(broker),
            cache: default_cache,
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
        };
        let router = crate::test_router(state);

        let (status, _) = post_json(&router, "/check-library/no-such-library").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
