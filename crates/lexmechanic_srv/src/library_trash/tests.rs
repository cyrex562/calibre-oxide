use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use lexmechanic_db::cache::Cache;

pub(crate) fn app() -> (tempfile::TempDir, Arc<Cache>, axum::Router) {
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

pub(crate) async fn call(router: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn book(cache: &Cache, dir: &std::path::Path, rel: &str, title: &str) -> i32 {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("%PDF {title}")).unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec!["Author".to_string()];
    cache.register_book_in_place(rel, &meta).unwrap()
}

#[tokio::test]
async fn delete_lists_in_the_trash_and_restores_through_the_routes() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "Shelf/a.pdf", "A Title");

    let (status, body) = call(&router, "POST", &format!("/cdb/delete-books/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"deleted": [id]}), "nothing failed: it went into .caltrash, not an OS bin");
    assert!(!dir.path().join("Shelf/a.pdf").exists());

    let (status, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["count"], 1);
    assert_eq!(listing["expire_after_seconds"], 14 * 86400);
    let entry = &listing["entries"][0];
    assert_eq!(entry["kind"], "book");
    assert_eq!(entry["title"], "A Title");
    assert_eq!(entry["authors"], "Author");
    assert_eq!(entry["formats"], json!(["PDF"]));
    assert!(entry["size"].as_i64().unwrap() > 0);
    assert!(entry["deleted_at"].as_i64().unwrap() > 0);
    assert!(entry["expires_at"].as_i64().unwrap() > entry["deleted_at"].as_i64().unwrap());
    assert_eq!(listing["total_size"], entry["size"]);

    let (status, restored) = call(&router, "POST", "/trash/restore/-", Some(json!({"kind": "book", "key": entry["key"]}))).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["title"], "A Title");
    assert_eq!(std::fs::read(dir.path().join("Shelf/a.pdf")).unwrap(), b"%PDF A Title");
    assert!(cache.field_for(restored["book_id"].as_i64().unwrap() as i32, "title").unwrap().is_some());
    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(listing["count"], 0);
}

#[tokio::test]
async fn deleting_one_entry_and_emptying_are_for_ever() {
    let (dir, cache, router) = app();
    for (rel, title) in [("a.pdf", "A"), ("b.pdf", "B"), ("c.pdf", "C")] {
        let id = book(&cache, dir.path(), rel, title);
        cache.delete_book(id).unwrap();
    }
    let (_, listing) = call(&router, "GET", "/trash/default", None).await;
    assert_eq!(listing["count"], 3);
    let first = listing["entries"][0]["key"].clone();

    let (status, _) = call(&router, "POST", "/trash/delete/-", Some(json!({"kind": "book", "key": first}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(listing["count"], 2);

    let (status, body) = call(&router, "POST", "/trash/empty/-", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"], 2);
    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(listing["count"], 0);
}

#[tokio::test]
async fn a_removed_format_is_restored_by_naming_the_format() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "a.pdf", "A");
    std::fs::write(dir.path().join("a.epub"), b"epub").unwrap();
    cache.record_format_row(id, "EPUB", "a", 4, None).unwrap();
    cache.remove_format(id, "epub").unwrap();

    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    let entry = &listing["entries"][0];
    assert_eq!(entry["kind"], "format");

    let (status, body) = call(&router, "POST", "/trash/restore/-", Some(json!({"kind": "format", "key": entry["key"]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "needs the format: {body}");
    let (status, body) = call(&router, "POST", "/trash/restore/-", Some(json!({"kind": "format", "key": entry["key"], "format": "EPUB"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read(dir.path().join("a.epub")).unwrap(), b"epub");
}

/// Listing the trash must not delete anything, however old an entry is; only
/// the explicit expire route does.
#[tokio::test]
async fn a_get_never_expires_old_entries_but_the_post_does() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "a.pdf", "A");
    cache.delete_book(id).unwrap();

    // Age the entry to the epoch.
    let manifest = walk_for(&dir.path().join(".caltrash"), "lexmechanic-trash.json").expect("a trash manifest");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let mut value: Value = serde_json::from_str(&text).unwrap();
    value["deleted_at"] = json!(1);
    std::fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    for _ in 0..2 {
        let (status, listing) = call(&router, "GET", "/trash/-", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listing["count"], 1, "a read must not expire: {listing}");
    }
    let (status, body) = call(&router, "POST", "/trash/expire/-", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"].as_array().map(Vec::len), Some(1), "{body}");
    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(listing["count"], 0);
}

fn walk_for(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = walk_for(&p, name) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

#[tokio::test]
async fn expire_removes_only_what_is_old_enough_and_bad_input_is_a_400() {
    let (dir, cache, router) = app();
    let id = book(&cache, dir.path(), "a.pdf", "A");
    cache.delete_book(id).unwrap();

    let (status, body) = call(&router, "POST", "/trash/expire/-", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"], json!([]), "a fresh entry is not old");
    let (_, listing) = call(&router, "GET", "/trash/-", None).await;
    assert_eq!(listing["count"], 1);

    let (status, _) = call(&router, "POST", "/trash/restore/-", Some(json!({"kind": "nonsense", "key": "1"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&router, "POST", "/trash/restore/-", Some(json!({"kind": "book", "key": "../.."}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&router, "GET", "/trash/nonexistent-library", None).await;
    // With no broker every id resolves to the one library.
    assert_eq!(status, StatusCode::OK);
}
