//! `POST /rename-category-item/{category}/{item_name}/{library_id}`
//! -- a real, **new** route (issue #749), not a port. Real rename/
//! merge logic already existed (`calibre_db::legacy::LegacyDb::
//! rename_author`/`rename_tag`/`rename_publisher`, from the #223
//! `LibraryDatabase` compatibility layer), but `LegacyDb` opens its
//! own independent `Cache`/SQLite connection onto the library file
//! rather than accepting an already-open one -- `calibre_srv` never
//! opens a second connection to the same library, so it couldn't
//! reach this logic without one. Moved onto `Cache` itself
//! (`Cache::rename_author`/`rename_tag`/`rename_publisher`, with
//! `LegacyDb`'s own three methods now delegating there) so a real
//! caller with only a `Cache` in scope can use it directly -- the same
//! "real backend, wrong host type" gap this whole issue cluster keeps
//! finding, just one layer removed from the usual "zero HTTP route"
//! shape.
//!
//! # Scope
//!
//! `authors`/`tags`/`publisher` only, matching the three real methods
//! `Cache` actually has (itself matching what `legacy.rs`'s own port
//! covers, not upstream's larger `rename_items` surface -- see that
//! module's own doc). `series`/`languages` are real standard
//! categories elsewhere in this crate (`categories::get_item_id`
//! resolves them fine) but have no rename method to call -- requesting
//! either 400s with a clear message rather than silently no-op'ing.
//!
//! Renaming to a name that collides with an existing item of the same
//! category is a real merge (every book linked to the old item is
//! re-pointed at the existing one, and the old item is dropped), not
//! an error -- matching `Cache::rename_author`'s own real, disclosed
//! behavior.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::categories;

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    new_name: String,
}

/// `POST /rename-category-item/{category}/{item_name}/{library_id}`.
pub async fn rename(State(state): State<AppState>, Path((category, item_name, library_id)): Path<(String, String, String)>, Json(body): Json<RenameBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;

    tokio::task::spawn_blocking(move || -> Result<(), ServerError> {
        // Checked before resolving the item so an unsupported category
        // always reports the same clear "not supported" reason,
        // whether or not the named item happens to exist.
        let rename_fn: fn(&calibre_db::cache::Cache, i32, &str) -> anyhow::Result<()> = match category.as_str() {
            "authors" => calibre_db::cache::Cache::rename_author,
            "tags" => calibre_db::cache::Cache::rename_tag,
            "publisher" => calibre_db::cache::Cache::rename_publisher,
            _ => return Err(ServerError::BadRequest(format!("renaming {category:?} is not supported"))),
        };
        let item_id = categories::get_item_id(&cache, &category, &item_name)
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("No {category} named {item_name:?}")))?;
        rename_fn(&cache, item_id, &body.new_name).map_err(|e| ServerError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.txt");
        std::fs::write(&source, b"hello").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["Old Name".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None,
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
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
    async fn renames_a_real_author_end_to_end() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, "/rename-category-item/authors/Old%20Name/default", serde_json::json!({"new_name": "New Name"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (status, book) = get_json(&router, &format!("/ajax/book/{book_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(book["authors"], serde_json::json!(["New Name"]));
    }

    #[tokio::test]
    async fn rejects_an_unsupported_category() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(&router, "/rename-category-item/series/Some%20Series/default", serde_json::json!({"new_name": "New Series"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rename_404s_for_an_unknown_item_name() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(&router, "/rename-category-item/authors/Nobody/default", serde_json::json!({"new_name": "X"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
