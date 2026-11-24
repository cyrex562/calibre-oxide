//! `POST /check-library/{library_id}` -- a real, **new** route (issue
//! #748), not a port (real upstream's "Check Library" dialog is Qt
//! GUI-only, never exposed over HTTP). Runs
//! `lexmechanic_db::check_library::CheckLibrary`'s real integrity scan
//! against a live library and returns every finding as structured
//! JSON.
//!
//! `CheckLibrary` was originally written against `lexmechanic_db::Library`
//! (the pre-`Cache` API) -- `lexmechanic_srv` only ever holds a `Cache`,
//! and unlike `legacy::LegacyDb` (issue #749's own finding), `Library`
//! and `Cache` are sibling structs that each independently own their
//! own connection, not wrapper/wrapped, so this needed a real port
//! (`Cache::get_book`/`all_authors`/`format_files`/`is_case_sensitive`,
//! `CheckLibrary` itself now taking `&Cache`) rather than a one-line
//! delegation. See that module's own doc for the details, including a
//! real pre-existing false-positive found and fixed along the way
//! (WAL-mode SQLite sidecar files were reported as "invalid authors").

use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

fn encode(findings: &[lexmechanic_db::library_check::Finding]) -> Value {
    json!(findings.iter().map(|f| json!({"book_id": f.book_id, "title": f.title, "path": f.path})).collect::<Vec<_>>())
}

/// The check's report, in the shape `POST /check-library` has always returned.
fn report_json(check: &lexmechanic_db::library_check::LibraryCheck) -> Value {
    json!({
        "missing_files": encode(&check.missing_files),
        "untracked_files": encode(&check.untracked_files),
        "moved_files": encode(&check.moved_files),
        "changed_files": encode(&check.changed_files),
        "missing_covers": encode(&check.missing_covers),
        "unreadable_folders": encode(&check.unreadable_folders),
        "orphaned_books": encode(&check.orphaned_books),
        "ignored_files": encode(&check.ignored_files),
        "nested_libraries": encode(&check.nested_libraries),
        // Whether the scan saw the whole library. False means
        // `missing_files` was not computed at all, and the UI has to
        // say so rather than implying nothing is missing.
        "conclusive": check.conclusive,
        "needs_attention": check.needs_attention(),
        "cancelled": false,
    })
}

/// The body of a [`crate::library_jobs::JobKind::CheckLibrary`] job.
///
/// `library_check` rather than the `check_library` port (#897). That port
/// recognised book folders by calibre's `Title (id)` naming, which this
/// project never produced, and reported every edited file as a corrupted one
/// -- so half its answers were unreliable and the other half cried wolf. See
/// its module docs.
pub fn run_job(cache: &lexmechanic_db::cache::Cache, control: &lexmechanic_db::scan_pipeline::JobControl) -> anyhow::Result<Value> {
    match lexmechanic_db::library_check::check_controlled(cache, control)? {
        Some(check) => Ok(report_json(&check)),
        // Cancelled part-way: nothing was concluded, and the report says so.
        None => {
            let mut empty = report_json(&lexmechanic_db::library_check::LibraryCheck::default());
            empty["cancelled"] = json!(true);
            Ok(empty)
        }
    }
}

