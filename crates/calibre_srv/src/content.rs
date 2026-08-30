//! Port of a subset of `calibre.srv.content` -- serving book covers and
//! formats, the `/get` endpoint OPDS acquisition entries link to (see
//! `opds.rs`'s `acquisition_entry`).
//!
//! # Disclosed simplifications
//!
//! - **No conditional-GET/etag caching.** Upstream's `create_file_copy`
//!   does `If-None-Match`/`If-Modified-Since` handling and streams a
//!   cached copy from a temp-file cache keyed by content hash. This
//!   port re-reads the file from the library on every request and
//!   always returns `200 OK` with the full body -- correct, just not
//!   bandwidth-optimal for repeat requests. A real improvement here
//!   (proper `ETag`/`Last-Modified` conditional responses) is a natural
//!   next increment, not attempted yet.
//! - **No thumbnail resizing.** `thumb` returns the same full-size cover
//!   as `cover` -- upstream's `sz=WxH` resizing (reusing
//!   `oeb::transforms::rescale`-equivalent logic, already ported and
//!   used by `calibre_db::catalogs::thumbnails`) is deferred.
//! - **No `opf`/`json` sub-endpoints, no plugboard metadata transforms**
//!   on format downloads (upstream's `update_metadata_in_fmts` embedding
//!   fresh metadata into EPUB/AZW3 on the fly). Format files are served
//!   as-is from disk.
//! - **Whole file read into memory**, not streamed -- fine for typical
//!   ebook/cover sizes, would want revisiting for very large files.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;

use crate::errors::ServerError;
use crate::AppState;

fn book_id_from_path_segment(raw: &str) -> Result<i32, ServerError> {
    let stem = raw.split('_').next().unwrap_or(raw);
    stem.parse::<i32>().map_err(|_| ServerError::NotFound(format!("Book with id {raw:?} does not exist")))
}

async fn serve_path(path: std::path::PathBuf, content_type: &str, download_name: Option<&str>) -> Result<Response, ServerError> {
    let bytes = tokio::fs::read(&path).await.map_err(|_| ServerError::NotFound("Not found".to_string()))?;
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    if let Some(name) = download_name {
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

async fn fetch_book_row(state: &AppState, book_id: i32) -> Result<serde_json::Value, ServerError> {
    let cache = state.cache.clone();
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        cache.get_data_as_dict(None, true, Some(&ids), false)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, "default"))
}

async fn handle(state: AppState, what: String, book_id_raw: String) -> Result<Response, ServerError> {
    let book_id = book_id_from_path_segment(&book_id_raw)?;
    let book = fetch_book_row(&state, book_id).await?;

    match what.as_str() {
        "cover" | "thumb" => {
            let Some(cover) = book["cover"].as_str() else {
                return Err(ServerError::NotFound("No cover for this book".to_string()));
            };
            serve_path(std::path::PathBuf::from(cover), "image/jpeg", None).await
        }
        fmt => {
            let key = format!("fmt_{}", fmt.to_lowercase());
            let Some(path) = book.get(&key).and_then(|v| v.as_str()) else {
                return Err(ServerError::NotFound(format!("No {} format for the book {}", fmt.to_lowercase(), book_id)));
            };
            let title = book["title"].as_str().unwrap_or("Unknown");
            let name = format!("{}.{}", title.chars().take(60).collect::<String>().replace(['"', '/'], "_"), fmt.to_lowercase());
            let mime = mime_guess::from_ext(&fmt.to_lowercase()).first_raw().unwrap_or("application/octet-stream");
            serve_path(std::path::PathBuf::from(path), mime, Some(&name)).await
        }
    }
}

/// `GET /get/{what}/{book_id}/{library_id}`. Port of `content.get`
/// restricted to `cover`/`thumb`/format downloads -- see this module's
/// doc. `library_id` is accepted (matching upstream's URL shape, which
/// OPDS acquisition links always include) but unused -- this increment
/// is single-library, see the crate root doc.
pub async fn get(State(state): State<AppState>, Path((what, book_id, _library_id)): Path<(String, String, String)>) -> Result<Response, ServerError> {
    handle(state, what, book_id).await
}

/// Same as [`get`], for a URL with no trailing `/{library_id}` segment.
pub async fn get_no_library(State(state): State<AppState>, Path((what, book_id)): Path<(String, String)>) -> Result<Response, ServerError> {
    handle(state, what, book_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn add_test_book(dir: &std::path::Path, cache: &calibre_db::cache::Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, crate::AppState, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = calibre_db::cache::Cache::new(dir.path()).unwrap();
        let id = add_test_book(dir.path(), &cache, "Test Book");
        let _ = id;
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()) };
        let router = crate::router(state.clone());
        (dir, state, router)
    }

    #[tokio::test]
    async fn get_serves_an_existing_format() {
        let (_dir, state, router) = test_app();
        let rows = {
            let cache = state.cache.clone();
            tokio::task::spawn_blocking(move || cache.get_data_as_dict(None, true, None, false).unwrap()).await.unwrap()
        };
        let book_id = rows[0]["id"].as_i64().unwrap();

        let req = Request::builder().uri(format!("/get/epub/{book_id}")).body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"fake epub bytes");
    }

    #[tokio::test]
    async fn get_404s_for_a_missing_format() {
        let (_dir, state, router) = test_app();
        let rows = {
            let cache = state.cache.clone();
            tokio::task::spawn_blocking(move || cache.get_data_as_dict(None, true, None, false).unwrap()).await.unwrap()
        };
        let book_id = rows[0]["id"].as_i64().unwrap();

        let req = Request::builder().uri(format!("/get/pdf/{book_id}")).body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_404s_for_an_unknown_book_id() {
        let (_dir, _state, router) = test_app();
        let req = Request::builder().uri("/get/epub/999999").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

