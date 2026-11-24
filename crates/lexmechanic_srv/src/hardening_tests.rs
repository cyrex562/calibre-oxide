//! Router-level tests for the request guard (`guard`), the one-`Cache`-per-
//! library rule and the file-moving-job 409s.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use lexmechanic_db::cache::Cache;

use crate::library_trash::tests::app;

async fn send(router: &axum::Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let resp = router.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn book(cache: &Cache, dir: &std::path::Path, name: &str) -> i32 {
    let path = dir.join(format!("{name}.pdf"));
    std::fs::write(&path, format!("%PDF {name}")).unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = name.to_string();
    meta.authors = vec!["Author".to_string()];
    cache.register_book_in_place(&format!("{name}.pdf"), &meta).unwrap()
}

/// The attack: a page on another site posts to the no-body delete route.
#[tokio::test]
async fn a_foreign_origin_cannot_delete_books_or_empty_the_trash() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Victim");
    let host = ("host", "127.0.0.1:8080");

    let (status, _) = send(&router, "POST", &format!("/cdb/delete-books/{id}/-"), &[host, ("origin", "http://evil.example")], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&router, "POST", &format!("/cdb/delete-books/{id}/-"), &[host, ("origin", "null")], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(cache.field_for(id, "title").unwrap().is_some(), "the book is still there");

    cache.delete_book(id).unwrap();
    let (status, _) = send(&router, "POST", "/trash/empty/-", &[host, ("origin", "http://evil.example")], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, listing) = send(&router, "GET", "/trash/-", &[host], None).await;
    assert_eq!(listing["count"], 1, "the trash was not emptied");
}

/// DNS rebinding: the page's own name resolves to 127.0.0.1, so the browser
/// sends it as `Host` and treats the answers as same-origin.
#[tokio::test]
async fn a_foreign_host_is_refused() {
    let (_dir, _cache, router) = app();
    let (status, _) = send(&router, "GET", "/ajax/library-info", &[("host", "rebind.evil.example:8080")], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&router, "GET", "/ajax/library-info", &[("host", "127.0.0.1:8080")], None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn same_origin_and_originless_requests_still_work() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Mine");
    cache.delete_book(id).unwrap();

    // GET with no Origin (a navigation, curl).
    let (status, listing) = send(&router, "GET", "/trash/-", &[("host", "localhost:8080")], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["count"], 1);
    // The web UI's own POST: Origin equals the server's.
    let (status, body) = send(&router, "POST", "/trash/restore/-", &[("host", "127.0.0.1:8080"), ("origin", "http://127.0.0.1:8080"), ("content-type", "application/json")], Some(json!({"kind": "book", "key": listing["entries"][0]["key"]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The Tauri webview's origin.
    let (status, _) = send(&router, "POST", "/trash/empty/-", &[("host", "127.0.0.1:8080"), ("origin", "tauri://localhost")], None).await;
    assert_eq!(status, StatusCode::OK);
}

/// `text/plain` is a CORS-safelisted content type, so a cross-site form can
/// send it with no preflight; the JSON-parsing command route must not take it.
#[tokio::test]
async fn the_command_route_requires_a_json_content_type() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Cmd");
    let body = json!([[id], false]);

    let resp = router.clone().oneshot(Request::builder().method("POST").uri("/cdb/cmd/remove/0").header("content-type", "text/plain").body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = router.clone().oneshot(Request::builder().method("POST").uri("/cdb/cmd/remove/0").body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(cache.field_for(id, "title").unwrap().is_some(), "nothing was removed");

    let (status, out) = send(&router, "POST", "/cdb/cmd/remove/0", &[("content-type", "application/json")], Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// A broker built the way the server does shares the server's own `Cache`.
#[tokio::test]
async fn the_servers_cache_is_the_brokers_default_library() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("A"), dir.path().join("B"));
    for p in [&a, &b] {
        std::fs::create_dir_all(p).unwrap();
        Cache::new(p).unwrap();
    }
    let cache = Arc::new(Cache::new(&a).unwrap());
    let broker = Arc::new(crate::library_broker::LibraryBroker::new_seeded(&[a.clone(), b], Some((a.as_path(), cache.clone()))).unwrap());
    let state = crate::AppState { libraries: Some(broker.clone()), cache: cache.clone(), ..test_state() };
    assert!(Arc::ptr_eq(&state.cache, &state.cache_for(None).unwrap()));
    assert!(Arc::ptr_eq(&state.cache, &state.cache_for(Some("A")).unwrap()));
    assert!(Arc::ptr_eq(&state.cache, &state.cache_for(Some("default")).unwrap()));
    assert!(Arc::ptr_eq(&state.cache, &state.cache_for(Some("-")).unwrap()));

    // An edit made through the broker's handle is read through the state's.
    let id = book(&state.cache_for(Some("A")).unwrap(), &a, "Shared");
    state.cache_for(Some("A")).unwrap().set_field(id, "title", "Edited").unwrap();
    assert_eq!(state.cache.field_for(id, "title").unwrap().as_deref(), Some("Edited"));
}

fn test_state() -> crate::AppState {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(dir.path()).unwrap());
    // The directory outlives the test process' use of it only as a scratch
    // library; leak it so the cache's files stay valid.
    std::mem::forget(dir);
    crate::AppState {
        libraries: None,
        cache,
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
    }
}

/// While an organize/resolve/import job moves files, the routes that also move
/// files answer 409 instead of racing it.
#[tokio::test]
async fn file_moving_routes_answer_409_while_a_file_moving_job_runs() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Busy");
    let job = crate::library_jobs::pretend_running(&cache, crate::library_jobs::JobKind::Organize);

    let (status, body) = send(&router, "POST", &format!("/cdb/delete-books/{id}/-"), &[], None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = send(&router, "POST", "/trash/restore/-", &[("content-type", "application/json")], Some(json!({"kind": "book", "key": "1"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&router, "POST", "/duplicates/runs/-/some-run/restore", &[], None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&router, "POST", "/rename-files/-", &[("content-type", "application/json")], Some(json!({"book_ids": [id], "template": "{title}", "dry_run": false}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&router, "POST", "/cdb/merge-books/-", &[("content-type", "application/json")], Some(json!({"dest": id, "sources": [id + 1], "dry_run": false}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // A preview moves nothing, so it is still allowed.
    let (status, _) = send(&router, "POST", "/rename-files/-", &[("content-type", "application/json")], Some(json!({"book_ids": [id], "template": "{title}", "dry_run": true}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cache.field_for(id, "title").unwrap().is_some(), "the book was not deleted");

    crate::library_jobs::finish_pretend(&job);
    let (status, body) = send(&router, "POST", &format!("/cdb/delete-books/{id}/-"), &[], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A scan or a hash backfill moves no files, so it does not block deleting.
#[tokio::test]
async fn a_non_moving_job_does_not_block_deleting() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Fine");
    let job = crate::library_jobs::pretend_running(&cache, crate::library_jobs::JobKind::HashBackfill);
    let (status, body) = send(&router, "POST", &format!("/cdb/delete-books/{id}/-"), &[], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    crate::library_jobs::finish_pretend(&job);
}
