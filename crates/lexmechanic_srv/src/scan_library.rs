//! `POST /scan-library/{library_id}` -- reads the library folder and
//! brings the index up to date with it.
//!
//! The counterpart to [`crate::check_library`], which only *reports*.
//! This one writes: files nothing claims become books, and files the
//! user moved inside their own folder have their records corrected.
//! Nothing on disk is touched.
//!
//! Part of the tracked-in-place model (#889), where the folder is the
//! library and this route is how the folder's current contents become
//! the library's current contents.
//!
//! # Who calls it
//!
//! Not "when a library is opened", which an earlier version of this
//! comment claimed and the code never did: opening a folder creates the
//! database and does not look at the files in it. The callers are Check
//! Library's "add the untracked files" action, the desktop app's watcher
//! when the folder it watches is inside the library, and the prompt shown
//! when a library is opened over a folder with untracked book files: the app
//! asks `GET /library-untracked/{library_id}` (a count, no scan) and offers
//! to run this -- it never starts it by itself, since on a share it takes
//! long.
//!
//! # A job, not a request
//!
//! A scan of a share with hundreds of thousands of files runs for a long
//! time, so the route starts a job ([`crate::library_jobs`]) and answers
//! `202` with its id; progress and cancel are on `/library-jobs/{id}`.
//! `?wait=true` holds the response until the job ends and answers with the
//! final report -- the shape this route had when it was synchronous, for
//! callers that want exactly that (the desktop watcher, tests).
//!
//! The job has two stages. The first reads each new file once, for its
//! content hash, and registers it as a book titled from its filename --
//! books appear as batches commit. The second reads embedded metadata and
//! renders covers, filling in only what the user has not already changed.

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::errors::ServerError;
use crate::library_jobs::{respond, JobKind, StartQuery};
use crate::AppState;

/// `POST /scan-library/{library_id}`.
pub async fn scan(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    Ok(respond(cache, JobKind::Scan, &query).await)
}

#[derive(Debug, serde::Deserialize)]
pub struct UntrackedQuery {
    /// Stop counting at this many. The default is enough to say "lots".
    pub max: Option<usize>,
    /// Walk again even if a recent answer is remembered.
    pub fresh: Option<bool>,
}

/// How long a count is trusted when the library has not changed. Files appear
/// in the folder without the library knowing, so the answer is not kept for ever.
const UNTRACKED_TTL: std::time::Duration = std::time::Duration::from_secs(60);

type UntrackedKey = (std::path::PathBuf, usize);
type UntrackedEntry = (std::time::Instant, String, serde_json::Value);

/// The last answer per library and cap: `(when, change watermark, answer)`. The
/// walk is long on a big or remote library, and the page asks on every load.
fn untracked_memo() -> &'static std::sync::Mutex<std::collections::HashMap<UntrackedKey, UntrackedEntry>> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<UntrackedKey, UntrackedEntry>>> = std::sync::OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The count for `cache`, from the memo when the library has not changed and the
/// answer is recent; otherwise a (name-only) walk, which is then remembered.
pub(crate) fn untracked_cached(cache: &lexmechanic_db::cache::Cache, cap: usize, fresh: bool) -> anyhow::Result<serde_json::Value> {
    let key = (cache.backend.library_path.clone(), cap);
    // Read before walking: a change during the walk then makes the next
    // request walk again instead of trusting a count that may predate it.
    let watermark = cache.change_watermark();
    if !fresh {
        if let Some((at, mark, value)) = untracked_memo().lock().unwrap().get(&key) {
            if *mark == watermark && at.elapsed() < UNTRACKED_TTL {
                return Ok(value.clone());
            }
        }
    }
    let count = lexmechanic_db::scan::count_untracked(cache, std::time::SystemTime::now(), cap)?;
    let value = serde_json::to_value(count).unwrap_or_default();
    untracked_memo().lock().unwrap().insert(key, (std::time::Instant::now(), watermark, value.clone()));
    Ok(value)
}

