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
use calibre_ebooks::scraper::Browser;

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
pub async fn search(State(state): State<AppState>, Json(body): Json<SearchBody>) -> Result<Json<Value>, ServerError> {
    if is_blank(&body.title) && is_blank(&body.authors) && is_blank(&body.isbn) {
        return Err(ServerError::BadRequest("at least one of title, authors, or isbn is required".to_string()));
    }

    let SearchBody { title, authors, isbn } = body;
    let (plugin_title, plugin_authors, plugin_isbn) = (title.clone(), authors.clone(), isbn.clone());

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

    // Installed third-party WASM metadata sources (#800) contribute
    // alongside the built-ins, under the same partial-failure rule.
    if let Some(store) = state.plugin_store.as_ref() {
        let plugin_query = calibre_plugins_wasm::metadata_source::MetadataQuery { title: plugin_title, authors: plugin_authors, isbn: plugin_isbn };
        let store = std::sync::Arc::clone(store);
        let (found, errs) = tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            let mut errs = Vec::new();
            run_wasm_metadata_plugins(&store, &plugin_query, &mut found, &mut errs);
            (found, errs)
        })
        .await
        .unwrap_or_else(|e| (Vec::new(), vec![format!("plugins: {e}")]));
        candidates.extend(found);
        source_errors.extend(errs);
    }

    if candidates.is_empty() && !source_errors.is_empty() {
        return Err(ServerError::FailedDependency(source_errors.join("; ")));
    }

    Ok(Json(json!({"candidates": candidates, "source_errors": source_errors})))
}

/// Converts a WASM plugin's wire-form candidate into the real
/// `MetadataCandidate` the rest of this server already speaks.
///
/// `calibre_plugins_wasm` deliberately mirrors the struct rather than
/// importing it -- it must not depend on `calibre_ebooks`, or a WASM
/// runtime would follow into every crate that does (see that crate's
/// own doc). This is the one place the two shapes meet, and
/// `every_metadata_candidate_field_survives_the_plugin_dto_round_trip`
/// pins them together so they cannot drift silently.
fn candidate_from_dto(dto: calibre_plugins_wasm::metadata_source::CandidateDto) -> MetadataCandidate {
    MetadataCandidate {
        source: dto.source,
        title: dto.title,
        authors: dto.authors,
        description: dto.description,
        publisher: dto.publisher,
        pubdate: dto.pubdate,
        tags: dto.tags,
        identifiers: dto.identifiers,
        language: dto.language,
        cover_url: dto.cover_url,
        rating: dto.rating,
    }
}

/// Runs every installed WASM metadata-source plugin, appending their
/// candidates and reporting any failures the same way a built-in
/// source's failure is reported.
///
/// A plugin that fails must not fail the request -- it is exactly the
/// partial-failure case `/metadata/search` already handles for its
/// built-in sources, and third-party code is if anything more likely
/// to break.
fn run_wasm_metadata_plugins(
    store: &calibre_plugins_wasm::PluginStore,
    query: &calibre_plugins_wasm::metadata_source::MetadataQuery,
    candidates: &mut Vec<MetadataCandidate>,
    source_errors: &mut Vec<String>,
) {
    let packages = match store.list() {
        Ok(p) => p,
        Err(e) => {
            source_errors.push(format!("plugins: {e}"));
            return;
        }
    };

    for pkg in packages {
        if pkg.manifest.plugin_type != calibre_plugins_wasm::PluginType::MetadataSource {
            continue;
        }
        let name = pkg.manifest.name.clone();
        match calibre_plugins_wasm::WasmMetadataSource::load(&pkg).and_then(|s| s.search(query)) {
            Ok(found) => candidates.extend(found.into_iter().map(candidate_from_dto)),
            Err(e) => source_errors.push(format!("{name}: {e}")),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CoverProxyParams {
    pub url: String,
}

/// Raster image types this route will actually proxy. Deliberately
/// excludes `image/svg+xml` (SVG can carry `<script>`/event-handler
/// XSS payloads) and anything non-image (a compromised or malicious
/// upstream host could otherwise get this *same-origin* route to hand
/// the browser `text/html` bytes it would then treat as same-origin
/// content -- see [`cover_proxy`]'s own doc for the full threat this
/// guards against).
const ALLOWED_COVER_CONTENT_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

fn is_allowed_cover_content_type(content_type: &str) -> bool {
    let base = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    ALLOWED_COVER_CONTENT_TYPES.contains(&base.as_str())
}

const MAX_COVER_REDIRECTS: u32 = 5;

enum HopOutcome {
    Redirect(String),
    Body(Vec<u8>, String),
}

/// Distinguishes a caller/policy problem (bad URL, non-http scheme,
/// blocked by the SSRF guard -- at the *initial* URL or at any
/// redirect hop) from a genuine upstream-fetch problem (non-2xx
/// status, malformed redirect, too many hops, network error) so
/// [`cover_proxy`] can map them to the right HTTP status (400 vs 424)
/// -- matching the status codes this route's own validation already
/// used before the redirect-chasing loop existed.
enum CoverFetchError {
    Invalid(String),
    Upstream(String),
}

impl std::fmt::Display for CoverFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoverFetchError::Invalid(msg) | CoverFetchError::Upstream(msg) => write!(f, "{msg}"),
        }
    }
}

