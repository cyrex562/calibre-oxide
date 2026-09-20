//! Port of `calibre.srv.content`'s data-files endpoints -- arbitrary
//! files attached to a book's own directory outside its standard
//! formats (e.g. a companion PDF), backed by `calibre_db::extra_files`
//! (issue #418).
//!
//! - `GET /data-files/get/{book_id}/{*relpath}` -- serve one attached
//!   file. `relpath` is a wildcard (multi-segment) capture, since a
//!   real relpath contains `/` (e.g. `data/notes.pdf`) -- `axum`
//!   requires a wildcard segment to be the last one in a route, so
//!   (unlike every other endpoint in this crate) this route has no
//!   trailing `library_id` segment at all -- single-library-only for
//!   this one endpoint specifically, not a narrowing shared with the
//!   rest of the crate (`upload`/`remove` below do honor theirs, via
//!   `AppState::cache_for`).
//! - `POST /data-files/upload/{book_id}/{library_id}` -- base64-encoded
//!   files in a JSON body, `[{data_url, name}]`.
//! - `POST /data-files/remove/{book_id}/{library_id}` -- remove by
//!   relpath, a JSON array of relpaths.
//! - `GET /data-files/list/{book_id}/{library_id}` -- real, **new**
//!   route (issue #757): `upload`/`remove` both already return the
//!   post-operation file list as a side effect, but nothing lets a
//!   client fetch it *before* the first upload -- a real gap for any
//!   UI that wants to show a book's already-attached data files.
//!
//! # Content-Type/disposition, narrowed beyond upstream's own fidelity
//!
//! Upstream's `GET` endpoint defaults `Content-Disposition` to
//! `attachment` but lets the client override it to `inline` via a
//! `?content_disposition=` query parameter, serving with a
//! filename-guessed `Content-Type` -- the exact shape of the stored-
//! XSS finding already fixed in `calibre_srv::notes` (PR #415): an
//! attacker who can reach `upload` could name a file `evil.html`, then
//! share a `.../data-files/get/.../evil.html?content_disposition=inline`
//! link that renders same-origin script on click. This port only
//! honors `inline` for a real image-only Content-Type allowlist (same
//! list `notes.rs` uses) -- anything else always downloads as an
//! attachment with `application/octet-stream`, regardless of the query
//! parameter.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

use calibre_db::extra_files::{self, ExtraFile};

use crate::errors::ServerError;
use crate::AppState;

fn encode_stat(f: &ExtraFile) -> Value {
    serde_json::json!({ "size": f.size, "mtime_ns": f.mtime_ns })
}

fn safe_content_type(relpath: &str) -> &'static str {
    match mime_guess::from_path(relpath).first_raw() {
        Some(m @ ("image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif")) => m,
        _ => "application/octet-stream",
    }
}

fn filename_of(relpath: &str) -> &str {
    relpath.rsplit('/').next().unwrap_or(relpath)
}

#[derive(Debug, Deserialize)]
pub struct GetQuery {
    #[serde(default)]
    content_disposition: Option<String>,
}

