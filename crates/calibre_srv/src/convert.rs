//! Port of `old_src/src/calibre/srv/convert.py` (issue #429, part of
//! #60's tracking epic): server-side ebook format conversion, queued
//! as a background job and polled over HTTP.
//!
//! # Endpoints
//!
//! - `POST /conversion/start/{book_id}` -- body `{"input_fmt":
//!   "EPUB", "output_fmt": "MOBI"}`; copies the book's `input_fmt`
//!   format into a fresh temp dir and queues a real conversion job via
//!   [`calibre_ebooks::conversion::plumber::Plumber`] (the real,
//!   tested format-dispatch table issue #476 found and wired
//!   `calibre_conversion`'s own binary to, reused here for the same
//!   reason: it's the crate's one real conversion engine). Returns the
//!   bare job id, matching upstream's own `return job_id`.
//! - `GET`/`POST /conversion/status/{job_id}` -- polls the job;
//!   `?abort_job=1` best-effort aborts it (matching upstream's own
//!   `rd.query.get('abort_job')`). On completion, adds the converted
//!   file to the book via `Cache::add_format` and publishes a real
//!   [`crate::web_socket::ChangeEvent::FormatsAdded`] -- the first
//!   real trigger for that event variant in this crate (see
//!   `web_socket`'s own module doc, which explicitly disclosed it had
//!   no trigger yet).
//! - `GET /conversion/book-data/{book_id}` -- available input formats
//!   (the book's own formats, intersected with what
//!   [`calibre_ebooks::conversion::plumber::convert_to_oebbook`] can
//!   actually read) and output formats (via
//!   `calibre_conversion::config::get_sorted_output_formats`, the
//!   real, already-ported format-preference sort), plus title/authors.
//!
//! # Real, disclosed narrowing versus upstream
//!
//! - **No conversion options.** `Plumber::run` (issue #476) takes no
//!   options at all -- upstream's `queue_job` merges per-book saved
//!   option specifics, GUI recommendations, and profile settings into
//!   a real `OptionRecommendation` list passed to its own `Plumber`.
//!   This port accepts (and ignores) an `options` field in the start
//!   request body for forward compatibility, but nothing is actually
//!   applied. Wiring `calibre_conversion::config`'s already-real
//!   option-recommendation registry into `Plumber` is real, separate,
//!   disclosed follow-up work (#476's own disclosed gap).
//! - **No live progress percentage/message.** Upstream's `Plumber`
//!   takes a `report_progress` callback writing `percent:msg|||` lines
//!   to a status file this module's `conversion_status` tails.
//!   `calibre_ebooks::conversion::plumber::Plumber::run` has no
//!   progress-reporting hook -- `conversion_status` reports a fixed
//!   `{"running": true, "percent": 0.0, "msg": ""}` while a job is in
//!   flight instead, same shape, no live detail.
//! - **No captured log/traceback text on success**, and a failure's
//!   `traceback` is just the error's `Display` text (via `{e:#}`), not
//!   a real Python-style traceback -- there's no `Log`-capturing
//!   equivalent wired into `Plumber` either.
//! - **`book-data` omits `profiles`/`conversion_options`.** Upstream's
//!   `profiles()` needs `input_profiles()`/`output_profiles()` device
//!   profile registries this crate doesn't have; `conversion_options`
//!   needs the same live per-format option data `Plumber` doesn't
//!   support yet (previous bullet) -- both real, separate,
//!   deliberately deferred rather than faked.
//! - **Cleanup granularity.** Upstream deletes the copied-in source
//!   ebook as soon as the job itself finishes (`job_done`), before the
//!   client ever polls status; this port deletes the whole per-job
//!   temp dir (source copy + output file) together, once, when
//!   `conversion_status` observes the job is no longer running --
//!   same eventual cleanup, slightly different timing, no correctness
//!   difference (nothing else reads that temp dir in between).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::jobs::{JobId, JobStatus};
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

/// Formats [`calibre_ebooks::conversion::plumber::convert_to_oebbook`]
/// can actually read, matching its own real dispatch table exactly
/// (issue #476's own research) -- used to filter a book's own
/// `available_formats` down to ones that are real conversion *inputs*.
const READABLE_FORMATS: &[&str] = &[
    "EPUB", "MOBI", "AZW", "AZW3", "PRC", "HTML", "HTM", "XHTML", "TXT", "MD", "MARKDOWN", "TEXT", "TEXTILE", "DOCX", "CBZ", "ZIP", "FB2", "RB", "LIT", "SNB", "RTF", "PDF", "LRF", "TCR", "PDB", "ODT",
    "DJVU", "RECIPE", "CHM", "AZW4",
];

/// Formats `Plumber::write_output` can actually produce, matching its
/// own real dispatch table exactly (issue #476's own research) --
/// AZW3 is a real, disclosed gap (no dedicated AZW3 output plugin
/// exists anywhere in this crate yet, only AZW3 *input*).
const WRITABLE_FORMATS: &[&str] = &["EPUB", "DOCX", "MOBI", "AZW", "PRC", "RB", "LIT", "TXT", "SNB", "RTF", "PDF", "LRF", "OEB", "PDB", "ODT", "TCR"];

