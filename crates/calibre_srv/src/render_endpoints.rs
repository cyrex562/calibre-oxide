//! Port of `old_src/src/calibre/srv/books.py`'s `book_manifest`/
//! `book_file`/`queue_job`/`job_done` (issue #483, part of #427's
//! tracking epic): the HTTP surface tying the render pipeline
//! (`calibre_ebooks::render_book`, #481) and the disk cache
//! ([`crate::books_cache::BookCache`], #482) together, using
//! [`crate::jobs::JobsManager`] (#428) for the async render-job
//! queue/poll contract instead of upstream's forked-subprocess model
//! (the same disclosed substitution `jobs.rs` itself made).
//!
//! # Endpoints
//!
//! - `GET /book-manifest/{book_id}/{fmt}` -- cache hit returns the
//!   rendered manifest JSON merged with live DB metadata/read-
//!   positions/annotations; cache miss enqueues a render job (or
//!   reuses one already in flight for the same content hash) and
//!   returns a small job-status object for the client to poll by
//!   re-`GET`ing the same URL.
//! - `GET /book-file/{book_id}/{fmt}/{size}/{mtime}/{*name}` -- serves
//!   one rendered asset from the cache, `ETag`'d with the content
//!   hash itself (a natural strong ETag -- no separate hashing
//!   needed), with a real path-traversal guard.
//!
//! # `queued_jobs`/`failed_jobs` as [`RenderJobRegistry`]
//!
//! Upstream keeps two module-level dicts (`queued_jobs`,
//! `failed_jobs`) under a lock to dedupe in-flight renders and give a
//! cache-missing request one more look at a just-failed job's error
//! after [`crate::jobs::JobsManager`]'s own equivalent of `self.jobs`
//! has already forgotten it. [`RenderJobRegistry`] is the same idea as
//! a real, testable type instead of two free-floating globals.
//!
//! # Simplifications versus upstream's `queue_job`/`job_done`
//!
//! - **No staged copy of the input file.** Upstream copies the
//!   format's bytes into a staging file because the real render work
//!   runs in a *separate forked process* (`fork_job`) that can't just
//!   read the library's own open file handle. This port runs
//!   rendering as a `tokio` `spawn_blocking` task in the same process
//!   (see `jobs.rs`'s own disclosed substitution), so
//!   [`calibre_ebooks::render_book::render`] simply reads the format
//!   file directly from its real on-disk library path -- one less
//!   copy, no correctness difference (the read is never mutated).
//! - **`job_done`'s move-into-final-cache is folded into the job's own
//!   body** rather than a separate callback upstream registers with
//!   `ctx.start_job(..., job_done_callback=job_done, ...)` --
//!   [`JobsManager::start_job`] has no separate callback hook, so the
//!   staged-output-dir rename and [`RenderJobRegistry`] bookkeeping
//!   both happen at the end of the same `work` future instead.

use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Mutex;

use axum::body::Body;
use axum::extract::{Extension, Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthenticatedUser;
use crate::books::effective_user;
use crate::books_cache::{book_hash, MANIFEST_FILENAME};
use crate::errors::ServerError;
use crate::jobs::{JobId, JobStatus};
use crate::AppState;

/// Port of `queued_jobs`/`failed_jobs`, as a real type -- see this
/// module's own doc for why. Keyed by content hash (`bhash`), not
/// `JobId`, matching upstream (a client polls by re-fetching
/// `book_manifest`, which only knows the hash, not the job id, until
/// the first response tells it).
#[derive(Default)]
pub struct RenderJobRegistry {
    queued: Mutex<HashMap<String, JobId>>,
    failed: Mutex<HashMap<String, (bool, String)>>,
}

impl RenderJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn queued_get(&self, bhash: &str) -> Option<JobId> {
        self.queued.lock().unwrap().get(bhash).copied()
    }

    fn queued_insert(&self, bhash: String, job_id: JobId) {
        self.queued.lock().unwrap().insert(bhash, job_id);
    }

    fn queued_remove(&self, bhash: &str) {
        self.queued.lock().unwrap().remove(bhash);
    }

    /// One-shot: a request that observes a failure removes it, so the
    /// *next* request after that (once nothing tracks `bhash` at all
    /// any more) starts a fresh render attempt, matching upstream's
    /// own `failed_jobs.pop(bhash, None)`.
    fn failed_take(&self, bhash: &str) -> Option<(bool, String)> {
        self.failed.lock().unwrap().remove(bhash)
    }

    fn failed_insert(&self, bhash: String, entry: (bool, String)) {
        self.failed.lock().unwrap().insert(bhash, entry);
    }
}