/// `POST /check-library/{library_id}` -- starts the check as a library job (it
/// walks the whole folder: minutes on a large library, so never one request).
/// Answers 202 with the job's id; poll `GET /library-jobs/{id}` for progress and
/// the report, `POST /library-jobs/{id}/cancel` to stop it. `?wait=true` holds
/// the response until the job ends and answers with the report itself.
pub async fn check(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<crate::library_jobs::StartQuery>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    Ok(crate::library_jobs::respond(cache, crate::library_jobs::JobKind::CheckLibrary, &query).await)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use lexmechanic_db::cache::Cache;

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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn get_with(router: &axum::Router, uri: &str, accept_encoding: Option<&str>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut builder = Request::builder().uri(uri);
        if let Some(enc) = accept_encoding {
            builder = builder.header("accept-encoding", enc);
        }
        let resp = router.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        (status, headers, to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
    }

    async fn get_json(router: &axum::Router, uri: &str) -> serde_json::Value {
        serde_json::from_slice(&get_with(router, uri, None).await.2).unwrap_or(serde_json::Value::Null)
    }

    /// The check is a library job: 202 and an id at once, progress and the report on the job, and the
    /// job list says there is a report without carrying it.
    #[tokio::test]
    async fn the_check_is_a_job_with_a_report_the_list_does_not_carry() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("dropped in.epub"), b"epub bytes").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        filetime::set_file_mtime(dir.path().join("dropped in.epub"), filetime::FileTime::from_system_time(old)).ok();

        let (status, started) = post_json(&router, "/check-library/default").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        assert_eq!(started["kind"], "check_library");
        let url = started["status_url"].as_str().unwrap().to_string();
        let mut job = serde_json::Value::Null;
        for _ in 0..400 {
            job = get_json(&router, &url).await;
            if job["state"] != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(job["state"], "finished", "{job}");
        assert!(job["report"]["untracked_files"].as_array().unwrap().iter().any(|v| v["path"] == "dropped in.epub"), "{job}");
        assert_eq!(job["report"]["cancelled"], false);
        assert_eq!(job["progress"]["phase"], "assembling");
        assert!(job["progress"]["timings_ms"]["walk"].is_number(), "{job}");

        let listed = get_json(&router, "/library-jobs").await;
        let entry = listed["jobs"].as_array().unwrap().iter().find(|j| j["id"] == job["id"]).unwrap();
        assert!(entry["report"].is_null(), "the list carries no report: {entry}");
        assert_eq!(entry["has_report"], true);
        // ...and a request for the reports gets them.
        let with_reports = get_json(&router, "/library-jobs?reports=true").await;
        assert!(with_reports["jobs"].as_array().unwrap().iter().any(|j| j["report"]["untracked_files"].is_array()));
    }

    /// Asking again while a check runs follows it, as for every library job.
    #[tokio::test]
    async fn a_second_request_while_one_runs_is_the_same_job() {
        let (_dir, router) = test_app();
        let (_, first) = post_json(&router, "/check-library/default").await;
        let (_, second) = post_json(&router, "/check-library/default").await;
        // Either the first was already done (a new job) or the second joined it.
        if second["already_running"] == true {
            assert_eq!(first["job_id"], second["job_id"]);
        }
    }

    /// JSON over a kilobyte is gzipped for a client that takes it, and only then; a small
    /// answer is left alone.
    #[tokio::test]
    async fn big_json_is_gzipped_for_a_client_that_accepts_it() {
        let (dir, router) = test_app();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        for i in 0..60 {
            let path = dir.path().join(format!("a rather long file name to make the report big {i:03}.epub"));
            std::fs::write(&path, format!("epub {i}")).unwrap();
            filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(old)).ok();
        }
        let (status, headers, packed) = {
            let req = Request::builder().method("POST").uri("/check-library/default?wait=true").header("accept-encoding", "gzip").body(Body::empty()).unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            let (status, headers) = (resp.status(), resp.headers().clone());
            (status, headers, to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
        };
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers.get("content-encoding").map(|v| v.to_str().unwrap()), Some("gzip"), "{headers:?}");
        assert_eq!(&packed[..2], &[0x1f, 0x8b], "a gzip stream");

        // Not asked for: not sent.
        let (_, plain_headers, plain) = {
            let req = Request::builder().method("POST").uri("/check-library/default?wait=true").body(Body::empty()).unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            let headers = resp.headers().clone();
            (resp.status(), headers, to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
        };
        assert!(plain_headers.get("content-encoding").is_none());
        assert!(plain.len() > 1024 && packed.len() < plain.len() / 2, "{} gzipped against {}", packed.len(), plain.len());
        assert!(serde_json::from_slice::<serde_json::Value>(&plain).is_ok());

        // A small answer is not worth compressing.
        let (_, small_headers, _) = get_with(&router, "/prepare-library/default", Some("gzip")).await;
        assert!(small_headers.get("content-encoding").is_none(), "{small_headers:?}");
    }

    #[tokio::test]
    async fn a_real_clean_library_reports_no_findings() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/check-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["missing_files"], serde_json::json!([]));
        assert_eq!(body["untracked_files"], serde_json::json!([]));
        assert_eq!(body["needs_attention"], false);
        assert_eq!(body["conclusive"], true);
    }

    /// A file that is not a book is not a library problem.
    ///
    /// The old check reported any unrecognised top-level entry as an
    /// "invalid author" -- because in calibre's layout a top-level
    /// directory *is* an author. A stray `.bin` being described that way
    /// is a good illustration of why the categories had to change (#897).
    #[tokio::test]
    async fn a_file_that_is_not_a_book_is_not_reported() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("not_a_real_calibre_file.bin"), b"junk").unwrap();

        let (status, body) = post_json(&router, "/check-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["untracked_files"], serde_json::json!([]), "{body}");
        assert_eq!(body["needs_attention"], false, "{body}");
    }

    /// A book file the library does not know about *is* worth reporting.
    #[tokio::test]
    async fn an_unclaimed_book_file_is_reported_as_untracked() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("dropped in.epub"), b"epub bytes").unwrap();
        // Past the scanner's settle window, which exists so a file still
        // being copied is not indexed half-written.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        filetime::set_file_mtime(dir.path().join("dropped in.epub"), filetime::FileTime::from_system_time(old)).ok();

        let (status, body) = post_json(&router, "/check-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let untracked = body["untracked_files"].as_array().unwrap();
        assert!(untracked.iter().any(|v| v["path"] == "dropped in.epub"), "{body}");
    }

    #[tokio::test]
    async fn check_404s_for_an_unknown_library_id() {
        // Must use multi-library mode: in single-library mode
        // (`AppState::libraries == None`), `cache_for` always falls
        // back to the default library regardless of the requested
        // name -- an unknown name can only actually 404 once a real
        // `LibraryBroker` is involved (matches this project's own
        // established test pattern, e.g. cdb.rs).
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        Cache::new(src_dir.path()).unwrap();
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).unwrap();
        let state = crate::AppState {
            libraries: Some(broker),
            cache: default_cache,
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);

        let (status, _) = post_json(&router, "/check-library/no-such-library?wait=true").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
