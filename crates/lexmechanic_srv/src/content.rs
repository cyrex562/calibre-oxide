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
//!   used by `lexmechanic_db::catalogs::thumbnails`) is deferred.
//! - **No `opf`/`json` sub-endpoints, no plugboard metadata transforms**
//!   on format downloads (upstream's `update_metadata_in_fmts` embedding
//!   fresh metadata into EPUB/AZW3 on the fly). Format files are served
//!   as-is from disk.
//! - Files are streamed from disk in 64 KiB chunks with `Accept-Ranges`
//!   and `Content-Length`; a single `Range` gets `206` (or `416`), so pdf.js
//!   can range-load a large PDF over a NAS.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;

use crate::errors::ServerError;
use crate::AppState;

fn book_id_from_path_segment(raw: &str) -> Result<i32, ServerError> {
    let stem = raw.split('_').next().unwrap_or(raw);
    stem.parse::<i32>().map_err(|_| ServerError::NotFound(format!("Book with id {raw:?} does not exist")))
}

/// A parsed single `Range: bytes=...` request, resolved against the file's length.
#[derive(Debug, PartialEq, Eq)]
enum RangeRequest {
    /// No (or an ignorable, e.g. multi-range) `Range` header: serve it all.
    Full,
    /// Inclusive byte positions.
    Partial { start: u64, end: u64 },
    /// Syntactically valid but outside the file (or start > end): 416.
    Unsatisfiable,
}

/// Single-range support only. A header with several ranges, or another unit,
/// is ignored (the whole file is a valid answer to it, RFC 9110 14.2).
fn parse_range(header: Option<&str>, len: u64) -> RangeRequest {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeRequest::Full;
    };
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeRequest::Unsatisfiable;
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        // Suffix: the last N bytes.
        return match last.parse::<u64>() {
            Ok(n) if n > 0 && len > 0 => RangeRequest::Partial { start: len.saturating_sub(n), end: len - 1 },
            _ => RangeRequest::Unsatisfiable,
        };
    }
    let Ok(start) = first.parse::<u64>() else {
        return RangeRequest::Unsatisfiable;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match last.parse::<u64>() {
            Ok(e) => e,
            Err(_) => return RangeRequest::Unsatisfiable,
        }
    };
    if start >= len || start > end {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial { start, end: end.min(len - 1) }
}

/// `attachment; filename="<ascii fallback>"; filename*=UTF-8''<percent-encoded>`
/// (RFC 6266 / RFC 5987), so a title with accents or an em dash survives and
/// the header itself stays ASCII.
fn content_disposition(name: &str) -> String {
    let fallback: String = name.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() && !matches!(c, '"' | '\\' | '%') { c } else { '_' }).collect();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{}", urlencoding::encode(name))
}

/// Streams `len` bytes of `file` from its current position, 64 KiB at a time,
/// so a 300 MB PDF is never held in memory.
fn file_body(file: tokio::fs::File, len: u64) -> Body {
    use tokio::io::AsyncReadExt;
    let stream = futures_util::stream::unfold((file, len), |(mut file, remaining)| async move {
        if remaining == 0 {
            return None;
        }
        let mut buf = vec![0u8; remaining.min(64 * 1024) as usize];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, remaining - n as u64)))
            }
            Err(e) => Some((Err(e), (file, 0))),
        }
    });
    Body::from_stream(stream)
}

async fn serve_path(path: std::path::PathBuf, content_type: &str, download_name: Option<&str>, range: Option<&str>) -> Result<Response, ServerError> {
    use tokio::io::AsyncSeekExt;
    let mut file = tokio::fs::File::open(&path).await.map_err(|_| ServerError::NotFound("Not found".to_string()))?;
    let total = file.metadata().await.map_err(|_| ServerError::NotFound("Not found".to_string()))?.len();
    let (status, start, length) = match parse_range(range, total) {
        RangeRequest::Full => (StatusCode::OK, 0, total),
        RangeRequest::Partial { start, end } => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        RangeRequest::Unsatisfiable => {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            resp.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{total}")).unwrap());
            resp.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(resp);
        }
    };
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    }
    let mut resp = Response::new(file_body(file, length));
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    if status == StatusCode::PARTIAL_CONTENT {
        headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {start}-{}/{total}", start + length - 1)).unwrap());
    }
    if let Some(name) = download_name {
        if let Ok(v) = HeaderValue::from_str(&content_disposition(name)) {
            headers.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

/// `pub(crate)`: also used by `cdb.rs`'s cover-from-a-PDF-page route,
/// which needs a format's real on-disk path and must resolve it the
/// same way `/get` does rather than re-deriving it (re-derivation is
/// exactly what made a renamed book's formats unreachable).
pub(crate) async fn fetch_book_row(cache: std::sync::Arc<lexmechanic_db::cache::Cache>, book_id: i32) -> Result<serde_json::Value, ServerError> {
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        cache.get_data_as_dict(None, true, Some(&ids), false)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, "default"))
}

