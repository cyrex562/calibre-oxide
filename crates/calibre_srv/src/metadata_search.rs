//! `POST /metadata/search` + `GET /metadata/cover-proxy` -- real,
//! **new** routes (issue #787, part of the #750 epic): query both real
//! online metadata source clients ([`calibre_ebooks::metadata::sources::google_books`],
//! [`calibre_ebooks::metadata::sources::open_library`]) concurrently and
//! return their candidates, for the frontend's per-field picker (a
//! later sub-issue, #788) to present.
//!
//! # Why a cover-proxy route
//!
//! A candidate's `cover_url` points at an external host (Google's or
//! Open Library's own image CDN). Fetching it directly from the
//! browser would need CORS cooperation from hosts this project doesn't
//! control (and doesn't reliably get). Routing the fetch through this
//! server instead sidesteps that, and keeps the one real SSRF-relevant
//! check ([`crate::net_guard::resolve_and_check`], already used by
//! `news.rs`/`share.rs` for the same class of "server dials a
//! caller-supplied host" route) in one place.
//!
//! # No "apply" route
//!
//! Applying a chosen candidate's fields to a book reuses the already-real
//! `POST /cdb/set-fields/{book_id}/{library_id}` directly from the
//! frontend (its cover field already accepts base64 image bytes, per
//! `cdb::tests::set_fields_sets_cover_from_base64_jpeg`) -- no new
//! write-side route needed here.
//!
//! # Partial-source-failure handling
//!
//! If one source errors (rate-limited, transient network issue -- a
//! real, observed case: Google Books' anonymous per-project daily
//! quota) but the other returns real candidates, this route still
//! responds 200 with whatever candidates it has, plus a
//! `source_errors` array so the frontend can show which source(s)
//! came up empty. Only a request where *both* sources failed AND
//! neither returned any candidates is a hard error.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_ebooks::metadata::sources::{google_books, open_library, MetadataCandidate};
use calibre_ebooks::scraper::{Browser, OpenOptions};

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct SearchBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    #[serde(default)]
    pub isbn: Option<String>,
}

fn is_blank(s: &Option<String>) -> bool {
    s.as_deref().unwrap_or("").trim().is_empty()
}

/// `POST /metadata/search`.
pub async fn search(State(_state): State<AppState>, Json(body): Json<SearchBody>) -> Result<Json<Value>, ServerError> {
    if is_blank(&body.title) && is_blank(&body.authors) && is_blank(&body.isbn) {
        return Err(ServerError::BadRequest("at least one of title, authors, or isbn is required".to_string()));
    }

    let SearchBody { title, authors, isbn } = body;

    let google_query = google_books::GoogleBooksQuery { title: title.clone(), authors: authors.clone(), isbn: isbn.clone() };
    let open_library_query = open_library::OpenLibraryQuery { title, authors, isbn };

    let (google, open_lib) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            let browser = Browser::new("", &[], true);
            google_books::search(&browser, &google_query)
        }),
        tokio::task::spawn_blocking(move || {
            let browser = Browser::new("", &[], true);
            open_library::search(&browser, &open_library_query)
        }),
    );

    let mut candidates: Vec<MetadataCandidate> = Vec::new();
    let mut source_errors: Vec<String> = Vec::new();

    match google {
        Ok(Ok(mut results)) => candidates.append(&mut results),
        Ok(Err(e)) => source_errors.push(format!("Google Books: {e}")),
        Err(e) => source_errors.push(format!("Google Books: {e}")),
    }
    match open_lib {
        Ok(Ok(mut results)) => candidates.append(&mut results),
        Ok(Err(e)) => source_errors.push(format!("Open Library: {e}")),
        Err(e) => source_errors.push(format!("Open Library: {e}")),
    }

    if candidates.is_empty() && !source_errors.is_empty() {
        return Err(ServerError::FailedDependency(source_errors.join("; ")));
    }

    Ok(Json(json!({"candidates": candidates, "source_errors": source_errors})))
}

#[derive(Debug, Deserialize)]
pub struct CoverProxyParams {
    pub url: String,
}