/// `GET /data-files/get/{book_id}/{*relpath}`. Port of
/// `get_data_file`.
pub async fn get(State(state): State<AppState>, Path((book_id, relpath)): Path<(i32, String)>, Query(q): Query<GetQuery>) -> Result<Response, ServerError> {
    let files = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || extra_files::list_extra_files(&cache, book_id, "data/**/*")
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let Some(file) = files.into_iter().find(|f| f.relpath == relpath) else {
        return Err(ServerError::NotFound(format!("No data file {relpath} in book {book_id}")));
    };

    let bytes = tokio::fs::read(&file.file_path).await.map_err(|_| ServerError::NotFound(format!("No data file {relpath} in book {book_id}")))?;
    let content_type = safe_content_type(&relpath);
    let wants_inline = q.content_disposition.as_deref() == Some("inline");
    let disposition = if wants_inline && content_type != "application/octet-stream" { "inline" } else { "attachment" };

    let mut resp = bytes.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
    if let Ok(v) = header::HeaderValue::from_str(&format!("{disposition}; filename=\"{}\"", filename_of(&relpath))) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// `GET /data-files/list/{book_id}/{library_id}`.
pub async fn list(State(state): State<AppState>, Path((book_id, library_id)): Path<(i32, String)>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let data_files = tokio::task::spawn_blocking(move || extra_files::list_extra_files(&cache, book_id, "data/**/*"))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    let data_files_json: serde_json::Map<String, Value> = data_files.iter().map(|f| (f.relpath.clone(), encode_stat(f))).collect();
    Ok(Json(serde_json::json!({ "data_files": Value::Object(data_files_json) })))
}

#[derive(Debug, Deserialize)]
pub struct UploadSpec {
    name: String,
    data_url: String,
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>, ServerError> {
    let (_, payload) = data_url.split_once(',').ok_or_else(|| ServerError::BadRequest("Invalid query: malformed data URL".to_string()))?;
    base64::engine::general_purpose::STANDARD.decode(payload).map_err(|e| ServerError::BadRequest(format!("Invalid query: {e}")))
}

/// `POST /data-files/upload/{book_id}/{library_id}`. Port of
/// `upload_data_files`.
pub async fn upload(State(state): State<AppState>, Path((book_id, library_id)): Path<(i32, String)>, Json(body): Json<Vec<UploadSpec>>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let mut files = HashMap::new();
    for spec in &body {
        let data = decode_data_url(&spec.data_url)?;
        files.insert(format!("data/{}", spec.name), data);
    }

    let (err, data_files) = tokio::task::spawn_blocking({
        let cache = cache.clone();
        move || -> anyhow::Result<(String, Vec<ExtraFile>)> {
            let err = match extra_files::add_extra_files(&cache, book_id, &files, true) {
                Ok(_) => String::new(),
                Err(e) => e.to_string(),
            };
            let data_files = extra_files::list_extra_files(&cache, book_id, "data/**/*")?;
            Ok((err, data_files))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let data_files_json: serde_json::Map<String, Value> = data_files.iter().map(|f| (f.relpath.clone(), encode_stat(f))).collect();
    Ok(Json(serde_json::json!({ "error": err, "data_files": Value::Object(data_files_json) })))
}

/// `POST /data-files/remove/{book_id}/{library_id}`. Port of
/// `remove_data_files`.
pub async fn remove(State(state): State<AppState>, Path((book_id, library_id)): Path<(i32, String)>, Json(relpaths): Json<Vec<String>>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let (errors, data_files) = tokio::task::spawn_blocking({
        let cache = cache.clone();
        move || -> anyhow::Result<(HashMap<String, Option<String>>, Vec<ExtraFile>)> {
            let errors = extra_files::remove_extra_files(&cache, book_id, &relpaths, true)?;
            let data_files = extra_files::list_extra_files(&cache, book_id, "data/**/*")?;
            Ok((errors, data_files))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let data_files_json: serde_json::Map<String, Value> = data_files.iter().map(|f| (f.relpath.clone(), encode_stat(f))).collect();
    let mut ans = serde_json::Map::new();
    ans.insert("data_files".into(), Value::Object(data_files_json));
    let real_errors: serde_json::Map<String, Value> = errors.into_iter().filter_map(|(k, v)| v.map(|msg| (k, Value::String(msg)))).collect();
    if !real_errors.is_empty() {
        ans.insert("errors".into(), Value::Object(real_errors));
    }
    Ok(Json(Value::Object(ans)))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.epub");
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Book".to_string();
        meta.authors = vec!["Author".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);
        (dir, router, book_id)
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
    async fn upload_then_get_round_trips_a_data_file() {
        let (_dir, router, book_id) = test_app();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"pdf bytes here");
        let (status, body) = post_json(&router, &format!("/data-files/upload/{book_id}/default"), serde_json::json!([{"name": "notes.pdf", "data_url": format!("data:application/pdf;base64,{encoded}")}])).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["error"], "");
        assert!(body["data_files"]["data/notes.pdf"]["size"].as_u64().unwrap() > 0);

        let req = Request::builder().uri(format!("/data-files/get/{book_id}/data/notes.pdf")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-disposition").unwrap().to_str().unwrap(), "attachment; filename=\"notes.pdf\"");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"pdf bytes here");
    }

    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router, i32) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let book_id = {
            let cache = Cache::new(src_dir.path()).unwrap();
            let source = src_dir.path().join("Book.epub");
            std::fs::write(&source, b"fake epub bytes").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = "Book".to_string();
            meta.authors = vec!["Author".to_string()];
            cache.add_book(&source, &meta).unwrap()
        };
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).expect("the broker's default library");
        let state = crate::AppState { libraries: Some(broker.clone()), cache: default_cache, opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router, book_id)
    }

    #[tokio::test]
    async fn upload_with_a_real_library_id_segment_targets_that_library() {
        // #725: `upload`/`remove` used to ignore their own
        // {library_id} path segment entirely and always hit
        // `state.cache` (the broker's default library) -- confirm the
        // data file is really stored against the NAMED library.
        let (src_dir, _dest_dir, broker, router, book_id) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();
        let src_cache = broker.get(Some(src_name)).unwrap();

        let encoded = base64::engine::general_purpose::STANDARD.encode(b"pdf bytes here");
        let (status, body) = post_json(&router, &format!("/data-files/upload/{book_id}/{src_name}"), serde_json::json!([{"name": "notes.pdf", "data_url": format!("data:application/pdf;base64,{encoded}")}])).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");

        let data_files = calibre_db::extra_files::list_extra_files(&src_cache, book_id, "data/**/*").unwrap();
        assert!(data_files.iter().any(|f| f.relpath == "data/notes.pdf"), "the data file should really be stored in the named library's own cache");
    }

    #[tokio::test]
    async fn upload_404s_for_an_unknown_library_id() {
        // Must use multi-library mode: in single-library mode
        // (`AppState::libraries == None`), `cache_for` always falls
        // back to the default library regardless of the requested
        // name -- an unknown name can only actually 404 once a real
        // `LibraryBroker` is involved.
        let (_src_dir, _dest_dir, _broker, router, book_id) = test_app_with_two_libraries();
        let (status, _) = post_json(&router, &format!("/data-files/upload/{book_id}/no-such-library"), serde_json::json!([])).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_404s_for_an_unknown_relpath() {
        let (_dir, router, book_id) = test_app();
        let req = Request::builder().uri(format!("/data-files/get/{book_id}/data/nope.pdf")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn upload_and_remove_round_trip() {
        let (_dir, router, book_id) = test_app();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"x");
        post_json(&router, &format!("/data-files/upload/{book_id}/default"), serde_json::json!([{"name": "a.txt", "data_url": format!("data:text/plain;base64,{encoded}")}])).await;

        let (status, body) = post_json(&router, &format!("/data-files/remove/{book_id}/default"), serde_json::json!(["data/a.txt"])).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["data_files"].as_object().unwrap().len(), 0);
        assert!(body.get("errors").is_none());

        let req = Request::builder().uri(format!("/data-files/get/{book_id}/data/a.txt")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_serves_a_non_image_as_attachment_even_if_inline_is_requested() {
        // Regression test for the same stored-XSS class fixed in
        // notes.rs (PR #415): requesting ?content_disposition=inline
        // on a non-image file must not be honored.
        let (_dir, router, book_id) = test_app();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"<script>alert(1)</script>");
        post_json(&router, &format!("/data-files/upload/{book_id}/default"), serde_json::json!([{"name": "evil.html", "data_url": format!("data:text/html;base64,{encoded}")}])).await;

        let req = Request::builder().uri(format!("/data-files/get/{book_id}/data/evil.html?content_disposition=inline")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/octet-stream");
        assert!(resp.headers().get("content-disposition").unwrap().to_str().unwrap().starts_with("attachment"));
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn list_reports_a_real_previously_uploaded_file() {
        let (_dir, router, book_id) = test_app();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"pdf bytes here");
        post_json(&router, &format!("/data-files/upload/{book_id}/default"), serde_json::json!([{"name": "notes.pdf", "data_url": format!("data:application/pdf;base64,{encoded}")}])).await;

        let (status, body) = get_json(&router, &format!("/data-files/list/{book_id}/default")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["data_files"]["data/notes.pdf"]["size"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn list_is_empty_for_a_book_with_no_data_files() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = get_json(&router, &format!("/data-files/list/{book_id}/default")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data_files"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upload_rejects_a_path_traversal_name() {
        let (_dir, router, book_id) = test_app();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"pwned");
        let (status, body) = post_json(
            &router,
            &format!("/data-files/upload/{book_id}/default"),
            serde_json::json!([{"name": "../../../../../../../tmp/data-files-traversal-poc", "data_url": format!("data:text/plain;base64,{encoded}")}]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["data_files"].as_object().unwrap().len(), 0, "the traversal write should have been rejected, not silently succeeded");
        assert!(!std::path::Path::new("/tmp/data-files-traversal-poc").exists());
    }
}
