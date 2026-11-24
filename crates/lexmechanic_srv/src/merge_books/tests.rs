use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use lexmechanic_db::cache::Cache;

fn book(cache: &Cache, name: &str, ext: &str, bytes: &str, title: &str) -> i32 {
    let stage = tempfile::tempdir().unwrap();
    let source = stage.path().join(format!("{name}.{ext}"));
    std::fs::write(&source, bytes).unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec!["Author".to_string()];
    cache.add_book(&source, &meta).unwrap()
}

fn app() -> (tempfile::TempDir, Arc<Cache>, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(dir.path()).unwrap());
    let state = crate::AppState {
        libraries: None,
        cache: cache.clone(),
        opts: Arc::new(crate::opts::ServerOptions::default()),
        auth: None,
        changes: crate::web_socket::new_change_broadcaster(),
        reader_profiles: Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
        book_cache: Arc::new(crate::books_cache::BookCache::open_temp()),
        jobs: Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
        render_jobs: Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
        conversion_jobs: Arc::new(crate::convert::ConversionJobRegistry::new()),
        news_jobs: Arc::new(crate::news::NewsJobRegistry::new()),
        tweak_sessions: Arc::new(crate::tweak::TweakSessionRegistry::new()),
        news_schedules: Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
        tts_voice: None,
        plugin_store: None,
        plugin_registry: Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
    };
    (dir, cache, crate::test_router(state))
}

async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(router: &axum::Router, uri: &str) -> Value {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null)
}

#[tokio::test]
async fn merging_moves_formats_reports_what_was_trashed_and_removes_the_source() {
    let (_dir, cache, router) = app();
    let dest = book(&cache, "d", "pdf", "pdf bytes", "T");
    let src = book(&cache, "s", "pdf", "different pdf bytes", "T");
    cache.set_field(src, "tags", "from-source").unwrap();

    let (status, body) = post(&router, "/cdb/merge-books", json!({"dest": dest, "sources": [src]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["results"][0]["book_id"], src);
    assert_eq!(body["results"][0]["formats_discarded"], json!(["PDF"]));
    assert_eq!(body["trashed"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["metadata_changed"], json!(["tags"]));
    assert!(cache.field_for(src, "path").unwrap().is_none());
    assert_eq!(cache.field_for(dest, "tags").unwrap().as_deref(), Some("from-source"));
}

#[tokio::test]
async fn the_mode_and_dry_run_are_honoured() {
    let (_dir, cache, router) = app();
    let dest = book(&cache, "d", "pdf", "d", "T");
    let src = book(&cache, "s", "epub", "s", "T");

    let (_, body) = post(&router, "/cdb/merge-books/default", json!({"dest": dest, "sources": [src], "dry_run": true})).await;
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["results"][0]["formats_moved"], json!(["EPUB"]));
    assert!(cache.field_for(src, "path").unwrap().is_some(), "a dry run changes nothing");
    assert_eq!(cache.format_file_info(dest).unwrap().len(), 1);

    let (status, body) = post(&router, "/cdb/merge-books", json!({"dest": dest, "sources": [src], "mode": "merge_keep_sources"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "merge_keep_sources");
    assert!(cache.field_for(src, "path").unwrap().is_some(), "the source is kept");
    assert_eq!(cache.format_file_info(dest).unwrap().len(), 2);
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let (_dir, cache, router) = app();
    let a = book(&cache, "a", "pdf", "a", "T");
    let (status, _) = post(&router, "/cdb/merge-books", json!({"dest": a, "sources": [a]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(&router, "/cdb/merge-books", json!({"dest": a, "sources": [999]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_bulk_merge_dry_runs_by_default_and_executes_only_a_confirmed_plan() {
    let (_dir, cache, router) = app();
    let mut dups = Vec::new();
    for n in 0..3 {
        let a = book(&cache, &format!("a{n}"), "pdf", &format!("same {n}"), &format!("Title {n}"));
        let b = book(&cache, &format!("b{n}"), "pdf", &format!("same {n}"), &format!("Other {n}"));
        dups.push((a, b));
    }
    // Same title and author, different content: never auto-merged.
    let m1 = book(&cache, "m1", "pdf", "meta one", "Same Title");
    let m2 = book(&cache, "m2", "pdf", "meta two", "Same Title");
    let total = cache.all_book_ids().unwrap().len();

    // No body flags at all: a dry run.
    let (status, plan) = post(&router, "/duplicates/merge-exact/default", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["group_count"], 3);
    assert_eq!(plan["books_to_merge"], 3);
    let ids: Vec<i64> = plan["groups"].as_array().unwrap().iter().flat_map(|g| g["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap())).collect();
    assert!(!ids.contains(&(m1 as i64)) && !ids.contains(&(m2 as i64)));
    assert_eq!(cache.all_book_ids().unwrap().len(), total, "a dry run changes nothing");
    let fingerprint = plan["plan_fingerprint"].as_str().unwrap().to_string();

    // Execute needs the fingerprint, and the right one.
    let (status, _) = post(&router, "/duplicates/merge-exact/default", json!({"dry_run": false})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(&router, "/duplicates/merge-exact/default", json!({"dry_run": false, "plan_fingerprint": "stale"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(cache.all_book_ids().unwrap().len(), total);

    let (status, started) = post(&router, "/duplicates/merge-exact/default", json!({"dry_run": false, "plan_fingerprint": fingerprint})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let url = started["status_url"].as_str().unwrap().to_string();
    let mut job = Value::Null;
    for _ in 0..200 {
        job = get(&router, &url).await;
        if job["state"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(job["state"], "finished", "{job}");
    assert_eq!(job["kind"], "merge_exact");
    assert_eq!(job["report"]["groups_merged"], 3, "{job}");
    assert_eq!(job["progress"]["seen"], 3);
    assert_eq!(cache.all_book_ids().unwrap().len(), total - 3);
    assert!(cache.field_for(m1, "path").unwrap().is_some() && cache.field_for(m2, "path").unwrap().is_some());
    for (a, b) in dups {
        assert!(cache.field_for(a, "path").unwrap().is_some() && cache.field_for(b, "path").unwrap().is_none());
    }
}

#[tokio::test]
async fn the_scan_offers_what_the_ui_needs_to_pick_a_destination() {
    let (dir, cache, router) = app();
    book(&cache, "a", "pdf", "same", "One");
    let b = book(&cache, "b", "pdf", "same", "Two");
    let extra = dir.path().join("extra.epub");
    std::fs::write(&extra, "e").unwrap();
    cache.add_format(b, &extra, "epub", true).unwrap();
    let (_, body) = post(&router, "/duplicates/scan/default", json!({})).await;
    let group = &body["groups"][0];
    assert_eq!(group["suggested_dest"], b, "{body}");
    let two = group["books"].as_array().unwrap().iter().find(|x| x["book_id"] == b).unwrap();
    assert_eq!(two["formats"], 2);
    assert!(two.get("has_cover").is_some() && two.get("metadata").is_some());
}
