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

async fn fetch_book_row(cache: std::sync::Arc<calibre_db::cache::Cache>, book_id: i32) -> Result<serde_json::Value, ServerError> {
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        cache.get_data_as_dict(None, true, Some(&ids), false)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, "default"))
}

async fn handle(state: AppState, what: String, book_id_raw: String, library_id: Option<&str>) -> Result<Response, ServerError> {
    let book_id = book_id_from_path_segment(&book_id_raw)?;
    let cache = state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or(""))))?;
    let book = fetch_book_row(cache, book_id).await?;

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
/// doc. `library_id` selects among [`AppState::libraries`] when set
/// (issue #423); in single-library mode ([`AppState::libraries`] is
/// `None`) it's accepted (matching upstream's URL shape, which OPDS
/// acquisition links always include) but has no effect.
pub async fn get(State(state): State<AppState>, Path((what, book_id, library_id)): Path<(String, String, String)>) -> Result<Response, ServerError> {
    handle(state, what, book_id, Some(&library_id)).await
}

/// Same as [`get`], for a URL with no trailing `/{library_id}` segment
/// -- always serves the default library.
pub async fn get_no_library(State(state): State<AppState>, Path((what, book_id)): Path<(String, String)>) -> Result<Response, ServerError> {
    handle(state, what, book_id, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn add_test_book_with_content(dir: &std::path::Path, cache: &calibre_db::cache::Cache, title: &str, content: &[u8]) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, content).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn add_test_book(dir: &std::path::Path, cache: &calibre_db::cache::Cache, title: &str) -> i32 {
        add_test_book_with_content(dir, cache, title, b"fake epub bytes")
    }

    fn test_app() -> (tempfile::TempDir, crate::AppState, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = calibre_db::cache::Cache::new(dir.path()).unwrap();
        let id = add_test_book(dir.path(), &cache, "Test Book");
        let _ = id;
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()) };
        let router = crate::test_router(state.clone());
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

    #[tokio::test]
    async fn library_id_in_the_url_switches_between_libraries() {
        let root = tempfile::tempdir().unwrap();
        let dir_a = root.path().join("LibA");
        let dir_b = root.path().join("LibB");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let cache_a = calibre_db::cache::Cache::new(&dir_a).unwrap();
        let cache_b = calibre_db::cache::Cache::new(&dir_b).unwrap();
        let book_a = add_test_book_with_content(&dir_a, &cache_a, "In A", b"bytes from library A");
        let book_b = add_test_book_with_content(&dir_b, &cache_b, "In B", b"bytes from library B");
        assert_eq!(book_a, book_b, "same book id, different libraries -- this is the point of the test");
        drop(cache_a);
        drop(cache_b);

        let broker = crate::library_broker::LibraryBroker::new(&[dir_a.clone(), dir_b.clone()]).unwrap();
        let ids: Vec<&str> = broker.library_ids().collect();
        assert_eq!(ids, vec!["LibA", "LibB"]);

        let state = crate::AppState {
            libraries: Some(std::sync::Arc::new(broker)),
            cache: std::sync::Arc::new(calibre_db::cache::Cache::new(&dir_a).unwrap()),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
        };
        let router = crate::test_router(state);

        let req_a = Request::builder().uri(format!("/get/epub/{book_a}/LibA")).body(Body::empty()).unwrap();
        let resp_a = router.clone().oneshot(req_a).await.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);
        let body_a = to_bytes(resp_a.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body_a[..], b"bytes from library A");

        let req_b = Request::builder().uri(format!("/get/epub/{book_b}/LibB")).body(Body::empty()).unwrap();
        let resp_b = router.clone().oneshot(req_b).await.unwrap();
        assert_eq!(resp_b.status(), StatusCode::OK);
        let body_b = to_bytes(resp_b.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body_b[..], b"bytes from library B");

        let req_unknown = Request::builder().uri(format!("/get/epub/{book_a}/NoSuchLibrary")).body(Body::empty()).unwrap();
        let resp_unknown = router.oneshot(req_unknown).await.unwrap();
        assert_eq!(resp_unknown.status(), StatusCode::NOT_FOUND);
    }
}

