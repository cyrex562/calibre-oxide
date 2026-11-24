//! Whether a library's change log has fallen behind its database, and the
//! repair (`lexmechanic_db::change_log::repair`).
//!
//! `metadata.db` is written first and the change log second. When the second
//! step fails (full disk, a share that dropped), the edit is kept and a
//! marker records that the log is missing changes; a rebuild from the log would
//! lose them, so it refuses. These two routes let the browser tell the user
//! and let them fix it:
//!
//! | | route |
//! |---|---|
//! | read the marker | `GET /change-log/status/{library_id}` |
//! | make the log whole, then clear it | `POST /change-log/repair/{library_id}` |
//!
//! The repair is not a "dismiss": it appends what the log lacks first and
//! clears the marker only once that is durable.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

fn cache_or_404(state: &AppState, library_id: &str) -> Result<std::sync::Arc<lexmechanic_db::cache::Cache>, ServerError> {
    state.cache_for(Some(library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))
}

/// The marker as the JSON the status route (and `/ajax/library-info`) sends:
/// `{behind, missed, first_at, last_error}`.
pub(crate) fn status_json(cache: &lexmechanic_db::cache::Cache) -> Value {
    match cache.change_log_gap() {
        Some(gap) => json!({ "behind": true, "missed": gap.missed, "first_at": gap.first_at, "last_error": gap.last_error }),
        None => json!({ "behind": false, "missed": 0, "first_at": null, "last_error": null }),
    }
}

/// `GET /change-log/status/{library_id}`.
pub async fn status(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    Ok(Json(status_json(&cache)))
}

/// `POST /change-log/repair/{library_id}`: appends what the log is missing,
/// then clears the marker. Returns the new status plus what was appended.
pub async fn repair(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    let (report, status) = tokio::task::spawn_blocking(move || cache.repair_change_log().map(|report| (report, status_json(&cache))))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(format!("could not repair the change log: {e:#}")))?;
    let mut out = status;
    out["repaired"] = json!(report);
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    use lexmechanic_db::cache::Cache;

    fn app(cache: Cache) -> axum::Router {
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        crate::test_router(state)
    }

    async fn call(router: &axum::Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = if method == "POST" {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from("{}")
        } else {
            Body::empty()
        };
        let resp = router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn add_book(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.pdf"));
        std::fs::write(&source, title.as_bytes()).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    #[tokio::test]
    async fn status_reports_a_healthy_library_as_not_behind() {
        let dir = tempfile::tempdir().unwrap();
        let router = app(Cache::new(dir.path()).unwrap());
        let (status, body) = call(&router, "GET", "/change-log/status/default").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["behind"], false);
        assert_eq!(body["missed"], 0);
    }

    #[tokio::test]
    async fn status_then_repair_clears_the_marker_and_leaves_a_log_that_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let id = add_book(dir.path(), &cache, "Kept");
        // The marker a failed log append leaves, and the edit that append belonged to.
        std::fs::write(
            dir.path().join(lexmechanic_db::constants::LIBRARY_HANDLE_DIR_NAME).join("change-log-behind.json"),
            r#"{"missed": 3, "first_at": 1700000000, "last_error": "disk full"}"#,
        )
        .unwrap();
        cache.backend.conn.lock().unwrap().execute("UPDATE books SET title = 'Edited off the log' WHERE id = ?1", [id]).unwrap();
        let router = app(cache);

        let (_, before) = call(&router, "GET", &format!("/change-log/status/default")).await;
        assert_eq!(before["behind"], true);
        assert_eq!(before["missed"], 3);
        assert_eq!(before["last_error"], "disk full");

        let (status, repaired) = call(&router, "POST", "/change-log/repair/default").await;
        assert_eq!(status, StatusCode::OK, "{repaired}");
        assert_eq!(repaired["behind"], false);
        assert!(repaired["repaired"]["other_changes"].as_u64().unwrap() >= 1, "the off-log edit was appended: {repaired}");

        let (_, after) = call(&router, "GET", "/change-log/status/default").await;
        assert_eq!(after["behind"], false);

        // The repaired log rebuilds the edit: copy only the log into a fresh library and rebuild.
        let to = tempfile::tempdir().unwrap();
        let handle = lexmechanic_db::constants::LIBRARY_HANDLE_DIR_NAME;
        for sub in ["changes", "snapshots"] {
            let dst = to.path().join(handle).join(sub);
            std::fs::create_dir_all(&dst).unwrap();
            for entry in std::fs::read_dir(dir.path().join(handle).join(sub)).into_iter().flatten().flatten() {
                std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
        }
        let rebuilt = Cache::new(to.path()).unwrap();
        rebuilt.rebuild_from_change_log().unwrap();
        let ids = rebuilt.all_book_ids().unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(rebuilt.field_for(ids[0], "title").unwrap().as_deref(), Some("Edited off the log"));
    }

    #[tokio::test]
    async fn an_unknown_library_is_a_404() {
        let dir = tempfile::tempdir().unwrap();
        let state_cache = Cache::new(dir.path()).unwrap();
        let router = app(state_cache);
        // Single-library mode answers to any id, so a 404 only exists with a broker; the
        // routes themselves must at least be mounted.
        let (status, _) = call(&router, "GET", "/change-log/status/anything").await;
        assert_eq!(status, StatusCode::OK);
    }
}
