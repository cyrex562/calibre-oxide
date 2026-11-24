use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use lexmechanic_db::cache::Cache;

fn add_file(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, author: &str, bytes: &[u8]) -> i32 {
    let source = dir.join(name);
    std::fs::write(&source, bytes).unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec![author.to_string()];
    cache.add_book(&source, &meta).unwrap()
}

fn app_for(cache: std::sync::Arc<Cache>) -> axum::Router {
    let state = crate::AppState {
        libraries: None,
        cache,
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
        plugin_store: None,
        plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
    };
    crate::test_router(state)
}

struct Lib {
    _dir: tempfile::TempDir,
    src: tempfile::TempDir,
    cache: std::sync::Arc<Cache>,
    router: axum::Router,
}

fn lib() -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let cache = std::sync::Arc::new(Cache::new(dir.path()).unwrap());
    let router = app_for(cache.clone());
    Lib { _dir: dir, src: tempfile::tempdir().unwrap(), cache, router }
}

impl Lib {
    fn add(&self, name: &str, title: &str, author: &str, bytes: &[u8]) -> i32 {
        add_file(self.src.path(), &self.cache, name, title, author, bytes)
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.call("GET", uri, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call("POST", uri, Some(body)).await
    }

    async fn scan(&self) -> Value {
        let (status, body) = self.call("POST", "/duplicates/scan/default", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn wait_job(&self, started: &Value) -> Value {
        let url = started["status_url"].as_str().unwrap().to_string();
        let mut job = Value::Null;
        for _ in 0..400 {
            job = self.get(&url).await.1;
            if job["state"] != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(job["state"], "finished", "{job}");
        job
    }
}

fn ids(group: &Value) -> Vec<i64> {
    let mut v: Vec<i64> = group["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
    v.sort_unstable();
    v
}

/// Two content duplicates, two same-title pairs (distinct files) and one fuzzy
/// pair, plus an unrelated book.
async fn mixed(l: &Lib) -> (Vec<i32>, Vec<i32>, Vec<i32>, Vec<i32>) {
    let content = vec![l.add("c1.txt", "Alpha Primer", "Writer One", b"same bytes"), l.add("c2.txt", "Alpha Primer Copy", "Writer One", b"same bytes")];
    let exact1 = vec![l.add("e1.txt", "The Hobbit", "J.R.R. Tolkien", b"hobbit one"), l.add("e2.txt", "Hobbit, The", "Tolkien, J.R.R.", b"hobbit two")];
    let exact2 = vec![l.add("f1.txt", "Dune", "Frank Herbert", b"dune one"), l.add("f2.txt", "Dune", "Frank Herbert", b"dune two")];
    let fuzzy = vec![l.add("z1.txt", "Sapiens: A Brief History of Humankind", "Yuval Noah Harari", b"sap one"), l.add("z2.txt", "Sapiens", "Yuval Noah Harari", b"sap two")];
    l.add("u.txt", "Unrelated", "Someone Else", b"nothing in common");
    (content, exact1, exact2, fuzzy)
}

#[tokio::test]
async fn no_scan_means_no_groups_yet() {
    let l = lib();
    let (status, body) = l.get("/duplicates/groups/default").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["has_scan"], false, "{body}");
}

#[tokio::test]
async fn groups_are_paged_filtered_sorted_and_counted() {
    let l = lib();
    let (content, exact1, exact2, fuzzy) = mixed(&l).await;
    let scan = l.scan().await;
    assert_eq!(scan["groups"].as_array().unwrap().len(), 4, "{scan}");

    let (_, all) = l.get("/duplicates/groups/default?limit=2").await;
    assert_eq!(all["has_scan"], true);
    assert_eq!((all["total"].as_u64(), all["all_total"].as_u64()), (Some(4), Some(4)));
    assert_eq!(all["groups"].as_array().unwrap().len(), 2);
    // Strongest confidence first.
    assert_eq!(all["groups"][0]["reason"], "content");
    assert_eq!(all["facets"]["reason"]["content"], 1);
    assert_eq!(all["facets"]["reason"]["title_author_exact"], 2);
    assert_eq!(all["facets"]["reason"]["title_author_fuzzy"], 1);
    assert_eq!(all["stale"], false);

    // The second page continues where the first stopped, with no overlap.
    let (_, page2) = l.get("/duplicates/groups/default?limit=2&offset=2").await;
    let first: Vec<&str> = all["groups"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap()).collect();
    let second: Vec<&str> = page2["groups"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap()).collect();
    assert_eq!(second.len(), 2);
    assert!(first.iter().all(|id| !second.contains(id)));

    // Reason filter; facets still show every tier (they ignore the reason filter).
    let (_, only) = l.get("/duplicates/groups/default?reason=title_author_exact").await;
    assert_eq!(only["total"], 2);
    assert!(only["groups"].as_array().unwrap().iter().all(|g| g["reason"] == "title_author_exact"));
    assert_eq!(only["facets"]["reason"]["content"], 1);
    let (_, two) = l.get("/duplicates/groups/default?reason=content,title_author_fuzzy").await;
    assert_eq!(two["total"], 2);
    let (status, _) = l.get("/duplicates/groups/default?reason=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Confidence range: exact is 0.9, fuzzy is below, content is 1.0.
    let (_, high) = l.get("/duplicates/groups/default?min_confidence=0.9").await;
    assert_eq!(high["total"], 3);
    let (_, low) = l.get("/duplicates/groups/default?max_confidence=0.89").await;
    assert_eq!(low["total"], 1);
    assert_eq!(low["groups"][0]["reason"], "title_author_fuzzy");
    // ...and the facets follow the confidence filter.
    assert_eq!(low["facets"]["reason"]["content"], Value::Null);

    // Path prefix: books sit in the library root as `<name>.txt`... narrow by file name.
    let (_, by_path) = l.get("/duplicates/groups/default?path=E1").await;
    assert_eq!(by_path["total"], 1, "{by_path}");
    assert_eq!(ids(&by_path["groups"][0]), exact1.iter().map(|i| *i as i64).collect::<Vec<_>>());
    let (_, none) = l.get("/duplicates/groups/default?path=nowhere/").await;
    assert_eq!(none["total"], 0);

    // Sort by size then order.
    let (_, asc) = l.get("/duplicates/groups/default?sort=confidence&order=asc").await;
    assert_eq!(asc["groups"][0]["reason"], "title_author_fuzzy");
    let (status, _) = l.get("/duplicates/groups/default?sort=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = (content, exact2, fuzzy);
}

#[tokio::test]
async fn a_group_id_is_stable_across_rescans_and_hashes_the_member_uuids() {
    let l = lib();
    let (_, exact1, ..) = mixed(&l).await;
    let first = l.scan().await;
    let second = l.scan().await;
    let id_of = |scan: &Value, books: &[i32]| {
        scan["groups"].as_array().unwrap().iter().find(|g| ids(g) == books.iter().map(|b| *b as i64).collect::<Vec<_>>()).unwrap()["id"].as_str().unwrap().to_string()
    };
    assert_eq!(id_of(&first, &exact1), id_of(&second, &exact1));
    let uuids: Vec<String> = exact1.iter().map(|b| l.cache.book_uuid(*b).unwrap().unwrap()).collect();
    assert_eq!(id_of(&first, &exact1), lexmechanic_db::dedupe_exemptions::set_id(&uuids));
}

#[tokio::test]
async fn a_book_carries_its_files_sizes_paths_and_cover_url() {
    let l = lib();
    let (content, ..) = mixed(&l).await;
    l.scan().await;
    let (_, page) = l.get("/duplicates/groups/default?reason=content").await;
    let book = &page["groups"][0]["books"][0];
    assert_eq!(book["book_id"], content[0]);
    assert!(book["uuid"].as_str().unwrap().len() > 10);
    let files = book["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["format"], "TXT");
    assert_eq!(files[0]["size"], "same bytes".len());
    assert!(files[0]["path"].as_str().unwrap().ends_with(".txt"));
    assert!(files[0]["mtime"].as_u64().unwrap() > 1_000_000_000, "{book}");
    assert_eq!(book["size"], "same bytes".len());
    assert_eq!(book["formats"], 1);
    assert!(book["mtime"].as_u64().is_some());
    // No cover, so no URL; the suggested keeper and its reason are given.
    assert_eq!(book["cover_url"], Value::Null);
    assert!(page["groups"][0]["keeper_reason"].as_str().is_some_and(|r| !r.is_empty()));
    assert_eq!(page["groups"][0]["bytes_reclaimable"], "same bytes".len());
}

#[tokio::test]
async fn the_snapshot_goes_stale_after_a_library_write() {
    let l = lib();
    mixed(&l).await;
    l.scan().await;
    assert_eq!(l.get("/duplicates/groups/default").await.1["stale"], false);
    l.add("late.txt", "Late Arrival", "New Author", b"late");
    let (_, after) = l.get("/duplicates/groups/default").await;
    assert_eq!(after["stale"], true, "{after}");
    // The groups are still there to review...
    assert_eq!(after["total"], 4);
    // ...but a resolve against a stale scan is refused.
    let (status, body) = l.post("/duplicates/resolve/default", json!({"dry_run": true})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["stale"], true);
    // A rescan makes it current again.
    l.scan().await;
    assert_eq!(l.get("/duplicates/groups/default").await.1["stale"], false);
}

#[tokio::test]
async fn the_scan_job_reports_a_summary_and_stores_the_snapshot_for_paging() {
    let l = lib();
    mixed(&l).await;
    let (status, started) = l.call("POST", "/duplicates/scan/default?job=true", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["group_count"], 4, "{job}");
    assert_eq!(job["report"]["facets"]["reason"]["content"], 1);
    assert!(job["report"]["groups"].is_null(), "the job report does not carry the groups");
    let (_, page) = l.get("/duplicates/groups/default?limit=10").await;
    assert_eq!(page["groups"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn dismissing_a_group_hides_it_keeps_the_snapshot_current_and_can_be_undone() {
    let l = lib();
    let (_, exact1, ..) = mixed(&l).await;
    let scan = l.scan().await;
    let gid = scan["groups"].as_array().unwrap().iter().find(|g| ids(g) == exact1.iter().map(|b| *b as i64).collect::<Vec<_>>()).unwrap()["id"].as_str().unwrap().to_string();

    let (status, done) = l.post("/duplicates/exempt/default", json!({"group_id": gid})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["dropped_group_ids"], json!([gid]));
    assert_eq!(done["needs_rescan"], false);
    assert_eq!(done["pairs"], 1);
    let (_, page) = l.get("/duplicates/groups/default").await;
    assert_eq!(page["total"], 3, "{page}");
    assert_eq!(page["stale"], false, "our own write must not strand the user's place");

    // It stays dismissed across a rescan, and is listed.
    let rescan = l.scan().await;
    assert_eq!(rescan["groups"].as_array().unwrap().len(), 3);
    let (_, listed) = l.get("/duplicates/exemptions/default").await;
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["exemptions"][0]["members"].as_array().unwrap().len(), 2);
    assert_eq!(listed["exemptions"][0]["members"][0]["present"], true);

    // Undo: the group is back after a rescan.
    let (_, removed) = l.post("/duplicates/exemptions/remove/default", json!({"id": listed["exemptions"][0]["id"]})).await;
    assert_eq!((removed["removed"].as_u64(), removed["needs_rescan"].as_bool()), (Some(1), Some(true)));
    assert_eq!(l.scan().await["groups"].as_array().unwrap().len(), 4);

    // By book ids, and "all".
    let (status, _) = l.post("/duplicates/exempt/default", json!({"book_ids": [exact1[0]]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = l.post("/duplicates/exempt/default", json!({"book_ids": [exact1[0], 99999]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = l.post("/duplicates/exempt/default", json!({"book_ids": exact1})).await;
    assert_eq!(status, StatusCode::OK);
    let (_, all) = l.post("/duplicates/exemptions/remove/default", json!({"all": true})).await;
    assert_eq!(all["removed"], 1);
}

#[tokio::test]
async fn dropping_groups_after_a_merge_keeps_the_place_without_a_rescan() {
    let l = lib();
    mixed(&l).await;
    let scan = l.scan().await;
    let first = scan["groups"][0]["id"].as_str().unwrap().to_string();
    let (_, dropped) = l.post("/duplicates/drop/default", json!({"group_ids": [first]})).await;
    assert_eq!(dropped["dropped"], 1);
    assert_eq!(dropped["remaining"], 3);
    let (_, page) = l.get("/duplicates/groups/default").await;
    assert_eq!(page["total"], 3);
}

#[tokio::test]
async fn a_book_deleted_from_a_group_leaves_it_and_a_group_of_one_goes() {
    let l = lib();
    let a = l.add("t1.txt", "Trio", "Same Author", b"trio one");
    let b = l.add("t2.txt", "Trio", "Same Author", b"trio two");
    let c = l.add("t3.txt", "Trio", "Same Author", b"trio three");
    l.scan().await;
    let (_, _) = l.post("/duplicates/drop/default", json!({"book_ids": [a]})).await;
    let (_, page) = l.get("/duplicates/groups/default").await;
    assert_eq!(page["total"], 1);
    assert_eq!(ids(&page["groups"][0]), vec![b as i64, c as i64]);
    assert!(page["groups"][0]["books"].as_array().unwrap().iter().all(|x| x["book_id"] != a));
    let keeper = page["groups"][0]["suggested_dest"].as_i64().unwrap();
    assert!(keeper == b as i64 || keeper == c as i64, "the keeper is still in the group");
    let (_, dropped) = l.post("/duplicates/drop/default", json!({"book_ids": [b]})).await;
    assert_eq!(dropped["remaining"], 0, "{dropped}");
}

// ---- resolve ----

async fn dry_run(l: &Lib, body: Value) -> Value {
    let (status, plan) = l.post("/duplicates/resolve/default", body).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    plan
}

#[tokio::test]
async fn the_default_resolve_selection_is_content_groups_only() {
    let l = lib();
    let (content, ..) = mixed(&l).await;
    l.scan().await;
    let plan = dry_run(&l, json!({})).await;
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["totals"]["groups"], 1, "{plan}");
    assert_eq!(plan["totals"]["books"], 1);
    assert_eq!(plan["totals"]["bytes_freed"], "same bytes".len());
    let group = &plan["groups"][0];
    assert_eq!(group["reason"], "content");
    let others = group["others"].as_array().unwrap();
    assert_eq!(others.len(), 1);
    let both: Vec<i64> = [group["keeper"]["book_id"].as_i64().unwrap(), others[0]["book_id"].as_i64().unwrap()].into_iter().collect();
    assert!(content.iter().all(|c| both.contains(&(*c as i64))));
    assert!(group["keeper_reason"].as_str().is_some_and(|r| !r.is_empty()));
    assert_eq!(plan["fully_restorable"], true);
}

#[tokio::test]
async fn title_tiers_are_only_selected_when_asked_for() {
    let l = lib();
    mixed(&l).await;
    l.scan().await;
    assert_eq!(dry_run(&l, json!({"tiers": ["content"]})).await["totals"]["groups"], 1);
    assert_eq!(dry_run(&l, json!({"tiers": ["title_author_exact"]})).await["totals"]["groups"], 2);
    assert_eq!(dry_run(&l, json!({"tiers": ["content", "title_author_exact", "title_author_fuzzy"]})).await["totals"]["groups"], 4);
    // Confidence floor and path prefix narrow a selection.
    assert_eq!(dry_run(&l, json!({"tiers": ["content", "title_author_exact", "title_author_fuzzy"], "min_confidence": 0.9})).await["totals"]["groups"], 3);
    assert_eq!(dry_run(&l, json!({"tiers": ["title_author_exact"], "path_prefix": "f1"})).await["totals"]["groups"], 1);
    let (status, _) = l.post("/duplicates/resolve/default", json!({"tiers": ["nonsense"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn executing_needs_the_fingerprint_and_refuses_a_changed_plan() {
    let l = lib();
    mixed(&l).await;
    l.scan().await;
    let plan = dry_run(&l, json!({})).await;

    let (status, _) = l.post("/duplicates/resolve/default", json!({"dry_run": false})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no fingerprint");
    let (status, body) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "plan_fingerprint": "stale"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // A different selection is a different plan, so the old fingerprint is refused.
    let (status, _) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "tiers": ["title_author_exact"], "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // And so is a different action.
    let (status, _) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "action": "merge_as_formats", "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Nothing happened.
    assert_eq!(l.cache.all_book_ids().unwrap().len(), 9);

    // A library change after the preview makes the same fingerprint stale.
    l.add("later.txt", "Later", "Author", b"later");
    let (status, body) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn keep_best_trash_rest_trashes_the_others_and_the_run_can_be_restored() {
    let l = lib();
    let (content, ..) = mixed(&l).await;
    l.scan().await;
    let plan = dry_run(&l, json!({"action": "keep_best_trash_rest"})).await;
    let keeper = plan["groups"][0]["keeper"]["book_id"].as_i64().unwrap() as i32;
    let other = *content.iter().find(|c| **c != keeper).unwrap();

    let (status, started) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "action": "keep_best_trash_rest", "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let run_id = started["run_id"].as_str().unwrap().to_string();
    let job = l.wait_job(&started).await;
    assert_eq!(job["kind"], "resolve");
    assert_eq!(job["report"]["run_id"], run_id.as_str());
    assert_eq!(job["report"]["books_trashed"], 1, "{job}");
    assert_eq!(job["report"]["groups_done"], 1);
    assert!(job["report"]["errors"].as_array().unwrap().is_empty(), "{job}");

    let remaining = l.cache.all_book_ids().unwrap();
    assert!(remaining.contains(&keeper) && !remaining.contains(&other), "{remaining:?}");
    // The run is listed, and its entries are tagged.
    let (_, runs) = l.get("/duplicates/runs/default").await;
    assert_eq!(runs["runs"][0]["run_id"], run_id.as_str());
    assert_eq!(runs["runs"][0]["books"], 1);
    // The snapshot lost the resolved group and is still current.
    let (_, page) = l.get("/duplicates/groups/default").await;
    assert_eq!((page["total"].as_u64(), page["stale"].as_bool()), (Some(3), Some(false)), "{page}");

    // Undo this run.
    let (status, restored) = l.post(&format!("/duplicates/runs/default/{run_id}/restore?wait=true"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["restored"].as_array().unwrap().len(), 1);
    assert_eq!(restored["needs_rescan"], true);
    assert_eq!(l.cache.all_book_ids().unwrap().len(), 9, "everything is back");
    let (status, _) = l.post(&format!("/duplicates/runs/default/{run_id}/restore?wait=true"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the run has nothing left in the trash");
}

#[tokio::test]
async fn a_keeper_override_is_honoured_and_checked() {
    let l = lib();
    let (content, ..) = mixed(&l).await;
    let scan = l.scan().await;
    let gid = scan["groups"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(scan["groups"][0]["reason"], "content");
    let suggested = scan["groups"][0]["suggested_dest"].as_i64().unwrap() as i32;
    let other = *content.iter().find(|c| **c != suggested).unwrap();

    let plan = dry_run(&l, json!({"group_ids": [gid], "keepers": {gid.clone(): other}})).await;
    assert_eq!(plan["groups"][0]["keeper"]["book_id"], other);
    assert_eq!(plan["groups"][0]["keeper_reason"], "chosen by you");
    let (status, _) = l.post("/duplicates/resolve/default", json!({"group_ids": [gid], "keepers": {gid.clone(): 99999}})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // An unknown explicit group is reported, not fatal.
    let plan = dry_run(&l, json!({"group_ids": [gid, "feedfacefeedface"]})).await;
    assert_eq!(plan["missing_group_ids"], json!(["feedfacefeedface"]));
}

#[tokio::test]
async fn merge_as_formats_merges_into_the_keeper_and_says_it_is_not_fully_undoable() {
    let l = lib();
    // Same title and author, different formats.
    let a = l.add("m1.txt", "Merge Me", "Pat Writer", b"text version");
    let b = l.add("m2.epub", "Merge Me", "Pat Writer", b"epub version");
    l.scan().await;
    let body = json!({"tiers": ["title_author_exact"], "action": "merge_as_formats"});
    let plan = dry_run(&l, body.clone()).await;
    assert_eq!(plan["fully_restorable"], false);
    assert!(plan["note"].as_str().unwrap().contains("does NOT remove"), "{plan}");
    let keeper = plan["groups"][0]["keeper"]["book_id"].as_i64().unwrap() as i32;

    let mut exec = body;
    exec["dry_run"] = json!(false);
    exec["plan_fingerprint"] = plan["plan_fingerprint"].clone();
    let (status, started) = l.post("/duplicates/resolve/default", exec).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["books_trashed"], 1, "{job}");
    assert_eq!(job["report"]["fully_restorable"], false);
    let left = l.cache.all_book_ids().unwrap();
    assert_eq!(left, vec![keeper], "{a} {b}");
    assert_eq!(l.cache.format_file_info(keeper).unwrap().len(), 2);
}

/// Undo of a run is a job: 202 and an id at once, the books come back as it runs, and the report is
/// what the synchronous route used to answer.
#[tokio::test]
async fn restoring_a_run_is_a_job_with_progress_and_the_old_report() {
    let l = lib();
    let (content, ..) = mixed(&l).await;
    l.scan().await;
    let plan = dry_run(&l, json!({"action": "keep_best_trash_rest"})).await;
    let keeper = plan["groups"][0]["keeper"]["book_id"].as_i64().unwrap() as i32;
    let other = *content.iter().find(|c| **c != keeper).unwrap();
    let (_, started) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "action": "keep_best_trash_rest", "plan_fingerprint": plan["plan_fingerprint"]})).await;
    let run_id = started["run_id"].as_str().unwrap().to_string();
    l.wait_job(&started).await;
    assert!(!l.cache.all_book_ids().unwrap().contains(&other));

    let (status, restoring) = l.post(&format!("/duplicates/runs/default/{run_id}/restore"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{restoring}");
    assert_eq!(restoring["run_id"], run_id.as_str());
    let job = l.wait_job(&restoring).await;
    assert_eq!(job["kind"], "restore_run");
    assert_eq!(job["report"]["restored"].as_array().unwrap().len(), 1, "{job}");
    assert_eq!(job["report"]["needs_rescan"], true);
    assert_eq!(job["report"]["cancelled"], false);
    assert_eq!(job["progress"]["total"], 1);
    assert_eq!(job["progress"]["seen"], 1);
    assert_eq!(job["progress"]["phase"], "restoring");
    assert!(l.cache.all_book_ids().unwrap().contains(&other) || l.cache.all_book_ids().unwrap().len() == 9);
}
