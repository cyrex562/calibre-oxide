//! `POST /duplicates/scan/{library_id}` -- a real, **new** route
//! (issue #762, part of #747's tracking epic): scan an entire library
//! for likely-duplicate books, independent of the add-time collision
//! check `calibre_db::copy_to_library::find_duplicate_books` already
//! provides. Real upstream (`gui2/actions/find_duplicates.py`) has a
//! Qt GUI action that does the same; this port exposes it over HTTP
//! instead, since this crate's own content server is the only real UI
//! this workspace has.
//!
//! The scan is [`calibre_db::dedupe`]: content hashes, valid shared
//! identifiers (ISBN-10/13 normalised, `asin`, `goodreads`, ...), exact
//! normalised title + author, and fuzzy title + author. Each group carries
//! its strongest `reason`, a `confidence` and the `evidence` that joined it.
//! That module's doc is where the normalisation rules and their deviations
//! from calibre live.
//!
//! # Request
//!
//! `POST /duplicates/scan/{library_id}?min=fuzzy|exact|identifier|content`
//! (default `fuzzy`: report everything and let the UI filter). Evidence
//! weaker than `min` is not used rather than merely hidden, so a fuzzy match
//! can never bridge two groups in an `exact` scan. `&job=true` starts the
//! scan as a cancellable `dedupe` job in [`crate::library_jobs`] and answers
//! `202 {job_id, status_url}`; its report is `{"groups": [...], ...}`, the
//! same shape this route answers with directly. Without it the scan runs on
//! a blocking thread and answers directly (50k books take a few seconds in a
//! debug build; see `calibre_db`'s ignored `dedupe::tests::scale_50k_books`).
//!
//! # Response
//!
//! `{"groups": [{"books": [...], "reason": "content" | "identifier" |
//! "title_author_exact" | "title_author_fuzzy", "confidence": 0.0-1.0,
//! "evidence": [{"books": [a, b], "reason", "confidence", "detail"}],
//! "suggested_dest": id}], "min": ...}`, strongest first.
//!
//! The pre-tranche-8 `"metadata"` reason is now `"title_author_exact"`. The
//! only client is this repo's frontend, changed in the same commit.
//!
//! # Real, disclosed narrowing versus upstream
//!
//! - **This route only reports.** Merging a group's records is
//!   [`crate::merge_books`] (`POST /cdb/merge-books`). The bulk
//!   `POST /duplicates/merge-exact` is **content-only**: it plans from
//!   byte-identical files and never from this scan, so a fuzzy group can
//!   only be merged by a person confirming it (there is a test for that).
//! - A book with no specific author (or an `Unknown` one) is never matched
//!   on its title; see `calibre_db::dedupe`.

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_db::copy_to_library;
use calibre_db::dedupe::{self, Strength};
use calibre_db::merge;
use calibre_db::scan_pipeline::JobControl;

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct ScanQuery {
    /// `fuzzy` (default), `exact`, `identifier` or `content`.
    pub min: Option<String>,
    /// Run as a `library_jobs` job instead of answering directly.
    pub job: Option<bool>,
}

fn parse_min(min: Option<&str>) -> Result<Strength, ServerError> {
    match min {
        None => Ok(Strength::Fuzzy),
        Some(s) => Strength::parse_min(s).ok_or_else(|| ServerError::BadRequest(format!("min must be one of content, identifier, exact, fuzzy; got {s:?}"))),
    }
}