/// Only formats [`calibre_ebooks::render_book::extract_book`] can
/// actually explode are viewable -- matches upstream's own
/// `plugin_for_input_format(fmt) is None` gate, just checked here
/// directly instead of through a generic plugin registry this crate
/// doesn't have.
fn is_viewable_format(fmt_lower: &str) -> bool {
    matches!(fmt_lower, "epub" | "kepub")
}

async fn fetch_book_row(state: &AppState, book_id: i32) -> Result<Value, ServerError> {
    let cache = state.cache.clone();
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        cache.get_data_as_dict(None, true, Some(&ids), false)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, "default"))
}

async fn queue_render_job(state: &AppState, ebook_path: PathBuf, bhash: String, size: i64, mtime: i64) -> JobId {
    let staging_out = state.book_cache.staging_dir().join(&bhash);
    let final_dir = state.book_cache.hash_dir(&bhash);
    let registry = state.render_jobs.clone();
    let bhash_for_job = bhash.clone();

    state
        .jobs
        .start_job(move || async move {
            let sb_out = staging_out.clone();
            let sb_hash = bhash_for_job.clone();
            let render_result = tokio::task::spawn_blocking(move || -> Result<calibre_ebooks::render_book::BookRenderData, String> {
                crate::books_cache::safe_remove(&sb_out);
                std::fs::create_dir_all(&sb_out).map_err(|e| e.to_string())?;
                calibre_ebooks::render_book::render(&ebook_path, &sb_out, Some((&sb_hash, size, mtime)), true).map_err(|e| format!("{e:#}"))
            })
            .await;

            match render_result {
                Ok(Ok(_data)) => {
                    crate::books_cache::safe_remove(&final_dir);
                    let move_result = crate::books_cache::rename_with_retry(&staging_out, &final_dir);
                    registry.queued_remove(&bhash_for_job);
                    match move_result {
                        Ok(()) => Ok("ok".to_string()),
                        Err(e) => {
                            registry.failed_insert(bhash_for_job.clone(), (false, e.to_string()));
                            Err(e.to_string())
                        }
                    }
                }
                Ok(Err(e)) => {
                    crate::books_cache::safe_remove(&staging_out);
                    registry.failed_insert(bhash_for_job.clone(), (false, e.clone()));
                    registry.queued_remove(&bhash_for_job);
                    Err(e)
                }
                Err(join_err) => {
                    crate::books_cache::safe_remove(&staging_out);
                    let aborted = join_err.is_cancelled();
                    let msg = join_err.to_string();
                    registry.failed_insert(bhash_for_job.clone(), (aborted, msg.clone()));
                    registry.queued_remove(&bhash_for_job);
                    Err(msg)
                }
            }
        })
        .await
}

fn job_status_json(status: JobStatus, job_id: JobId) -> Value {
    let (job_status, traceback, aborted) = match status {
        JobStatus::Waiting => ("waiting", None, false),
        JobStatus::Running => ("running", None, false),
        JobStatus::Finished { .. } => ("finished", None, false),
        JobStatus::Failed { error, was_aborted } => ("failed", Some(error), was_aborted),
        JobStatus::Unknown => ("unknown", None, false),
    };
    json!({"aborted": aborted, "traceback": traceback, "job_status": job_status, "job_id": job_id})
}