#[derive(Clone)]
struct ConversionJobMeta {
    book_id: i32,
    output_fmt: String,
    output_path: PathBuf,
    tdir: PathBuf,
}

/// Port of `conversion_jobs`: real per-job bookkeeping keyed directly
/// by `JobId` (unlike `render_endpoints::RenderJobRegistry`, there's
/// no content-hash dedup here -- upstream's own `queue_job` always
/// starts a fresh job on every `POST /conversion/start`, never reuses
/// one).
#[derive(Default)]
pub struct ConversionJobRegistry {
    jobs: Mutex<HashMap<JobId, ConversionJobMeta>>,
}

impl ConversionJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, job_id: JobId, meta: ConversionJobMeta) {
        self.jobs.lock().unwrap().insert(job_id, meta);
    }

    fn peek(&self, job_id: JobId) -> Option<ConversionJobMeta> {
        self.jobs.lock().unwrap().get(&job_id).cloned()
    }

    fn take(&self, job_id: JobId) -> Option<ConversionJobMeta> {
        self.jobs.lock().unwrap().remove(&job_id)
    }
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

#[derive(Debug, Deserialize)]
pub struct StartConversionBody {
    input_fmt: String,
    output_fmt: String,
    /// Accepted, not applied -- see this module's own doc.
    #[serde(default)]
    #[allow(dead_code)]
    options: Value,
}

/// `POST /conversion/start/{book_id}`. Port of `start_conversion`/
/// `queue_job`.
pub async fn start_conversion(State(state): State<AppState>, AxumPath(book_id): AxumPath<i32>, Json(body): Json<StartConversionBody>) -> Result<Json<Value>, ServerError> {
    let row = fetch_book_row(&state, book_id).await?;
    let input_fmt_lower = body.input_fmt.to_lowercase();
    let path_str = row.get(format!("fmt_{input_fmt_lower}")).and_then(|v| v.as_str()).ok_or_else(|| ServerError::NotFound(format!("No {input_fmt_lower} format for the book {book_id}")))?;
    let src_on_disk = PathBuf::from(path_str);
    let output_fmt_lower = body.output_fmt.to_lowercase();

    let tdir = tempfile::tempdir().map_err(|e| ServerError::InternalServerError(e.to_string()))?.keep();
    let src_path = tdir.join(format!("input.{input_fmt_lower}"));
    tokio::fs::copy(&src_on_disk, &src_path).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    let output_path = tdir.join(format!("output.{output_fmt_lower}"));

    let job_id = {
        let job_src = src_path.clone();
        let job_out = output_path.clone();
        state
            .jobs
            .start_job(move || async move {
                let result = tokio::task::spawn_blocking(move || calibre_ebooks::conversion::plumber::Plumber::new(&job_src, &job_out).run()).await;
                match result {
                    Ok(Ok(())) => Ok("ok".to_string()),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(join_err) => Err(join_err.to_string()),
                }
            })
            .await
    };

    state.conversion_jobs.insert(job_id, ConversionJobMeta { book_id, output_fmt: output_fmt_lower, output_path, tdir });
    Ok(Json(json!(job_id)))
}