/// The scan's answer: every group at `min` or stronger, as JSON. Shared by
/// the route and the `dedupe` job, which checks `control` between groups.
pub fn scan_report(cache: &Cache, min: Strength, control: Option<&JobControl>) -> anyhow::Result<Value> {
    use std::sync::atomic::Ordering;
    if let Some(c) = control {
        c.set_phase("matching");
    }
    let groups = dedupe::scan_library(cache, min)?;
    if let Some(c) = control {
        c.set_phase("describing");
        c.total.store(groups.len() as u64, Ordering::Relaxed);
    }
    let mut out = Vec::with_capacity(groups.len());
    for group in &groups {
        if control.is_some_and(|c| c.is_cancelled()) {
            break;
        }
        // `formats`, `metadata` and `has_cover` are what the merge engine
        // ranks records by (`merge::best_destination`); the UI applies the
        // same rule to pre-select a destination.
        let mut summaries = Vec::new();
        let mut books = Vec::new();
        for &id in &group.book_ids {
            let (title, authors) = copy_to_library::book_title_and_authors(cache, id)?;
            let s = merge::book_summary(cache, id)?;
            books.push(json!({"book_id": id, "title": title, "authors": authors, "formats": s.formats, "metadata": s.metadata, "has_cover": s.has_cover}));
            summaries.push(s);
        }
        let evidence: Vec<Value> = group.evidence.iter().map(|e| json!({"books": [e.a, e.b], "reason": e.strength.reason(), "confidence": round2(e.confidence), "detail": e.detail})).collect();
        out.push(json!({"books": books, "reason": group.strength.reason(), "confidence": round2(group.confidence), "evidence": evidence, "suggested_dest": merge::best_destination(&summaries)}));
        if let Some(c) = control {
            c.seen.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(json!({"groups": out, "min": min.reason()}))
}

fn round2(x: f32) -> f64 {
    ((x as f64) * 100.0).round() / 100.0
}

/// What the `dedupe` job runs.
pub fn run_job(cache: &Cache, min: Strength, control: &JobControl) -> anyhow::Result<Value> {
    scan_report(cache, min, Some(control))
}

/// `POST /duplicates/scan/{library_id}`.
pub async fn scan(State(state): State<AppState>, AxumPath(library_id): AxumPath<String>, Query(query): Query<ScanQuery>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let min = parse_min(query.min.as_deref())?;

    if query.job.unwrap_or(false) {
        let params = JobParams { dedupe_min: Some(min), ..Default::default() };
        return Ok(match library_jobs::start_with(cache, JobKind::Dedupe, None, params) {
            Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": "dedupe", "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
            Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": "dedupe", "state": "running", "already_running": true, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
            Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": format!("a {} job is already running on this library", active.kind.name()), "job_id": active.id}))).into_response(),
        });
    }

    let report = tokio::task::spawn_blocking(move || scan_report(&cache, min, None))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?;
    Ok(Json(report).into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    /// Each book gets **distinct** content. The fixture used to write
    /// `b"content"` for every book, which the content-based scan added in
    /// #892 correctly reports as duplicates -- two byte-identical files
    /// are duplicates, whatever their metadata says.
    fn add_book(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, authors: &[&str]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, format!("content of {name}")).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = authors.iter().map(|s| s.to_string()).collect();
        cache.add_book(&source, &meta).unwrap()
    }

    /// Builds a real library, seeds it via `seed` (run against the
    /// same `Cache` the router's own `AppState` will use -- seeding
    /// through a *second* connection to the same DB file would be
    /// real but pointlessly riskier to get right), then wraps it into
    /// a router.
    fn test_app_with<T>(seed: impl FnOnce(&std::path::Path, &Cache) -> T) -> (tempfile::TempDir, axum::Router, T) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let seeded = seed(dir.path(), &cache);
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (dir, router, seeded)
    }

    async fn post_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn a_real_near_duplicate_pair_is_reported_as_one_group() {
        let (_dir, router, (a, b)) = test_app_with(|dir, cache| {
            let a = add_book(dir, cache, "a.txt", "The Great Test", &["Ada Lovelace"]);
            let b = add_book(dir, cache, "b.txt", "The Great Test", &["Ada Lovelace"]);
            (a, b)
        });

        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        let ids: Vec<i64> = groups[0]["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
        assert!(ids.contains(&(a as i64)));
        assert!(ids.contains(&(b as i64)));
        assert_eq!(groups[0]["books"][0]["title"], "The Great Test");
        // Different files, same title and author -- so the reason is
        // metadata, and the UI can say so rather than implying the
        // stronger claim.
        assert_eq!(groups[0]["reason"], "title_author_exact");
        assert!(groups[0]["confidence"].as_f64().unwrap() >= 0.9);
        assert!(!groups[0]["evidence"].as_array().unwrap().is_empty());
    }

    /// The blind spot metadata comparison has: the same file under two
    /// different titles was invisible before #892.
    #[tokio::test]
    async fn two_byte_identical_books_are_reported_as_a_content_duplicate() {
        let (_dir, router, ()) = test_app_with(|dir, cache| {
            for name in ["a.txt", "b.txt"] {
                let source = dir.join(name);
                std::fs::write(&source, b"exactly the same bytes").unwrap();
                let mut meta = calibre_ebooks::metadata::MetaInformation::default();
                meta.title = format!("Title for {name}");
                meta.authors = vec!["Whoever".to_string()];
                cache.add_book(&source, &meta).unwrap();
            }
        });

        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        assert_eq!(groups[0]["reason"], "content");
    }

    #[tokio::test]
    async fn a_real_library_with_no_duplicates_reports_an_empty_list() {
        let (_dir, router, ()) = test_app_with(|dir, cache| {
            add_book(dir, cache, "a.txt", "Book One", &["Author One"]);
            add_book(dir, cache, "b.txt", "Book Two", &["Author Two"]);
        });

        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["groups"].as_array().unwrap().len(), 0, "{body}");
    }

    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        Cache::new(src_dir.path()).unwrap();
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).expect("the broker's default library");
        let state = crate::AppState {
            libraries: Some(broker.clone()),
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router)
    }

    #[tokio::test]
    async fn scan_404s_for_an_unknown_library_id() {
        let (_src_dir, _dest_dir, _broker, router) = test_app_with_two_libraries();
        let (status, _) = post_json(&router, "/duplicates/scan/no-such-library").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    async fn post_with_body(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn group_ids(group: &serde_json::Value) -> Vec<i64> {
        let mut ids: Vec<i64> = group["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn the_same_book_recorded_differently_is_one_exact_group_with_evidence() {
        let (_dir, router, (a, b, c)) = test_app_with(|dir, cache| {
            (
                add_book(dir, cache, "a.txt", "The Hobbit", &["J.R.R. Tolkien"]),
                add_book(dir, cache, "b.txt", "Hobbit, The (z-lib.org)", &["Tolkien, J. R. R."]),
                add_book(dir, cache, "c.txt", "Collected Poems", &["W. H. Auden"]),
            )
        });
        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        assert_eq!(group_ids(&groups[0]), vec![a as i64, b as i64]);
        assert!(!group_ids(&groups[0]).contains(&(c as i64)));
        assert_eq!(groups[0]["reason"], "title_author_exact");
        let evidence = groups[0]["evidence"].as_array().unwrap();
        assert_eq!(evidence[0]["reason"], "title_author_exact");
        assert_eq!(evidence[0]["books"].as_array().unwrap().len(), 2);
        assert!(evidence[0]["detail"].as_str().unwrap().contains("hobbit"));
    }

    #[tokio::test]
    async fn a_subtitle_difference_is_a_fuzzy_group_and_min_exact_hides_it() {
        let (_dir, router, (a, b)) = test_app_with(|dir, cache| {
            (
                add_book(dir, cache, "a.txt", "Sapiens: A Brief History of Humankind", &["Yuval Noah Harari"]),
                add_book(dir, cache, "b.txt", "Sapiens", &["Yuval Noah Harari"]),
            )
        });
        let (_, body) = post_json(&router, "/duplicates/scan/default").await;
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        assert_eq!(group_ids(&groups[0]), vec![a as i64, b as i64]);
        assert_eq!(groups[0]["reason"], "title_author_fuzzy");
        let confidence = groups[0]["confidence"].as_f64().unwrap();
        assert!(confidence > 0.5 && confidence < 0.9, "{confidence}");

        for min in ["exact", "identifier", "content"] {
            let (status, body) = post_json(&router, &format!("/duplicates/scan/default?min={min}")).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body["groups"].as_array().unwrap().is_empty(), "min={min} must hide a fuzzy group: {body}");
        }
        let (status, body) = post_json(&router, "/duplicates/scan/default?min=fuzzy").await;
        assert_eq!((status, body["groups"].as_array().unwrap().len()), (StatusCode::OK, 1));
        let (status, _) = post_json(&router, "/duplicates/scan/default?min=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn isbn_10_and_13_group_two_books_with_nothing_else_in_common() {
        let (_dir, router, (a, b)) = test_app_with(|dir, cache| {
            let a = add_book(dir, cache, "a.txt", "One Title", &["Writer One"]);
            let b = add_book(dir, cache, "b.txt", "Another Title Altogether", &["Writer Two"]);
            cache.set_field(a, "identifiers", "isbn:0-261-10221-4").unwrap();
            cache.set_field(b, "identifiers", "isbn:9780261102217").unwrap();
            (a, b)
        });
        let (_, body) = post_json(&router, "/duplicates/scan/default").await;
        let groups = body["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{body}");
        assert_eq!(group_ids(&groups[0]), vec![a as i64, b as i64]);
        assert_eq!(groups[0]["reason"], "identifier");
        let (_, body) = post_json(&router, "/duplicates/scan/default?min=identifier").await;
        assert_eq!(body["groups"].as_array().unwrap().len(), 1);
        let (_, body) = post_json(&router, "/duplicates/scan/default?min=content").await;
        assert!(body["groups"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn junk_isbns_unknown_authors_and_other_authors_do_not_group() {
        let (_dir, router, ()) = test_app_with(|dir, cache| {
            let a = add_book(dir, cache, "a.txt", "Alpha Book", &["Writer One"]);
            let b = add_book(dir, cache, "b.txt", "Beta Book", &["Writer Two"]);
            cache.set_field(a, "identifiers", "isbn:1234567890").unwrap();
            cache.set_field(b, "identifiers", "isbn:1234567890").unwrap();
            add_book(dir, cache, "c.txt", "Collected Poems", &["W. H. Auden"]);
            add_book(dir, cache, "d.txt", "Collected Poems", &["Philip Larkin"]);
            add_book(dir, cache, "e.txt", "Mystery Title", &["Unknown"]);
            add_book(dir, cache, "f.txt", "Mystery Title", &["Unknown"]);
            add_book(dir, cache, "g.txt", "Vol. 1 Story, Vol. 1", &["Same Writer"]);
            add_book(dir, cache, "h.txt", "Harry Potter, Vol. 1", &["J.K. Rowling"]);
            add_book(dir, cache, "i.txt", "Harry Potter, Vol. 2", &["J.K. Rowling"]);
        });
        let (status, body) = post_json(&router, "/duplicates/scan/default").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["groups"].as_array().unwrap().is_empty(), "{body}");
    }

    /// The bulk merge acts on byte-identical files only. A fuzzy or exact
    /// title/author group must never reach it.
    #[tokio::test]
    async fn merge_exact_stays_content_only_and_ignores_fuzzy_and_exact_metadata_groups() {
        let (_dir, router, (content_a, content_b)) = test_app_with(|dir, cache| {
            // A content pair under unrelated metadata.
            let mut ids = Vec::new();
            for (name, title) in [("p.txt", "Pair Left"), ("q.txt", "Pair Right")] {
                let source = dir.join(name);
                std::fs::write(&source, b"identical bytes").unwrap();
                let mut meta = calibre_ebooks::metadata::MetaInformation::default();
                meta.title = title.to_string();
                meta.authors = vec![format!("Author {title}")];
                ids.push(cache.add_book(&source, &meta).unwrap());
            }
            // Fuzzy and exact pairs with different bytes.
            add_book(dir, cache, "f1.txt", "Sapiens: A Brief History", &["Yuval Harari"]);
            add_book(dir, cache, "f2.txt", "Sapiens", &["Yuval Harari"]);
            add_book(dir, cache, "e1.txt", "The Hobbit", &["J.R.R. Tolkien"]);
            add_book(dir, cache, "e2.txt", "Hobbit, The", &["Tolkien, J.R.R."]);
            (ids[0], ids[1])
        });
        let (_, scan) = post_json(&router, "/duplicates/scan/default").await;
        let reasons: Vec<&str> = scan["groups"].as_array().unwrap().iter().map(|g| g["reason"].as_str().unwrap()).collect();
        assert_eq!(reasons, vec!["content", "title_author_exact", "title_author_fuzzy"], "{scan}");

        let (status, plan) = post_with_body(&router, "/duplicates/merge-exact/default", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        assert_eq!(plan["group_count"], 1, "{plan}");
        let mut ids: Vec<i64> = plan["groups"][0]["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![content_a as i64, content_b as i64]);
    }

    #[tokio::test]
    async fn the_scan_can_run_as_a_library_job() {
        let (_dir, router, ()) = test_app_with(|dir, cache| {
            add_book(dir, cache, "a.txt", "The Hobbit", &["J.R.R. Tolkien"]);
            add_book(dir, cache, "b.txt", "Hobbit, The", &["Tolkien, J.R.R."]);
        });
        let (status, started) = post_json(&router, "/duplicates/scan/default?job=true&min=exact").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        assert_eq!(started["kind"], "dedupe");
        let url = started["status_url"].as_str().unwrap().to_string();
        let mut job = serde_json::Value::Null;
        for _ in 0..200 {
            let req = Request::builder().method("GET").uri(&url).body(Body::empty()).unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            job = serde_json::from_slice(&bytes).unwrap();
            if job["state"] != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["report"]["groups"].as_array().unwrap().len(), 1, "{job}");
        assert_eq!(job["report"]["groups"][0]["reason"], "title_author_exact");
    }
}
