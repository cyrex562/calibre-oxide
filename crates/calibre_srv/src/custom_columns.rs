//! Write endpoints for custom column management -- real, **new**
//! routes, not a port. `calibre_db::cli`'s own
//! `cmd_add_custom_column`/`cmd_remove_custom_column`/
//! `cmd_custom_columns` are CLI-layer only (raw `&[String]` args,
//! `println!` output) and, per real upstream's own
//! `cmd_add_custom_column.py`, have no `implementation()` a remote
//! command could call either (`raise NotImplementedError()` there) --
//! see `crate::cdb`'s own `remote_command` doc for the identical gap
//! already found in that module. These handlers are a real,
//! independent `implementation`-equivalent on top of `Cache`'s own
//! already-real `custom_column_label_map`/`add_custom_column`/
//! `remove_custom_column` (issue #720).
//!
//! - `GET /custom-columns` -- the full label map (same shape
//!   `calibre_db::field_metadata::FieldMetadata::from_cache` already
//!   builds its `#label`-keyed entries from; `/ajax/field-metadata`
//!   is still the richer, field-metadata-shaped way to discover
//!   columns for rendering purposes -- this route is for the
//!   management UI's own listing, a flatter `{label: {..}}` map).
//! - `POST /custom-columns/add` -- body `{label, name, datatype,
//!   is_multiple}`.
//! - `POST /custom-columns/remove/{label}`.
//!
//! # Real bug found and fixed alongside this module
//!
//! Issue #720's own filing claimed `POST /cdb/set-fields` "already"
//! writes an existing custom column's value because it accepts
//! arbitrary field names. That was checked against `Cache::set_field`
//! directly and found false: its fallback arm unconditionally
//! rejected any name it didn't special-case, including every real
//! custom column label. Fixed in `calibre_db::cache::Cache::set_field`
//! (now checks the real `custom_columns` table before giving up) --
//! `/cdb/set-fields` needed no changes of its own once that was
//! fixed, since it already forwards arbitrary field names straight to
//! `Cache::set_field`.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::ServerError;
use crate::AppState;

/// `GET /custom-columns`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let map = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.custom_column_label_map()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(serde_json::json!(map)))
}

#[derive(Debug, Deserialize)]
pub struct AddCustomColumnBody {
    label: String,
    name: String,
    datatype: String,
    #[serde(default)]
    is_multiple: bool,
}

/// `POST /custom-columns/add`.
pub async fn add(State(state): State<AppState>, Json(body): Json<AddCustomColumnBody>) -> Result<Json<Value>, ServerError> {
    let col_id = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.add_custom_column(&body.label, &body.name, &body.datatype, body.is_multiple)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    // A duplicate label / unsupported combination (e.g. a
    // multiple-value text column, see `Cache::add_custom_column`'s
    // own doc) is a client error, not a server one.
    .map_err(|e| ServerError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({"num": col_id})))
}

/// `POST /custom-columns/remove/{label}`.
pub async fn remove(State(state): State<AppState>, Path(label): Path<String>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.remove_custom_column(&label)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::BadRequest(e.to_string()))?;
    Ok(())
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
        meta.title = "Book".to_string();
        meta.authors = vec!["Author".to_string()];
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
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None,
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
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
    async fn add_then_list_then_remove_round_trips() {
        let (_dir, router, _book_id) = test_app();

        let (status, body) = post_json(&router, "/custom-columns/add", serde_json::json!({"label": "shelf", "name": "Shelf", "datatype": "text"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["num"].as_i64().is_some());

        let (status, body) = get_json(&router, "/custom-columns").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["shelf"]["datatype"], "text");
        assert_eq!(body["shelf"]["name"], "Shelf");

        let (status, _) = post_json(&router, "/custom-columns/remove/shelf", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&router, "/custom-columns").await;
        assert!(body.get("shelf").is_none());
    }

    #[tokio::test]
    async fn add_rejects_a_duplicate_label() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(&router, "/custom-columns/add", serde_json::json!({"label": "dup", "name": "Dup", "datatype": "text"})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = post_json(&router, "/custom-columns/add", serde_json::json!({"label": "dup", "name": "Dup 2", "datatype": "text"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_real_custom_column_value_round_trips_through_set_fields_and_ajax_book() {
        // Real end-to-end proof of this module's own doc: #720's
        // claim that `/cdb/set-fields` "already" wrote custom column
        // values was false until `Cache::set_field`'s fallback was
        // fixed. Drives the exact HTTP routes `web/`'s frontend would
        // call, not `Cache` methods directly.
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(&router, "/custom-columns/add", serde_json::json!({"label": "mycol", "name": "My Column", "datatype": "text"})).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_json(&router, &format!("/cdb/set-fields/{book_id}"), serde_json::json!({"changes": {"mycol": "hello there"}})).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (status, body) = get_json(&router, &format!("/ajax/book/{book_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["mycol"], "hello there");
    }
}
