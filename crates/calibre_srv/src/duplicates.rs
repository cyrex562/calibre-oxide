//! `POST /duplicates/scan/{library_id}` -- a real, **new** route
//! (issue #762, part of #747's tracking epic): scan an entire library
//! for likely-duplicate books, independent of the add-time collision
//! check `calibre_db::copy_to_library::find_duplicate_books` already
//! provides. Real upstream (`gui2/actions/find_duplicates.py`) has a
//! Qt GUI action that does the same; this port exposes it over HTTP
//! instead, since this crate's own content server is the only real UI
//! this workspace has.
//!
//! Reuses `calibre_db::copy_to_library::scan_library_for_duplicates`
//! (added alongside this route for #762) directly -- confirmed via
//! reading `duplicate_detection_maps`'s own doc before assuming a
//! naive O(n²) re-scan was needed: it already builds a real
//! full-library author/title index once, so the scan is one map build
//! plus one lookup per book, not a pairwise comparison.
//!
//! # Real, disclosed narrowing versus upstream
//!
//! - **Review-and-delete only, no merge.** Real upstream's Find
//!   Duplicates action can also merge two duplicate records' formats
//!   into one. This route (and the frontend panel built on it) only
//!   reports groups for the user to inspect and delete the unwanted
//!   copy from -- matching this issue's own filed scope, which
//!   explicitly calls a merge action "a reasonable, separable
//!   follow-up if the simpler review-and-delete flow ships first."
//! - **Same title/author-intersection heuristic as the add-time
//!   check**, not upstream's own fuzzier matching -- see
//!   `calibre_db::utils::find_identical_books`'s own doc for what's
//!   narrower there (already a disclosed, pre-existing gap, not new
//!   here).

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use serde_json::{json, Value};

use calibre_db::copy_to_library;

use crate::errors::ServerError;
use crate::AppState;

/// `POST /duplicates/scan/{library_id}`.
pub async fn scan(State(state): State<AppState>, AxumPath(library_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;

    let groups = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let raw_groups = copy_to_library::scan_library_for_duplicates(&cache)?;
        raw_groups
            .into_iter()
            .map(|ids| -> anyhow::Result<Value> {
                let books: anyhow::Result<Vec<Value>> = ids
                    .into_iter()
                    .map(|id| {
                        let (title, authors) = copy_to_library::book_title_and_authors(&cache, id)?;
                        Ok(json!({"book_id": id, "title": title, "authors": authors}))
                    })
                    .collect();
                Ok(json!(books?))
            })
            .collect()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(json!({"groups": groups})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn add_book(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, authors: &[&str]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, b"content").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = authors.iter().map(|s| s.to_string()).collect();
        cache.add_book(&source, &meta).unwrap()
    }

    /// Builds a real library, seeds it via `seed` (run against the
    /// same `Cache` the router's own `AppState` will use -- seeding
    /// through a *second* connection to the same DB file would be
    /// real but pointlessly riskier to get right), then wraps it into
    /// a router.
    fn test_app_with<T>(seed: impl FnOnce(&std::path::Path, &Cache) -> T) -> (tempfile::TempDir, axum::Router, T) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let seeded = seed(dir.path(), &cache);
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None,
        };
        let router = crate::test_router(state);
        (dir, router, seeded)
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
    async fn a_real_near_duplicate_pair_is_reported_as_one_group() {
        let (_dir, router, (a, b)) = test_app_with(|dir, cache| {
            let a = add_book(dir, cache, "a.txt", "The Great Test", &["Ada Lovelace"]);
            let b = add_book(dir, cache, "b.txt", "The Great Test", &["Ada Lovelace"]);
            (a, b)
        });

        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        let ids: Vec<i64> = groups[0].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
        assert!(ids.contains(&(a as i64)));
        assert!(ids.contains(&(b as i64)));
        assert_eq!(groups[0][0]["title"], "The Great Test");
    }

    #[tokio::test]
    async fn a_real_library_with_no_duplicates_reports_an_empty_list() {
        let (_dir, router, ()) = test_app_with(|dir, cache| {
            add_book(dir, cache, "a.txt", "Book One", &["Author One"]);
            add_book(dir, cache, "b.txt", "Book Two", &["Author Two"]);
        });

        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["groups"].as_array().unwrap().len(), 0, "{body}");
    }

    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        Cache::new(src_dir.path()).unwrap();
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).expect("the broker's default library");
        let state = crate::AppState {
            libraries: Some(broker.clone()),
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None,
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router)
    }

    #[tokio::test]
    async fn scan_404s_for_an_unknown_library_id() {
        let (_src_dir, _dest_dir, _broker, router) = test_app_with_two_libraries();
        let (status, _) = post_json(&router, "/duplicates/scan/no-such-library").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
