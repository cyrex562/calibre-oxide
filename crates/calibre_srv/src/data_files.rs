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
//!   trailing `library_id` segment at all rather than accepting-and-
//!   ignoring one; single-library-only, same as everywhere else.
//! - `POST /data-files/upload/{book_id}/{library_id}` -- base64-encoded
//!   files in a JSON body, `[{data_url, name}]`.
//! - `POST /data-files/remove/{book_id}/{library_id}` -- remove by
//!   relpath, a JSON array of relpaths.
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
pub async fn upload(State(state): State<AppState>, Path((book_id, _library_id)): Path<(i32, String)>, Json(body): Json<Vec<UploadSpec>>) -> Result<Json<Value>, ServerError> {
    let mut files = HashMap::new();
    for spec in &body {
        let data = decode_data_url(&spec.data_url)?;
        files.insert(format!("data/{}", spec.name), data);
    }

    let (err, data_files) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
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
pub async fn remove(State(state): State<AppState>, Path((book_id, _library_id)): Path<(i32, String)>, Json(relpaths): Json<Vec<String>>) -> Result<Json<Value>, ServerError> {
    let (errors, data_files) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
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
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
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