/// Real, disclosed choice: this fetch does **not** use the shared
/// [`Browser`] (its underlying client follows redirects internally,
/// with no hook to re-check a redirect target's resolved IP before
/// following it -- a real SSRF bypass an attacker-controlled or
/// compromised upstream host could exploit by 302-redirecting to an
/// internal address after the *initial* host already passed
/// [`crate::net_guard::resolve_and_check`]). Each hop here is fetched
/// with redirects disabled and re-validated against the same guard
/// before being followed, closing that gap.
async fn fetch_cover_with_revalidated_redirects(start_url: &str) -> Result<(Vec<u8>, String), CoverFetchError> {
    let mut current = start_url.to_string();
    for _ in 0..=MAX_COVER_REDIRECTS {
        let parsed = url::Url::parse(&current).map_err(|e| CoverFetchError::Invalid(format!("{current}: invalid URL ({e})")))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(CoverFetchError::Invalid(format!("{current}: only http/https URLs are allowed")));
        }
        let host = parsed.host_str().ok_or_else(|| CoverFetchError::Invalid(format!("{current}: no host")))?.to_string();
        let port = parsed.port_or_known_default().unwrap_or(443);
        crate::net_guard::resolve_and_check(&host, port).await.map_err(|e| CoverFetchError::Invalid(format!("{current}: {e}")))?;

        let hop_url = current.clone();
        let outcome = tokio::task::spawn_blocking(move || -> Result<HopOutcome, String> {
            let client = reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .get(&hop_url)
                .header(reqwest::header::USER_AGENT, calibre_utils::random_ua::random_common_chrome_user_agent())
                .send()
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if status.is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("{hop_url}: redirect with no Location header"))?;
                return Ok(HopOutcome::Redirect(location));
            }
            if !status.is_success() {
                return Err(format!("upstream returned HTTP {}", status.as_u16()));
            }
            let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream").to_string();
            let bytes = resp.bytes().map_err(|e| e.to_string())?.to_vec();
            Ok(HopOutcome::Body(bytes, content_type))
        })
        .await
        .map_err(|e| CoverFetchError::Upstream(e.to_string()))?
        .map_err(CoverFetchError::Upstream)?;

        match outcome {
            HopOutcome::Redirect(location) => {
                let next = parsed.join(&location).map_err(|e| CoverFetchError::Upstream(format!("{current}: bad redirect location {location:?} ({e})")))?;
                current = next.to_string();
            }
            HopOutcome::Body(bytes, content_type) => return Ok((bytes, content_type)),
        }
    }
    Err(CoverFetchError::Upstream(format!("{start_url}: too many redirects")))
}