/// `GET /book-manifest/{book_id}/{fmt}`. Port of `book_manifest`.
/// `?force_reload=1` deletes any existing cache entry's manifest
/// first, forcing a fresh render (matches upstream's own
/// `rd.query.get('force_reload') == '1'`).
pub async fn book_manifest(
    State(state): State<AppState>,
    AxumPath((book_id, fmt)): AxumPath<(i32, String)>,
    Query(query): Query<HashMap<String, String>>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<Json<Value>, ServerError> {
    let fmt_lower = fmt.to_lowercase();
    if !is_viewable_format(&fmt_lower) {
        return Err(ServerError::NotFound(format!("The format {} cannot be viewed", fmt.to_uppercase())));
    }

    let row = fetch_book_row(&state, book_id).await?;
    let path_str = row.get(format!("fmt_{fmt_lower}")).and_then(|v| v.as_str()).ok_or_else(|| ServerError::NotFound(format!("No {fmt_lower} format for the book {book_id}")))?;
    let path = PathBuf::from(path_str);
    let meta = tokio::fs::metadata(&path).await.map_err(|_| ServerError::NotFound(format!("No {fmt_lower} format for the book {book_id}")))?;
    let size = meta.len() as i64;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    let library_id = state.cache.library_id();
    let bhash = book_hash(&library_id, book_id, &fmt_lower, size, mtime);

    let force_reload = query.get("force_reload").map(|v| v == "1").unwrap_or(false);
    let manifest_path = state.book_cache.hash_dir(&bhash).join(MANIFEST_FILENAME);
    if force_reload {
        crate::books_cache::safe_remove(&state.book_cache.hash_dir(&bhash));
    }

    let is_authenticated = user.is_some();
    let user_name = effective_user(user);

    match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => {
            let _ = filetime::set_file_mtime(&manifest_path, filetime::FileTime::now());
            let mut ans: Value = serde_json::from_slice(&bytes).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            ans["metadata"] = crate::ajax::book_json(&row);
            if is_authenticated {
                let positions = tokio::task::spawn_blocking({
                    let cache = state.cache.clone();
                    let fmt = fmt_lower.clone();
                    let user_name = user_name.clone();
                    move || cache.get_last_read_positions(book_id, &fmt, &user_name)
                })
                .await
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
                ans["last_read_positions"] = Value::Array(positions);

                let annotations_map = tokio::task::spawn_blocking({
                    let cache = state.cache.clone();
                    let fmt = fmt_lower.clone();
                    let user_name = user_name.clone();
                    move || calibre_db::annotations::annotations_map_for_book(&cache, book_id, &fmt, "web", &user_name)
                })
                .await
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
                ans["annotations_map"] = json!(annotations_map);
            }
            Ok(Json(ans))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some((aborted, traceback)) = state.render_jobs.failed_take(&bhash) {
                return Ok(Json(json!({"aborted": aborted, "traceback": traceback, "job_status": "finished"})));
            }
            let job_id = match state.render_jobs.queued_get(&bhash) {
                Some(id) => id,
                None => {
                    let id = queue_render_job(&state, path, bhash.clone(), size, mtime).await;
                    state.render_jobs.queued_insert(bhash.clone(), id);
                    id
                }
            };
            let status = state.jobs.status(job_id).await;
            Ok(Json(job_status_json(status, job_id)))
        }
        Err(e) => Err(ServerError::InternalServerError(e.to_string())),
    }
}

/// Rejects any path segment that could escape the cache's own hash
/// directory once joined -- `..`/`.`/an absolute segment. Needed
/// because axum's `{*name}` wildcard passes a decoded, `../`-laden
/// segment through verbatim, and neither `PathBuf::join` nor
/// `Path::starts_with` collapse `..` components the way a real
/// filesystem `canonicalize` (or Python's `os.path.abspath`) would --
/// see `books_cache::abspath`'s own doc for the same caveat.
fn reject_traversal(name: &str) -> Result<(), ServerError> {
    let bad = StdPath::new(name).components().any(|c| !matches!(c, std::path::Component::Normal(_)));
    if bad || name.is_empty() {
        return Err(ServerError::NotFound("Not found".to_string()));
    }
    Ok(())
}