/// `GET /library-untracked/{library_id}` -- how many book files in the
/// library folder nothing tracks yet, **without scanning**: one walk, no
/// reads, stopping at `max` (default 10,000; `at_least` then says so).
///
/// This is the question behind "This folder has N untracked book files --
/// scan now?". Opening a folder does not scan it, and on a share a scan takes
/// long, so the app asks first; this is what it asks with.
pub async fn untracked(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<UntrackedQuery>) -> Result<axum::Json<serde_json::Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let cap = query.max.unwrap_or(10_000).clamp(1, 1_000_000);
    let fresh = query.fresh.unwrap_or(false);
    let value = tokio::task::spawn_blocking(move || untracked_cached(&cache, cap, fresh))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?;
    Ok(axum::Json(value))
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
            reader_profiles: std::sync::Arc::new(
                crate::reader_profiles::ProfileStore::new_in_memory().unwrap(),
            ),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(
                4,
                std::time::Duration::from_secs(3600),
            )),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(
                crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap(),
            ),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(
                lexmechanic_customize::registry::PluginRegistry::new(),
            )),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, value)
    }

    /// Past the scanner's settle window, which exists so a file still
    /// being copied is not indexed half-written.
    fn make_settled(path: &std::path::Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(old)).unwrap();
    }

    /// The prompt behind "This folder has N untracked book files -- scan
    /// now?": the count comes from a walk, and nothing is added by asking.
    #[tokio::test]
    async fn the_untracked_count_is_reported_without_scanning() {
        let (dir, router) = test_app();
        let (status, body) = get_json(&router, "/library-untracked/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["untracked"], 0);

        std::fs::create_dir_all(dir.path().join("Shelf")).unwrap();
        for name in ["a.pdf", "b.epub", "Shelf/c.pdf"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"%PDF x").unwrap();
            make_settled(&path);
        }
        let (_, body) = get_json(&router, "/library-untracked/-?fresh=true").await;
        assert_eq!(body["untracked"], 3, "{body}");
        assert_eq!(body["tracked"], 0);
        assert_eq!(body["at_least"], false);
        assert_eq!(body["conclusive"], true);

        let (_, capped) = get_json(&router, "/library-untracked/-?max=2").await;
        assert_eq!(capped["at_least"], true, "{capped}");

        // Asking added nothing; scanning does, and then there is nothing left.
        let (_, ajax) = get_json(&router, "/ajax/book/1").await;
        assert!(ajax.is_null() || ajax.get("title").is_none(), "counting must not create books: {ajax}");
        let (_, scanned) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(scanned["added"].as_array().unwrap().len(), 3);
        let (_, after) = get_json(&router, "/library-untracked/-").await;
        assert_eq!(after["untracked"], 0);
        assert_eq!(after["tracked"], 3);
    }

    /// The page asks on every load and the walk is long on a big or remote library:
    /// a recent answer is reused while the library is unchanged, a change to the
    /// library or `fresh=true` walks again.
    #[tokio::test]
    async fn the_untracked_count_is_remembered_until_the_library_changes() {
        let (dir, router) = test_app();
        let (_, first) = get_json(&router, "/library-untracked/default").await;
        assert_eq!(first["untracked"], 0);

        let path = dir.path().join("late.pdf");
        std::fs::write(&path, b"%PDF x").unwrap();
        make_settled(&path);
        let (_, again) = get_json(&router, "/library-untracked/default").await;
        assert_eq!(again["untracked"], 0, "answered from the remembered count, no walk: {again}");
        let (_, fresh) = get_json(&router, "/library-untracked/default?fresh=true").await;
        assert_eq!(fresh["untracked"], 1, "{fresh}");

        // A change to the library (the scan adds the book) invalidates it by itself.
        let (_, _) = post_json(&router, "/scan-library/default?wait=true").await;
        let (_, after) = get_json(&router, "/library-untracked/default").await;
        assert_eq!(after["untracked"], 0, "{after}");
        assert_eq!(after["tracked"], 1, "{after}");
    }

    #[tokio::test]
    async fn an_empty_library_scans_to_nothing() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["added"], serde_json::json!([]));
        assert_eq!(body["relocated"], 0);
        assert_eq!(body["conclusive"], true);
    }

    /// The headline: a file dropped into the folder becomes a book,
    /// and stays exactly where it was put.
    #[tokio::test]
    async fn a_dropped_file_becomes_a_book_without_moving() {
        let (dir, router) = test_app();
        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        make_settled(&book);

        let (status, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["added"].as_array().unwrap().len(), 1, "{body}");
        assert!(book.exists(), "the scan must not move the file it indexed");
    }

    #[tokio::test]
    async fn scanning_twice_does_not_add_the_same_file_again() {
        let (dir, router) = test_app();
        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        make_settled(&book);

        post_json(&router, "/scan-library/default?wait=true").await;
        let (_, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(body["already_known"], 1, "{body}");
    }

    /// A file still being written is reported, not indexed.
    #[tokio::test]
    async fn a_settling_file_is_reported_rather_than_indexed() {
        let (dir, router) = test_app();
        std::fs::write(dir.path().join("half-copied.pdf"), b"%PDF").unwrap();

        let (_, body) = post_json(&router, "/scan-library/default?wait=true").await;
        assert_eq!(body["added"], serde_json::json!([]), "{body}");
        assert_eq!(
            body["settling"],
            serde_json::json!(["half-copied.pdf"]),
            "{body}"
        );
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// Polls a job until it is no longer running.
    async fn wait_for_job(router: &axum::Router, id: u64) -> serde_json::Value {
        for _ in 0..600 {
            let (_, job) = get_json(router, &format!("/library-jobs/{id}")).await;
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    /// Without `?wait`, the route starts a job and answers at once with
    /// its id; the status route then reports progress and the report.
    #[tokio::test]
    async fn a_scan_is_a_job_with_status_and_a_final_report() {
        let (dir, router) = test_app();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            let book = dir.path().join(name);
            std::fs::write(&book, format!("%PDF-1.4 {name}")).unwrap();
            make_settled(&book);
        }

        let (status, started) = post_json(&router, "/scan-library/default").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let id = started["job_id"].as_u64().expect("a job id");

        let job = wait_for_job(&router, id).await;
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["kind"], "scan");
        assert_eq!(job["report"]["added"].as_array().unwrap().len(), 3, "{job}");
        assert_eq!(job["progress"]["added"], 3, "{job}");
        assert_eq!(job["progress"]["hashed"], 3, "{job}");
        assert_eq!(job["progress"]["errors"], 0, "{job}");
        assert_eq!(job["report"]["enrichment"]["remaining"], 0, "{job}");
    }

    async fn json_call(router: &axum::Router, method: &str, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// Organize: dry run, execute only with the dry run's fingerprint, undo.
    #[tokio::test]
    async fn organize_is_a_dry_run_then_a_fingerprinted_job_then_undoable() {
        let (dir, router) = test_app();
        let book = dir.path().join("inbox").join("Alpha.pdf");
        std::fs::create_dir_all(book.parent().unwrap()).unwrap();
        std::fs::write(&book, b"%PDF alpha").unwrap();
        make_settled(&book);
        post_json(&router, "/scan-library/default?wait=true").await;
        let t = "Sorted/{title}";

        let (status, dry) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"template": t})).await;
        assert_eq!(status, StatusCode::OK, "{dry}");
        assert_eq!(dry["files"], 1, "{dry}");
        assert_eq!(dry["moves"][0]["to"], "Sorted/Alpha.pdf");
        assert!(book.exists(), "a dry run moves nothing");

        let (status, _) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"template": t, "dry_run": false})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "executing needs a fingerprint");
        let (status, _) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"template": t, "dry_run": false, "plan_fingerprint": "nope"})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"template": "Other/{title}", "dry_run": false, "plan_fingerprint": dry["plan_fingerprint"]})).await;
        assert_eq!(status, StatusCode::CONFLICT, "a different template is a different plan");

        let (status, started) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"template": t, "dry_run": false, "plan_fingerprint": dry["plan_fingerprint"]})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let job = wait_for_job(&router, started["job_id"].as_u64().unwrap()).await;
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["report"]["books_moved"], 1, "{job}");
        assert!(dir.path().join("Sorted/Alpha.pdf").is_file());

        let (_, runs) = get_json(&router, "/organize-runs/default").await;
        let run_id = runs["runs"][0]["run_id"].as_str().unwrap().to_string();
        let (status, started) = json_call(&router, "POST", "/unorganize-library/default", serde_json::json!({"run_id": run_id})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let job = wait_for_job(&router, started["job_id"].as_u64().unwrap()).await;
        assert_eq!(job["report"]["books_restored"], 1, "{job}");
        assert!(book.is_file());
    }

    #[tokio::test]
    async fn organize_refuses_two_scopes_at_once() {
        let (_dir, router) = test_app();
        let (status, _) = json_call(&router, "POST", "/organize-library/default", serde_json::json!({"book_ids": [1], "path_prefix": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Each job's state is written to `.lexmechanic/jobs.json`.
    #[tokio::test]
    async fn a_jobs_state_is_persisted_in_the_library() {
        let (dir, router) = test_app();
        let (_, started) = post_json(&router, "/scan-library/default").await;
        let id = started["job_id"].as_u64().unwrap();
        wait_for_job(&router, id).await;

        // The job reads as finished a moment before its final state is saved,
        // so give the save a little time instead of racing it.
        let path = dir.path().join(".lexmechanic").join("jobs.json");
        let mut rec = serde_json::Value::Null;
        for _ in 0..50 {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let doc: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if let Some(found) = doc["jobs"].as_array().and_then(|jobs| jobs.iter().find(|j| j["id"] == id && j["state"] == "finished")) {
                    rec = found.clone();
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(rec.is_object(), "jobs.json records the finished job");
        assert_eq!(rec["kind"], "scan");
        assert_eq!(rec["state"], "finished");
        assert!(rec["report"].is_object());
    }

    /// A job a dead process left `running` comes back as `interrupted`, and a
    /// resumable kind says where to continue it.
    #[tokio::test]
    async fn a_job_left_running_by_a_dead_process_is_reported_interrupted() {
        let (dir, router) = test_app();
        let handle = dir.path().join(".lexmechanic");
        std::fs::create_dir_all(&handle).unwrap();
        let rec = |id: u64, kind: &str, state: &str| serde_json::json!({"id": id, "kind": kind, "library": "x", "state": state, "report": null, "error": null, "started_at": 5, "finished_at": null, "params": {}});
        let doc = serde_json::json!({"version": 1, "jobs": [rec(1, "prepare", "running"), rec(2, "scan", "finished"), rec(3, "resolve", "running")]});
        std::fs::write(handle.join("jobs.json"), doc.to_string()).unwrap();

        let (_, body) = get_json(&router, "/library-jobs").await;
        let jobs = body["jobs"].as_array().unwrap();
        let prepare = jobs.iter().find(|j| j["params"]["previous_id"] == 1).expect("restored");
        assert_eq!(prepare["state"], "interrupted", "{body}");
        assert_eq!(prepare["resumable"], true);
        assert_eq!(prepare["resume_route"], "prepare-library");
        let resolve = jobs.iter().find(|j| j["params"]["previous_id"] == 3).unwrap();
        assert_eq!(resolve["state"], "interrupted");
        assert_eq!(resolve["resumable"], false, "a resolve has nothing to resume from");
        assert_eq!(jobs.iter().find(|j| j["params"]["previous_id"] == 2).unwrap()["state"], "finished");
        // And the file now says so too.
        let again: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(handle.join("jobs.json")).unwrap()).unwrap();
        assert!(again["jobs"].as_array().unwrap().iter().all(|j| j["state"] != "running"));
    }

    #[tokio::test]
    async fn cancelling_a_finished_or_unknown_job_is_harmless() {
        let (_dir, router) = test_app();
        let (_, started) = post_json(&router, "/scan-library/default").await;
        let id = started["job_id"].as_u64().unwrap();
        wait_for_job(&router, id).await;

        let (status, job) = post_json(&router, &format!("/library-jobs/{id}/cancel")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(job["state"], "finished", "cancel must not rewrite a finished job");

        let (status, _) = post_json(&router, "/library-jobs/999999999/cancel").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The backfill is a job too, and it hashes files a library opened
    /// from real calibre never had hashes for.
    #[tokio::test]
    async fn the_hash_backfill_is_a_job_that_records_missing_hashes() {
        let (dir, router) = test_app();
        let book = dir.path().join("a.pdf");
        std::fs::write(&book, b"%PDF-1.4 a").unwrap();
        make_settled(&book);
        post_json(&router, "/scan-library/default?wait=true").await;

        let (status, body) = post_json(&router, "/hash-backfill/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // The scan already hashed it, so there is nothing left to do.
        assert_eq!(body["total"], 0, "{body}");
        assert_eq!(body["state"], "finished");
    }

    #[tokio::test]
    async fn an_unknown_library_is_a_404() {
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
        let broker = std::sync::Arc::new(
            crate::library_broker::LibraryBroker::new(&[
                src_dir.path().to_path_buf(),
                dest_dir.path().to_path_buf(),
            ])
            .unwrap(),
        );
        let default_cache = broker.get(None).unwrap();
        let state = crate::AppState {
            libraries: Some(broker),
            cache: default_cache,
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(
                crate::reader_profiles::ProfileStore::new_in_memory().unwrap(),
            ),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(
                4,
                std::time::Duration::from_secs(3600),
            )),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(
                crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap(),
            ),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(
                lexmechanic_customize::registry::PluginRegistry::new(),
            )),
        };
        let router = crate::test_router(state);

        let (status, _) = post_json(&router, "/scan-library/no-such-library").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    fn html_book(dir: &std::path::Path, name: &str, title: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("<html><head><title>{title}</title><meta name=\"author\" content=\"Real Author\"/></head><body>x</body></html>")).unwrap();
        make_settled(&path);
    }

    /// `?enrich=false` indexes and stops; the queue is drained later by
    /// `/enrich-library`. Every report says where the time went.
    #[tokio::test]
    async fn enrich_false_defers_the_metadata_pass_and_reports_timings() {
        let (dir, router) = test_app();
        html_book(dir.path(), "scan001.html", "Real Title");

        let (status, body) = post_json(&router, "/scan-library/default?wait=true&enrich=false").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["added"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body["enrichment_deferred"], true, "{body}");
        assert!(body.get("enrichment").is_none(), "{body}");
        for phase in ["walk", "drift", "index_hash", "index_write"] {
            assert!(body["timings_ms"][phase].is_number(), "no {phase} in {body}");
        }
        let (_, ajax) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(ajax["title"], "scan001", "the title is still the filename: {ajax}");

        let (_, enriched) = post_json(&router, "/enrich-library/default?wait=true").await;
        assert_eq!(enriched["enriched"], 1, "{enriched}");
        assert!(enriched["timings_ms"]["enrich_write"].is_number(), "{enriched}");
        let (_, ajax) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(ajax["title"], "Real Title", "{ajax}");
    }

    /// The chained job runs every stage, and a rerun picks up at the stage the
    /// last one reached.
    #[tokio::test]
    async fn prepare_chains_the_stages_and_resumes_where_it_stopped() {
        let (dir, router) = test_app();
        html_book(dir.path(), "a.html", "Alpha Title");

        let (status, body) = post_json(&router, "/prepare-library/default?wait=true").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["finished"], true, "{body}");
        assert_eq!(body["started_at_stage"], "scan");
        for stage in ["scan", "hash", "fingerprint", "dedupe"] {
            assert!(body["stages"][stage].is_object(), "no {stage} in {body}");
        }
        assert_eq!(body["stages"]["scan"]["added"].as_array().unwrap().len(), 1, "{body}");
        let (_, status_body) = get_json(&router, "/prepare-library/default").await;
        assert_eq!(status_body["stage"], "scan", "a finished chain starts over at the scan: {status_body}");

        // A run that stopped at the fingerprint stage resumes there: no walk.
        std::fs::write(dir.path().join(".lexmechanic/prepare-stage.json"), r#"{"stage":"fingerprint"}"#).unwrap();
        let (_, resumed) = post_json(&router, "/prepare-library/default?wait=true").await;
        assert_eq!(resumed["started_at_stage"], "fingerprint", "{resumed}");
        assert!(resumed["stages"]["scan"].is_null() && resumed["stages"]["hash"].is_null(), "{resumed}");
        assert!(resumed["stages"]["dedupe"].is_object(), "{resumed}");

        // `restart=true` ignores the persisted stage.
        std::fs::write(dir.path().join(".lexmechanic/prepare-stage.json"), r#"{"stage":"dedupe"}"#).unwrap();
        let (_, restarted) = post_json(&router, "/prepare-library/default?wait=true&restart=true").await;
        assert_eq!(restarted["started_at_stage"], "scan", "{restarted}");
    }
}