/// `GET /metadata/cover-proxy?url=...`. Fetches a candidate's cover
/// image server-side and streams it back. Same-origin by construction
/// (it's this server's own route), which is exactly why the response
/// `Content-Type` can't be trusted blindly from upstream: a response
/// this route serves is treated by the browser as coming from *this*
/// app's origin, not the external image host's -- so an upstream host
/// returning e.g. `text/html` with a script payload here would be a
/// same-origin XSS vector if passed through unchecked. See
/// [`ALLOWED_COVER_CONTENT_TYPES`] and the redirect re-validation in
/// [`fetch_cover_with_revalidated_redirects`].
pub async fn cover_proxy(Query(params): Query<CoverProxyParams>) -> Result<Response, ServerError> {
    let (bytes, content_type) = fetch_cover_with_revalidated_redirects(&params.url).await.map_err(|e| match e {
        CoverFetchError::Invalid(msg) => ServerError::BadRequest(msg),
        CoverFetchError::Upstream(msg) => ServerError::FailedDependency(msg),
    })?;

    if !is_allowed_cover_content_type(&content_type) {
        return Err(ServerError::FailedDependency(format!("upstream returned a non-image content type: {content_type}")));
    }

    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline; filename=\"cover\""));
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    /// One route's canned response, for [`TestSite`].
    enum TestRoute {
        Ok(&'static str, &'static [u8]),
        /// A 302 to an absolute URL (may point off-site -- e.g. to a
        /// blocked address, to exercise the redirect-revalidation
        /// guard without a second real listener).
        Redirect(String),
    }

    /// A tiny multi-route HTTP/1.1 test server -- extends the source
    /// clients' own private single-route `TestSite` helpers (not
    /// shared across crates' `#[cfg(test)]` modules) with redirect
    /// support, needed to test [`super::fetch_cover_with_revalidated_redirects`]'s
    /// own multi-hop behavior.
    struct TestSite {
        addr: std::net::SocketAddr,
    }

    impl TestSite {
        fn start(routes: HashMap<&'static str, TestRoute>) -> TestSite {
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
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut l = String::new();
                        if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                    }
                    match routes.get(path.as_str()) {
                        Some(TestRoute::Ok(content_type, body)) => {
                            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.write_all(body);
                        }
                        Some(TestRoute::Redirect(location)) => {
                            let resp = format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            let _ = stream.write_all(resp.as_bytes());
                        }
                        None => {
                            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                }
            });
            TestSite { addr }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
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
            tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
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

    #[test]
    fn every_metadata_candidate_field_survives_the_plugin_dto_round_trip() {
        // `calibre_plugins_wasm::CandidateDto` mirrors
        // `MetadataCandidate` rather than importing it (that crate must
        // not depend on calibre_ebooks, or wasmtime follows into every
        // crate that does). This pins the two shapes together: if a
        // field is added to one and not the other, this fails rather
        // than silently dropping plugin-supplied data.
        let dto = calibre_plugins_wasm::metadata_source::CandidateDto {
            source: "A Plugin".to_string(),
            title: Some("Dune".to_string()),
            authors: vec!["Frank Herbert".to_string()],
            description: Some("desc".to_string()),
            publisher: Some("Ace".to_string()),
            pubdate: Some("1965".to_string()),
            tags: vec!["Fiction".to_string()],
            identifiers: std::collections::BTreeMap::from([("isbn".to_string(), "9780441013593".to_string())]),
            language: Some("en".to_string()),
            cover_url: Some("https://example.com/c.jpg".to_string()),
            rating: Some(4.5),
        };

        let candidate = super::candidate_from_dto(dto.clone());

        assert_eq!(candidate.source, dto.source);
        assert_eq!(candidate.title, dto.title);
        assert_eq!(candidate.authors, dto.authors);
        assert_eq!(candidate.description, dto.description);
        assert_eq!(candidate.publisher, dto.publisher);
        assert_eq!(candidate.pubdate, dto.pubdate);
        assert_eq!(candidate.tags, dto.tags);
        assert_eq!(candidate.identifiers, dto.identifiers);
        assert_eq!(candidate.language, dto.language);
        assert_eq!(candidate.cover_url, dto.cover_url);
        assert_eq!(candidate.rating, dto.rating);

        // Field-count guard: serializing both to JSON must produce the
        // same key set, so an added field on either side is caught even
        // if someone forgets to extend the assertions above.
        let dto_keys: std::collections::BTreeSet<String> =
            serde_json::to_value(&dto).unwrap().as_object().unwrap().keys().cloned().collect();
        let candidate_keys: std::collections::BTreeSet<String> =
            serde_json::to_value(&candidate).unwrap().as_object().unwrap().keys().cloned().collect();
        assert_eq!(dto_keys, candidate_keys, "CandidateDto and MetadataCandidate have drifted apart");
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
        let site = TestSite::start(HashMap::from([("/cover.jpg", TestRoute::Ok("image/png", b"\x89PNG\r\n\x1a\nnotreallyapngbutfine"))]));
        let req = Request::builder().method("GET").uri(format!("/metadata/cover-proxy?url={}", urlencoding_minimal(&site.url("/cover.jpg")))).body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap().to_str().unwrap(), "image/png");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"\x89PNG\r\n\x1a\nnotreallyapngbutfine");
    }

    #[tokio::test]
    async fn cover_proxy_follows_a_real_redirect_to_a_legitimate_target() {
        let site = TestSite::start(HashMap::from([
            ("/redirect", TestRoute::Redirect("/final.jpg".to_string())),
            ("/final.jpg", TestRoute::Ok("image/jpeg", b"realjpegbytes")),
        ]));
        let req = Request::builder().method("GET").uri(format!("/metadata/cover-proxy?url={}", urlencoding_minimal(&site.url("/redirect")))).body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"realjpegbytes");
    }

    #[tokio::test]
    async fn cover_proxy_rejects_a_redirect_that_targets_a_blocked_address() {
        // The classic SSRF-via-redirect bypass this route's own
        // redirect-revalidation loop exists to close: the *initial*
        // URL (this TestSite, loopback) passes the guard, but its
        // Location header points at a link-local/cloud-metadata
        // address -- the second hop must be re-validated and
        // rejected, not blindly followed.
        let site = TestSite::start(HashMap::from([("/redirect", TestRoute::Redirect("http://169.254.169.254/latest/meta-data/".to_string()))]));
        let req = Request::builder().method("GET").uri(format!("/metadata/cover-proxy?url={}", urlencoding_minimal(&site.url("/redirect")))).body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cover_proxy_rejects_a_non_image_content_type() {
        // A compromised/malicious upstream returning `text/html` here
        // would otherwise be a same-origin XSS vector (this route's
        // response is served from *this* app's own origin) -- must be
        // rejected, not passed through.
        let site = TestSite::start(HashMap::from([("/evil", TestRoute::Ok("text/html", b"<script>alert(1)</script>"))]));
        let req = Request::builder().method("GET").uri(format!("/metadata/cover-proxy?url={}", urlencoding_minimal(&site.url("/evil")))).body(Body::empty()).unwrap();
        let (_dir, router) = test_app();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY);
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
