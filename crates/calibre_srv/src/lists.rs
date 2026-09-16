//! Write endpoints for virtual libraries and saved searches -- real,
//! **new** routes, not a port. Confirmed by grepping the real upstream
//! `calibre/srv/*.py` tree: neither one is ever exposed over HTTP
//! there at all (`ajax.py`/`code.py` only ever *read*
//! `db._pref('virtual_libraries', {})`; both features are otherwise
//! managed purely through the desktop GUI's own preferences dialogs).
//! This port's UI is web-only (no Qt GUI, see the project's own
//! architecture notes), so managing either one at all needs a real
//! HTTP surface that upstream simply never needed.
//!
//! Both features are a thin `{name: query}` map under one preference
//! key -- `calibre_db::cache::Cache`'s own `virtual_library_*`/
//! `saved_search_*` methods already implement real add/delete/(rename
//! for saved searches only, virtual libraries have no rename method to
//! wrap) logic; these handlers are direct, thin wrappers.
//!
//! - `GET /ajax/saved-searches` -- the whole map, matching
//!   `ajax::virtual_libraries`'s own shape (that route already exists;
//!   this is its saved-search counterpart, previously missing even as
//!   a read route).
//! - `POST /vl/set/{name}` / `POST /vl/delete/{name}` --
//!   `Cache::virtual_library_add`/`_delete`.
//! - `POST /saved-search/set/{name}` / `POST /saved-search/delete/{name}`
//!   / `POST /saved-search/rename/{old_name}/{new_name}` --
//!   `Cache::saved_search_add`/`_delete`/`_rename`.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct SetQueryBody {
    query: String,
}

/// `GET /ajax/saved-searches`.
pub async fn saved_searches(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let map = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.saved_search_names().and_then(|names| names.into_iter().map(|n| Ok((n.clone(), cache.saved_search_lookup(&n)?.unwrap_or_default()))).collect::<anyhow::Result<std::collections::HashMap<_, _>>>())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(serde_json::json!(map)))
}

/// `POST /vl/set/{name}`, body `{"query": "..."}`.
pub async fn set_virtual_library(State(state): State<AppState>, Path(name): Path<String>, Json(body): Json<SetQueryBody>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.virtual_library_add(&name, &body.query)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /vl/delete/{name}`.
pub async fn delete_virtual_library(State(state): State<AppState>, Path(name): Path<String>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.virtual_library_delete(&name)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /saved-search/set/{name}`, body `{"query": "..."}`.
pub async fn set_saved_search(State(state): State<AppState>, Path(name): Path<String>, Json(body): Json<SetQueryBody>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.saved_search_add(&name, &body.query)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /saved-search/delete/{name}`.
pub async fn delete_saved_search(State(state): State<AppState>, Path(name): Path<String>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.saved_search_delete(&name)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /saved-search/rename/{old_name}/{new_name}`.
pub async fn rename_saved_search(State(state): State<AppState>, Path((old_name, new_name)): Path<(String, String)>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.saved_search_rename(&old_name, &new_name)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
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
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn virtual_library_set_then_list_then_delete_round_trips() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/vl/set/My%20VL", serde_json::json!({"query": "tags:scifi"})).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = get_json(&router, "/ajax/virtual-libraries").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["My VL"], "tags:scifi");

        let (status, _) = post_json(&router, "/vl/delete/My%20VL", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&router, "/ajax/virtual-libraries").await;
        assert!(body.get("My VL").is_none());
    }

    #[tokio::test]
    async fn saved_search_set_then_list_then_rename_then_delete_round_trips() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/saved-search/set/My%20Search", serde_json::json!({"query": "authors:asimov"})).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = get_json(&router, "/ajax/saved-searches").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["My Search"], "authors:asimov");

        let (status, _) = post_json(&router, "/saved-search/rename/My%20Search/Renamed", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&router, "/ajax/saved-searches").await;
        assert!(body.get("My Search").is_none());
        assert_eq!(body["Renamed"], "authors:asimov");

        let (status, _) = post_json(&router, "/saved-search/delete/Renamed", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&router, "/ajax/saved-searches").await;
        assert!(body.as_object().unwrap().is_empty());
    }
}