/// `GET`/`POST /conversion/status/{job_id}`. Port of
/// `conversion_status`.
pub async fn conversion_status(State(state): State<AppState>, AxumPath(job_id): AxumPath<JobId>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ServerError> {
    let Some(meta) = state.conversion_jobs.peek(job_id) else {
        return Err(ServerError::NotFound(format!("No job with id: {job_id}")));
    };

    let status = state.jobs.status(job_id).await;
    if matches!(status, JobStatus::Waiting | JobStatus::Running) {
        if query.get("abort_job").is_some() {
            state.jobs.abort_job(job_id).await;
        }
        return Ok(Json(json!({"running": true, "percent": 0.0, "msg": ""})));
    }

    let meta = state.conversion_jobs.take(job_id).unwrap_or(meta);
    let (ok, was_aborted, traceback) = match &status {
        JobStatus::Finished { .. } => (true, false, String::new()),
        JobStatus::Failed { error, was_aborted } => (false, *was_aborted, error.clone()),
        _ => (false, false, "job status is no longer known".to_string()),
    };

    let mut ans = json!({"running": false, "ok": ok, "was_aborted": was_aborted, "traceback": traceback, "log": ""});
    if ok {
        let size = tokio::fs::metadata(&meta.output_path).await.map(|m| m.len()).unwrap_or(0);
        let added = tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            let output_path = meta.output_path.clone();
            let fmt = meta.output_fmt.clone();
            let book_id = meta.book_id;
            move || cache.add_format(book_id, &output_path, &fmt, true)
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        if added {
            web_socket::publish(&state, ChangeEvent::FormatsAdded { book_ids: vec![meta.book_id] });
        }
        ans["size"] = json!(size);
        ans["fmt"] = json!(meta.output_fmt);
    }

    let _ = tokio::fs::remove_dir_all(&meta.tdir).await;
    Ok(Json(ans))
}

/// `GET /conversion/book-data/{book_id}`. Port of `conversion_data`,
/// narrowed per this module's own doc (no `profiles`/
/// `conversion_options`). `?input_fmt=`/`?output_fmt=` move that
/// format to the front of the respective list when present, matching
/// upstream's own preferred-format handling.
pub async fn conversion_book_data(State(state): State<AppState>, AxumPath(book_id): AxumPath<i32>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ServerError> {
    let row = fetch_book_row(&state, book_id).await?;
    let available: Vec<String> = row["available_formats"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let mut input_formats: Vec<String> = available.into_iter().filter(|f| READABLE_FORMATS.contains(&f.as_str())).collect();
    if let Some(preferred) = query.get("input_fmt") {
        let preferred_upper = preferred.to_uppercase();
        if let Some(pos) = input_formats.iter().position(|f| f == &preferred_upper) {
            let f = input_formats.remove(pos);
            input_formats.insert(0, f);
        }
    }

    let writable: Vec<String> = WRITABLE_FORMATS.iter().map(|s| s.to_string()).collect();
    let output_formats = calibre_conversion::config::get_sorted_output_formats(query.get("output_fmt").map(String::as_str), &writable, None);

    Ok(Json(json!({
        "book_id": book_id,
        "title": row["title"],
        "authors": row["authors"],
        "input_formats": input_formats,
        "output_formats": output_formats,
    })))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn add_test_epub(dir: &std::path::Path, cache: &Cache, title: &str, content: &str) -> i32 {
        let source = dir.join(format!("{title}-src.epub"));
        let file = std::fs::File::create(&source).unwrap();
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
            format!(
                r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>{title}</dc:title>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.start_file("chap1.html", opts).unwrap();
        std::io::Write::write_all(&mut zip, format!("<html><body><p>{content}</p></body></html>").as_bytes()).unwrap();
        zip.finish().unwrap();

        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = add_test_epub(dir.path(), &cache, "Convert Test Book", "Hello, real conversion.");
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

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn poll_until_done(router: &axum::Router, job_id: i64) -> serde_json::Value {
        for _ in 0..200 {
            let (status, body) = get_json(router, &format!("/conversion/status/{job_id}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["running"] == false {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("conversion job never finished within the polling budget");
    }

    #[tokio::test]
    async fn a_real_epub_to_txt_conversion_round_trips_and_adds_the_new_format() {
        let (_dir, router, book_id) = test_app();
        let (status, job_id) = post_json(&router, &format!("/conversion/start/{book_id}"), serde_json::json!({"input_fmt": "epub", "output_fmt": "txt"})).await;
        assert_eq!(status, StatusCode::OK);
        let job_id = job_id.as_i64().unwrap();

        let result = poll_until_done(&router, job_id).await;
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["fmt"], "txt");
        assert!(result["size"].as_u64().unwrap() > 0);

        let (_status, data) = get_json(&router, &format!("/conversion/book-data/{book_id}")).await;
        let formats: Vec<String> = data["input_formats"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert!(formats.contains(&"TXT".to_string()), "converted TXT format should now be listed: {formats:?}");
    }

    #[tokio::test]
    async fn start_conversion_404s_for_a_missing_input_format() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(&router, &format!("/conversion/start/{book_id}"), serde_json::json!({"input_fmt": "pdf", "output_fmt": "txt"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn start_conversion_404s_for_an_unknown_book() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(&router, "/conversion/start/999999", serde_json::json!({"input_fmt": "epub", "output_fmt": "txt"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn status_404s_for_an_unknown_job_id() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = get_json(&router, "/conversion/status/999999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn book_data_lists_the_books_own_epub_format_and_a_real_sorted_output_list() {
        let (_dir, router, book_id) = test_app();
        let (status, data) = get_json(&router, &format!("/conversion/book-data/{book_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(data["input_formats"], serde_json::json!(["EPUB"]));
        assert_eq!(data["title"], "Convert Test Book");
        let outputs = data["output_formats"].as_array().unwrap();
        assert!(outputs.iter().any(|v| v == "EPUB"));
        assert!(outputs.iter().any(|v| v == "MOBI"));
    }

    #[tokio::test]
    async fn a_polled_status_after_completion_reuses_no_job_id_a_second_time() {
        let (_dir, router, book_id) = test_app();
        let (_, job_id) = post_json(&router, &format!("/conversion/start/{book_id}"), serde_json::json!({"input_fmt": "epub", "output_fmt": "txt"})).await;
        let job_id = job_id.as_i64().unwrap();
        poll_until_done(&router, job_id).await;

        let (status, _) = get_json(&router, &format!("/conversion/status/{job_id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "a job's status should only be readable once, matching upstream's own one-shot pop");
    }
}