/// `GET /book-file/{book_id}/{fmt}/{size}/{mtime}/{*name}`. Port of
/// `book_file`. `size`/`mtime` are recomputed into the same
/// [`book_hash`] the URL itself encodes -- **not** re-read from the
/// DB -- so a stale/tampered URL simply misses the cache rather than
/// serving the wrong content (matches upstream exactly).
pub async fn book_file(State(state): State<AppState>, AxumPath((book_id, fmt, size, mtime, name)): AxumPath<(i32, String, i64, i64, String)>, headers: HeaderMap) -> Result<Response, ServerError> {
    let title_exists = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.field_for(book_id, "title")
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .is_some();
    if !title_exists {
        return Err(ServerError::book_not_found(book_id, "default"));
    }

    reject_traversal(&name)?;

    let fmt_lower = fmt.to_lowercase();
    let library_id = state.cache.library_id();
    let bhash = book_hash(&library_id, book_id, &fmt_lower, size, mtime);

    let base = crate::books_cache::abspath(&state.book_cache.final_dir()).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    let requested = base.join(&bhash).join(&name);
    let resolved = crate::books_cache::abspath(&requested).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    if !resolved.starts_with(&base) {
        return Err(ServerError::NotFound("Not found".to_string()));
    }

    let etag_value = format!("\"{bhash}\"");
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH) {
        if if_none_match.to_str().map(|v| v == etag_value || v == bhash).unwrap_or(false) {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::NOT_MODIFIED;
            resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag_value).unwrap());
            return Ok(resp);
        }
    }

    let bytes = tokio::fs::read(&resolved).await.map_err(|_| ServerError::NotFound("Not found".to_string()))?;
    let mime = mime_guess::from_path(&resolved).first_raw().unwrap_or("application/octet-stream");
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag_value).unwrap());
    Ok(resp.into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{header, Request, StatusCode};
    use axum::response::Response;
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    use super::*;

    fn write_test_epub_zip(path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file("mimetype", opts).unwrap();
        std::io::Write::write_all(&mut zip, b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        std::io::Write::write_all(
            &mut zip,
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        zip.start_file("content.opf", opts).unwrap();
        std::io::Write::write_all(
            &mut zip,
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Render Endpoint Test Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        zip.start_file("chap1.xhtml", opts).unwrap();
        std::io::Write::write_all(&mut zip, br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Chapter One</h1><p>Hello.</p></body></html>"#).unwrap();
        zip.finish().unwrap();
    }

    fn add_test_epub(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}-src.epub"));
        write_test_epub_zip(&source);
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = add_test_epub(dir.path(), &cache, "Book 0");
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
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
    }

    async fn get_raw(router: &axum::Router, uri: &str) -> Response {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        router.clone().oneshot(req).await.unwrap()
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = get_raw(router, uri).await;
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap_or(Value::Null) };
        (status, value)
    }

    async fn poll_manifest_until_done(router: &axum::Router, uri: &str) -> Value {
        for _ in 0..200 {
            let (status, body) = get_json(router, uri).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body.get("job_status").is_none() {
                return body;
            }
            if body["job_status"] == "failed" {
                panic!("render job failed: {body}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("manifest never became ready within the polling budget");
    }

    #[tokio::test]
    async fn book_manifest_404s_for_an_unviewable_format() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = get_json(&router, &format!("/book-manifest/{book_id}/pdf")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn book_manifest_404s_for_an_unknown_book() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = get_json(&router, "/book-manifest/999999/epub").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_cache_miss_queues_a_job_and_polling_eventually_returns_a_real_manifest() {
        let (_dir, router, book_id) = test_app();
        let uri = format!("/book-manifest/{book_id}/epub");

        let (status, first) = get_json(&router, &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert!(first.get("job_status").is_some(), "first response should be a job-status object: {first}");

        let manifest = poll_manifest_until_done(&router, &uri).await;
        assert_eq!(manifest["spine"], json!(["chap1.xhtml"]));
        assert_eq!(manifest["metadata"]["title"], "Book 0");
        // book_hash must be the real {hash,size,mtime} object shape the
        // actual read_book client expects (`manifest.book_hash.size`/
        // `.mtime` are used to build book-file URLs) -- a bare string
        // here is a real bug, not a narrower-but-valid simplification.
        assert!(manifest["book_hash"]["hash"].is_string(), "{manifest}");
        assert!(manifest["book_hash"]["size"].is_i64(), "{manifest}");
        assert!(manifest["book_hash"]["mtime"].is_i64(), "{manifest}");
    }

    #[tokio::test]
    async fn a_second_request_while_the_first_is_still_rendering_reuses_the_same_job() {
        let (_dir, router, book_id) = test_app();
        let uri = format!("/book-manifest/{book_id}/epub");

        let (_, first) = get_json(&router, &uri).await;
        let (_, second) = get_json(&router, &uri).await;
        if first.get("job_id").is_some() && second.get("job_id").is_some() {
            assert_eq!(first["job_id"], second["job_id"], "concurrent cache-miss requests should share one render job");
        }
        poll_manifest_until_done(&router, &uri).await;
    }

    #[tokio::test]
    async fn force_reload_re_renders_even_when_already_cached() {
        let (_dir, router, book_id) = test_app();
        let uri = format!("/book-manifest/{book_id}/epub");
        poll_manifest_until_done(&router, &uri).await;

        let (status, forced) = get_json(&router, &format!("{uri}?force_reload=1")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(forced.get("job_status").is_some(), "force_reload should invalidate the cache hit and queue a fresh render: {forced}");
        poll_manifest_until_done(&router, &uri).await;
    }

    /// Recomputes the same `(size, mtime)` pair `book_manifest`/
    /// `book_file` derive from the format's real on-disk file, so
    /// tests can build a real `/book-file/...` URL without duplicating
    /// the handler's own hashing logic.
    fn format_size_mtime(dir: &std::path::Path, book_id: i32) -> (i64, i64) {
        let cache = calibre_db::cache::Cache::new(dir).unwrap();
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        let row = cache.get_data_as_dict(None, true, Some(&ids), false).unwrap().into_iter().next().unwrap();
        let path = row["fmt_epub"].as_str().unwrap();
        let meta = std::fs::metadata(path).unwrap();
        let size = meta.len() as i64;
        let mtime = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        (size, mtime)
    }

    #[tokio::test]
    async fn a_rendered_asset_can_be_fetched_and_returns_a_real_etag() {
        let (dir, router, book_id) = test_app();
        let manifest_uri = format!("/book-manifest/{book_id}/epub");
        poll_manifest_until_done(&router, &manifest_uri).await;

        let (size, mtime) = format_size_mtime(dir.path(), book_id);
        let asset_uri = format!("/book-file/{book_id}/epub/{size}/{mtime}/chap1.xhtml");
        let resp = get_raw(&router, &asset_uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let etag = resp.headers().get(header::ETAG).cloned();
        assert!(etag.is_some(), "a rendered asset should carry a real ETag");
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"n\""), "should be the reader's own serialized JSON tree: {text}");

        // A conditional GET with the same ETag is a real 304.
        let req = Request::builder().uri(&asset_uri).header(header::IF_NONE_MATCH, etag.unwrap()).body(Body::empty()).unwrap();
        let resp2 = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn a_stale_size_mtime_in_the_url_misses_the_cache_instead_of_serving_wrong_content() {
        let (dir, router, book_id) = test_app();
        let manifest_uri = format!("/book-manifest/{book_id}/epub");
        poll_manifest_until_done(&router, &manifest_uri).await;

        let (size, mtime) = format_size_mtime(dir.path(), book_id);
        let tampered_uri = format!("/book-file/{book_id}/epub/{size}/{}/chap1.xhtml", mtime + 1);
        let resp = get_raw(&router, &tampered_uri).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn book_file_rejects_a_path_traversal_attempt() {
        let (_dir, router, book_id) = test_app();
        let resp = get_raw(&router, &format!("/book-file/{book_id}/epub/0/0/../../../../etc/passwd")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn book_file_404s_for_an_unknown_book() {
        let (_dir, router, _book_id) = test_app();
        let resp = get_raw(&router, "/book-file/999999/epub/0/0/chap1.xhtml").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