/// `pub(crate)`: reused by `legacy.rs`'s `/legacy/get` (issue #430),
/// which needs to post-process the response (strip
/// `Content-Disposition` for old Kindle browsers) rather than just
/// dispatching to the public [`get`]/[`get_no_library`] handlers.
pub(crate) async fn handle(state: AppState, what: String, book_id_raw: String, library_id: Option<&str>) -> Result<Response, ServerError> {
    handle_ranged(state, what, book_id_raw, library_id, None).await
}

/// [`handle`] honouring a `Range` header value.
pub(crate) async fn handle_ranged(state: AppState, what: String, book_id_raw: String, library_id: Option<&str>, range: Option<&str>) -> Result<Response, ServerError> {
    let book_id = book_id_from_path_segment(&book_id_raw)?;
    let cache = state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or(""))))?;
    let book = fetch_book_row(cache, book_id).await?;

    match what.as_str() {
        "cover" | "thumb" => {
            let Some(cover) = book["cover"].as_str() else {
                return Err(ServerError::NotFound("No cover for this book".to_string()));
            };
            serve_path(std::path::PathBuf::from(cover), "image/jpeg", None, range).await
        }
        fmt => {
            let key = format!("fmt_{}", fmt.to_lowercase());
            let Some(path) = book.get(&key).and_then(|v| v.as_str()) else {
                return Err(ServerError::NotFound(format!("No {} format for the book {}", fmt.to_lowercase(), book_id)));
            };
            let title = book["title"].as_str().unwrap_or("Unknown");
            let name = format!("{}.{}", title.chars().take(60).collect::<String>().replace(['"', '/'], "_"), fmt.to_lowercase());
            let mime = mime_guess::from_ext(&fmt.to_lowercase()).first_raw().unwrap_or("application/octet-stream");
            serve_path(std::path::PathBuf::from(path), mime, Some(&name), range).await
        }
    }
}

fn range_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::RANGE).and_then(|v| v.to_str().ok())
}

/// `GET /get/{what}/{book_id}/{library_id}`. Port of `content.get`
/// restricted to `cover`/`thumb`/format downloads -- see this module's
/// doc. `library_id` selects among [`AppState::libraries`] when set
/// (issue #423); in single-library mode ([`AppState::libraries`] is
/// `None`) it's accepted (matching upstream's URL shape, which OPDS
/// acquisition links always include) but has no effect.
pub async fn get(State(state): State<AppState>, headers: HeaderMap, Path((what, book_id, library_id)): Path<(String, String, String)>) -> Result<Response, ServerError> {
    handle_ranged(state, what, book_id, Some(&library_id), range_header(&headers)).await
}