/// `GET /metadata/cover-proxy?url=...`.
pub async fn cover_proxy(Query(params): Query<CoverProxyParams>) -> Result<Response, ServerError> {
    let parsed = url::Url::parse(&params.url).map_err(|e| ServerError::BadRequest(format!("{}: invalid URL ({e})", params.url)))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ServerError::BadRequest(format!("{}: only http/https URLs are allowed", params.url)));
    }
    let host = parsed.host_str().ok_or_else(|| ServerError::BadRequest(format!("{}: no host", params.url)))?.to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    crate::net_guard::resolve_and_check(&host, port).await.map_err(|e| ServerError::BadRequest(format!("{}: {e}", params.url)))?;

    let fetch_url = params.url.clone();
    let (bytes, content_type) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String), String> {
        let browser = Browser::new("", &[], true);
        let resp = browser.open_novisit(&fetch_url, &OpenOptions::default()).map_err(|e| e.to_string())?;
        if let Some(status) = resp.status() {
            if status >= 400 {
                return Err(format!("upstream returned HTTP {status}"));
            }
        }
        let content_type = resp.header("content-type").unwrap_or("image/jpeg").to_string();
        Ok((resp.read().to_vec(), content_type))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(ServerError::FailedDependency)?;

    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    /// A tiny single-route HTTP/1.1 test server, serving one fixed
    /// JSON/bytes body at any path -- matches the source clients' own
    /// private `TestSite` helpers (not shared across crates'
    /// `#[cfg(test)]` modules).
    struct TestSite {
        addr: std::net::SocketAddr,
    }

    impl TestSite {
        fn start(content_type: &'static str, body: &'static [u8]) -> TestSite {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream: TcpStream = stream;
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    loop {
                        let mut l = String::new();
                        if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                    }
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(body);
                }
            });
            TestSite { addr }
        }

        fn url(&self) -> String {
            format!("http://{}/cover.jpg", self.addr)
        }
    }

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
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None,
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn search_rejects_an_entirely_empty_query() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/metadata/search", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_by_title_returns_candidates_from_at_least_one_real_source() {
        // A real live-network test (no mock server -- both source
        // clients' own base URLs are hardcoded, not injectable through
        // this route). Tolerate one source being unavailable (this
        // box's own Google Books quota is known-exhausted, see #785's
        // PR) but require at least one real candidate to come back --
        // that's the one behavior this route actually promises.
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/metadata/search", serde_json::json!({"title": "Dune", "authors": "Frank Herbert"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let candidates = body["candidates"].as_array().unwrap();
        assert!(!candidates.is_empty(), "expected at least one real candidate, got {body}");
        assert!(candidates.iter().any(|c| c["title"].as_str().unwrap_or_default().contains("Dune")));
    }

    #[tokio::test]
    async fn cover_proxy_streams_real_bytes_with_the_real_content_type() {
        let site = TestSite::start("image/png", b"\x89PNG\r\n\x1a\nnotreallyapngbutfine");
        let req = Request::builder().method("GET").uri(format!("/metadata/cover-proxy?url={}", urlencoding_minimal(&site.url()))).body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap().to_str().unwrap(), "image/png");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"\x89PNG\r\n\x1a\nnotreallyapngbutfine");
    }

    #[tokio::test]
    async fn cover_proxy_rejects_a_non_http_scheme() {
        let req = Request::builder().method("GET").uri("/metadata/cover-proxy?url=file:///etc/passwd").body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cover_proxy_rejects_a_link_local_address_like_cloud_metadata() {
        let req = Request::builder().method("GET").uri("/metadata/cover-proxy?url=http://169.254.169.254/latest/meta-data/").body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// Minimal query-string escaping sufficient for a `127.0.0.1:PORT`
    /// test URL (only `:` and `/` need encoding here) -- not a general
    /// URL-encoder, this crate already pulls in `url`'s own
    /// `form_urlencoded` for real request-building code, this is just
    /// test glue.
    fn urlencoding_minimal(s: &str) -> String {
        s.replace(':', "%3A").replace('/', "%2F")
    }
}