/// Same as [`get`], for a URL with no trailing `/{library_id}` segment
/// -- always serves the default library.
pub async fn get_no_library(State(state): State<AppState>, headers: HeaderMap, Path((what, book_id)): Path<(String, String)>) -> Result<Response, ServerError> {
    handle_ranged(state, what, book_id, None, range_header(&headers)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn add_test_book_with_content(dir: &std::path::Path, cache: &lexmechanic_db::cache::Cache, title: &str, content: &[u8]) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, content).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn add_test_book(dir: &std::path::Path, cache: &lexmechanic_db::cache::Cache, title: &str) -> i32 {
        add_test_book_with_content(dir, cache, title, b"fake epub bytes")
    }

    fn test_app() -> (tempfile::TempDir, crate::AppState, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = lexmechanic_db::cache::Cache::new(dir.path()).unwrap();
        let id = add_test_book(dir.path(), &cache, "Test Book");
        let _ = id;
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
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

    /// Covers live in `.lexmechanic/covers/<uuid>.jpg`, not
    /// `<book folder>/cover.jpg`; `/get/cover` and `/get/thumb` must
    /// serve them from there (they 404'd every cover before).
    #[tokio::test]
    async fn get_serves_a_cover_from_the_sidecar_covers_folder() {
        let (_dir, state, router) = test_app();
        let book_id = {
            let cache = state.cache.clone();
            tokio::task::spawn_blocking(move || {
                let id = cache.get_data_as_dict(None, true, None, false).unwrap()[0]["id"].as_i64().unwrap() as i32;
                lexmechanic_db::covers::set_cover(&cache, id, b"\xff\xd8\xff fake jpeg").unwrap();
                id
            })
            .await
            .unwrap()
        };
        for what in ["cover", "thumb"] {
            let req = Request::builder().uri(format!("/get/{what}/{book_id}")).body(Body::empty()).unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "/get/{what}");
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            assert_eq!(&body[..], b"\xff\xd8\xff fake jpeg");
        }
    }

    async fn get_with_range(router: &axum::Router, uri: &str, range: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri(uri);
        if let Some(r) = range {
            req = req.header("range", r);
        }
        router.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn get_honours_byte_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let cache = lexmechanic_db::cache::Cache::new(dir.path()).unwrap();
        let id = add_test_book_with_content(dir.path(), &cache, "Ünïcödé — Range Book", b"0123456789");
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);
        let uri = format!("/get/epub/{id}");

        // No Range: whole file, but it advertises ranges and its length, and the
        // header is ASCII with an RFC 5987 UTF-8 name.
        let resp = get_with_range(&router, &uri, None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["accept-ranges"], "bytes");
        assert_eq!(resp.headers()["content-length"], "10");
        let disposition = resp.headers()["content-disposition"].to_str().unwrap().to_string();
        assert!(disposition.is_ascii(), "{disposition}");
        assert!(disposition.contains("filename=\"_n_c_d_ _ Range Book.epub\""), "{disposition}");
        assert!(disposition.contains("filename*=UTF-8''%C3%9Cn%C3%AFc%C3%B6d%C3%A9%20%E2%80%94%20Range%20Book.epub"), "{disposition}");

        let resp = get_with_range(&router, &uri, Some("bytes=2-5")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()["content-range"], "bytes 2-5/10");
        assert_eq!(resp.headers()["content-length"], "4");
        assert_eq!(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()[..], b"2345");

        // Open-ended, past-the-end and suffix ranges.
        let resp = get_with_range(&router, &uri, Some("bytes=7-")).await;
        assert_eq!(resp.headers()["content-range"], "bytes 7-9/10");
        assert_eq!(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()[..], b"789");
        let resp = get_with_range(&router, &uri, Some("bytes=8-999")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()[..], b"89");
        let resp = get_with_range(&router, &uri, Some("bytes=-3")).await;
        assert_eq!(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()[..], b"789");

        // Outside the file, or backwards: 416 naming the real length.
        for bad in ["bytes=10-20", "bytes=50-", "bytes=5-2", "bytes=-0", "bytes=x-y"] {
            let resp = get_with_range(&router, &uri, Some(bad)).await;
            assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE, "{bad}");
            assert_eq!(resp.headers()["content-range"], "bytes */10", "{bad}");
        }

        // Several ranges, or another unit, are ignored: the whole file.
        for ignored in ["bytes=0-1,4-5", "items=1-2"] {
            let resp = get_with_range(&router, &uri, Some(ignored)).await;
            assert_eq!(resp.status(), StatusCode::OK, "{ignored}");
            assert_eq!(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()[..], b"0123456789");
        }
    }

    #[test]
    fn the_content_disposition_is_ascii_with_a_utf8_name() {
        assert_eq!(content_disposition("a.pdf"), "attachment; filename=\"a.pdf\"; filename*=UTF-8''a.pdf");
        let d = content_disposition("Caf\u{e9} \"q\" 100%.pdf");
        assert!(d.is_ascii());
        assert!(d.starts_with("attachment; filename=\"Caf_ _q_ 100_.pdf\";"), "{d}");
        assert!(d.ends_with("filename*=UTF-8''Caf%C3%A9%20%22q%22%20100%25.pdf"), "{d}");
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
        let cache_a = lexmechanic_db::cache::Cache::new(&dir_a).unwrap();
        let cache_b = lexmechanic_db::cache::Cache::new(&dir_b).unwrap();
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
            cache: std::sync::Arc::new(lexmechanic_db::cache::Cache::new(&dir_a).unwrap()),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
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

