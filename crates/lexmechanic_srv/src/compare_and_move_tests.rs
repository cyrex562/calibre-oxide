//! Route tests for tranche 16: `GET /duplicates/compare`, the user's groups
//! (`/duplicates/manual-groups`) and `POST /move-books`.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use lexmechanic_db::cache::Cache;
use lexmechanic_db::pdf_similarity::{minhash, sample_indices, words, PageHash, PdfFingerprint};

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
    path: std::path::PathBuf,
    _dir: Option<tempfile::TempDir>,
    src: tempfile::TempDir,
    cache: std::sync::Arc<Cache>,
    router: axum::Router,
}

fn lib() -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let mut l = lib_in(dir.path());
    l._dir = Some(dir);
    l
}

/// A library in a folder somebody else owns (so it can be reopened).
fn lib_in(path: &std::path::Path) -> Lib {
    let cache = std::sync::Arc::new(Cache::new(path).unwrap());
    let router = app_for(cache.clone());
    Lib { path: path.to_path_buf(), _dir: None, src: tempfile::tempdir().unwrap(), cache, router }
}

impl Lib {
    fn add(&self, name: &str, title: &str, author: &str, bytes: &[u8]) -> i32 {
        let source = self.src.path().join(name);
        std::fs::write(&source, bytes).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        self.cache.add_book(&source, &meta).unwrap()
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

    async fn groups(&self, query: &str) -> Value {
        let (status, body) = self.get(&format!("/duplicates/groups/default{query}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    fn folder_of(&self, id: i32) -> String {
        self.cache.field_for(id, "path").unwrap().unwrap_or_default()
    }
}

fn book_ids(group: &Value) -> Vec<i64> {
    let mut v: Vec<i64> = group["books"].as_array().unwrap().iter().map(|b| b["book_id"].as_i64().unwrap()).collect();
    v.sort_unstable();
    v
}

fn evidence_reasons(group: &Value) -> Vec<String> {
    group["evidence"].as_array().unwrap().iter().map(|e| e["reason"].as_str().unwrap().to_string()).collect()
}

// ---------------------------------------------------------------------------
// PDF fixtures: the same synthetic fingerprints the duplicates tests use
// ---------------------------------------------------------------------------

fn fingerprint(seed: u64, pages: u32, has_text: bool, text_seed: u64) -> PdfFingerprint {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let hashes = sample_indices(pages as usize).into_iter().map(|i| PageHash::new(i as u32, [next(), next(), next(), next()])).collect();
    let text: String = (0..3000).map(|i| format!("w{} ", (i as u64).wrapping_mul(text_seed + 7) % 2500)).collect();
    PdfFingerprint { page_count: pages, pages: hashes, text_chars: if has_text { 9000 } else { 0 }, has_text, scanned: true, minhash: if has_text { minhash(&words(&text)) } else { Vec::new() } }
}

fn record(cache: &Cache, book: i32, fp: PdfFingerprint) {
    let hash = cache.checksums().all_format_records().unwrap().into_iter().find(|((id, f), _)| *id == book && f.eq_ignore_ascii_case("pdf")).map(|(_, (h, _, _))| h).unwrap();
    cache.checksums().record_pdf_fingerprint(&hash, &Ok(fp)).unwrap();
}

/// `scan` re-rendered with a cover added: same pages one later, a few bits off.
fn ocr_of(scan: &PdfFingerprint) -> PdfFingerprint {
    let mut ocr = fingerprint(1, scan.page_count + 1, true, 3);
    let known: std::collections::HashMap<u32, [u64; 4]> = scan.pages.iter().map(|p| (p.index + 1, [p.bits[0] ^ 0b101, p.bits[1], p.bits[2] ^ 0b1000, p.bits[3]])).collect();
    for p in ocr.pages.iter_mut() {
        if let Some(h) = known.get(&p.index) {
            p.bits = *h;
        }
    }
    ocr
}

// ---------------------------------------------------------------------------
// GET /duplicates/compare
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compare_takes_two_to_eight_books() {
    let l = lib();
    let ids: Vec<i32> = (0..9).map(|i| l.add(&format!("b{i}.txt"), &format!("Book number {i}"), &format!("Writer {i}"), format!("bytes {i}").as_bytes())).collect();
    let csv = |n: usize| ids[..n].iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");

    assert_eq!(l.get("/duplicates/compare/default").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(l.get(&format!("/duplicates/compare/default?ids={}", ids[0])).await.0, StatusCode::BAD_REQUEST, "one book is not a comparison");
    // The same id twice is still one book.
    assert_eq!(l.get(&format!("/duplicates/compare/default?ids={0},{0}", ids[0])).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(l.get(&format!("/duplicates/compare/default?ids={}", csv(9))).await.0, StatusCode::BAD_REQUEST, "nine is too many");
    assert_eq!(l.get("/duplicates/compare/default?ids=1,x").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(l.get(&format!("/duplicates/compare/default?ids={},99999", ids[0])).await.0, StatusCode::NOT_FOUND);

    let (status, two) = l.get(&format!("/duplicates/compare/default?ids={}", csv(2))).await;
    assert_eq!(status, StatusCode::OK, "{two}");
    assert_eq!((two["books"].as_array().unwrap().len(), two["pairs"].as_array().unwrap().len()), (2, 1));
    let (status, eight) = l.get(&format!("/duplicates/compare/default?ids={}", csv(8))).await;
    assert_eq!(status, StatusCode::OK, "{eight}");
    assert_eq!((eight["books"].as_array().unwrap().len(), eight["pairs"].as_array().unwrap().len()), (8, 28));
}

#[tokio::test]
async fn compare_works_for_any_books_and_reports_identical_bytes_and_title_evidence() {
    let l = lib();
    // No scan has run: these were never detected as anything.
    let a = l.add("a.txt", "The Hobbit", "J.R.R. Tolkien", b"same bytes");
    let b = l.add("b.txt", "Hobbit, The", "Tolkien, J.R.R.", b"same bytes");
    let c = l.add("c.txt", "Something Else Entirely", "Another Author", b"other bytes");
    let d = l.add("d.txt", "The Hobbit", "J.R.R. Tolkien", b"different hobbit bytes");

    let (status, body) = l.get(&format!("/duplicates/compare/default?ids={a},{b},{c},{d}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pair = |x: i32, y: i32| body["pairs"].as_array().unwrap().iter().find(|p| p["a"] == x && p["b"] == y).unwrap().clone();

    assert_eq!(pair(a, b)["identical_bytes"], true, "{body}");
    assert_eq!(pair(a, b)["identical_formats"], json!(["TXT"]));
    assert_eq!(pair(a, c)["identical_bytes"], false);
    assert_eq!(pair(a, d)["identical_bytes"], false);
    // Plain text books: no PDF comparison at all.
    assert!(pair(a, b)["pdf"].is_null());
    // Title/author evidence, where there is any.
    assert!(pair(a, d)["evidence"].as_array().unwrap().iter().any(|e| e["reason"] == "title_author_exact"), "{}", pair(a, d));
    assert!(pair(a, c)["evidence"].as_array().unwrap().is_empty());
    assert_eq!(pair(a, b)["exempt"], false);

    // Per book: what a review card shows.
    let book = body["books"].as_array().unwrap().iter().find(|x| x["book_id"] == a).unwrap();
    assert_eq!(book["title"], "The Hobbit");
    assert_eq!(book["authors"][0], "J.R.R. Tolkien");
    assert_eq!(book["files"][0]["format"], "TXT");
    assert!(book["files"][0]["path"].as_str().unwrap().ends_with(".txt"));
    assert!(book["files"][0]["size"].as_i64().unwrap() > 0);
    assert!(book["files"][0]["mtime"].is_i64() || book["files"][0]["mtime"].is_u64());
    assert_eq!(book["format_list"], json!(["TXT"]));
    assert!(book["path"].is_string() && book.get("cover_url").is_some());
    // The suggested keeper of the set, with a reason, and marked on its book.
    let keeper = body["suggested_keeper"].as_i64().unwrap();
    assert!(!body["keeper_reason"].as_str().unwrap().is_empty());
    assert!(body["books"].as_array().unwrap().iter().any(|x| x["book_id"] == keeper && x["suggested"] == true));
    assert_eq!(body["books"].as_array().unwrap().iter().filter(|x| x["suggested"] == true).count(), 1);
}

#[tokio::test]
async fn compare_a_fingerprinted_pdf_pair_gives_the_graded_comparison_and_an_unfingerprinted_one_is_not_an_error() {
    let l = lib();
    let scan = l.add("scan.pdf", "scan0042", "Unknown", b"%PDF scan bytes");
    let ocr = l.add("ocr.pdf", "The Collected Essays (OCR)", "Somebody Else", b"%PDF ocr bytes with text");
    let not_yet = l.add("ny.pdf", "Not Analysed", "C. Writer", b"%PDF not analysed");
    let scan_fp = fingerprint(1, 300, false, 0);
    record(&l.cache, scan, scan_fp.clone());
    record(&l.cache, ocr, ocr_of(&scan_fp));

    let (status, body) = l.get(&format!("/duplicates/compare/default?ids={scan},{ocr},{not_yet}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pair = |x: i32, y: i32| body["pairs"].as_array().unwrap().iter().find(|p| p["a"] == x && p["b"] == y).unwrap().clone();

    let p = pair(scan, ocr)["pdf"].clone();
    assert_eq!(p["fingerprinted"], true, "{p}");
    assert_eq!(p["kind"], "pdf_ocr_variant");
    let conf = p["confidence"].as_f64().unwrap();
    assert!((0.72..=0.92).contains(&conf), "{conf}");
    assert_eq!(p["visual_match"], true);
    assert_eq!(p["page_counts"], json!([300, 301]));
    assert_eq!(p["has_text"], json!([false, true]));
    assert!(p["text_jaccard"].is_null(), "one side has no text: {p}");
    let pages = p["pages"].as_array().unwrap();
    assert_eq!(pages.len() as u64, p["compared"].as_u64().unwrap());
    assert_eq!(pages.iter().filter(|x| x["matched"] == true).count() as u64, p["matched"].as_u64().unwrap());
    assert_eq!(p["matched"], p["compared"], "every compared sample page matches: {p}");
    for page in pages {
        assert!(page["a"].is_u64() && page["b"].is_u64() && page["distance"].as_u64().unwrap() <= 256);
    }
    assert!(p["detail"].as_str().unwrap().contains("sample pages match"), "{p}");

    // Never fingerprinted: "not yet", with who is missing, and no error.
    let q = pair(scan, not_yet)["pdf"].clone();
    assert_eq!(q["fingerprinted"], false, "{q}");
    assert_eq!(q["missing"], json!([not_yet]));
    assert_eq!(body["pdf"], json!({"books": 3, "fingerprinted": 2}));

    // The copy with the text layer is the one to keep.
    assert_eq!(body["suggested_keeper"], ocr as i64, "{body}");
    assert!(body["keeper_reason"].as_str().unwrap().contains("text layer"), "{body}");
    assert_eq!(body["books"][0]["pdf"]["pages"], 300);
    assert_eq!(body["books"][0]["pdf"]["image_only"], true);
}

#[tokio::test]
async fn compare_shows_an_unrelated_fingerprinted_pair_as_not_a_match_with_its_distances() {
    let l = lib();
    let a = l.add("a.pdf", "Quiet Harbour", "A. Writer", b"%PDF a");
    let b = l.add("b.pdf", "Loud Mountain", "B. Writer", b"%PDF b");
    record(&l.cache, a, fingerprint(3, 200, false, 0));
    record(&l.cache, b, fingerprint(4, 200, false, 0));
    let (_, body) = l.get(&format!("/duplicates/compare/default?ids={a},{b}")).await;
    let p = &body["pairs"][0]["pdf"];
    assert_eq!(p["fingerprinted"], true);
    assert!(p["kind"].is_null() && p["confidence"].is_null(), "{p}");
    assert_eq!(p["visual_match"], false);
    assert!(!p["pages"].as_array().unwrap().is_empty());
    assert!(p["pages"].as_array().unwrap().iter().all(|x| x["matched"] == false));
}

// ---------------------------------------------------------------------------
// The user's groups
// ---------------------------------------------------------------------------

async fn three_unrelated(l: &Lib) -> [i32; 3] {
    [l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha"), l.add("b.txt", "Totally Different", "Bob Writer", b"bravo"), l.add("c.txt", "Another Thing", "Cy Author", b"charlie")]
}

#[tokio::test]
async fn a_group_the_user_makes_appears_in_the_snapshot_as_manual() {
    let l = lib();
    let [a, b, c] = three_unrelated(&l).await;
    assert_eq!(l.scan().await["groups"].as_array().unwrap().len(), 0);

    assert_eq!(l.post("/duplicates/manual-groups/default", json!({"book_ids": [a]})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(l.post("/duplicates/manual-groups/default", json!({"book_ids": [a, 99999]})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(l.post("/duplicates/manual-groups/default", json!({"group_id": "mg-nope", "add": [a]})).await.0, StatusCode::NOT_FOUND);

    let (status, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [b, a]})).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let gid = made["manual_group"]["id"].as_str().unwrap().to_string();
    assert!(gid.starts_with("mg-"));
    let group = &made["group"];
    assert_eq!(group["reason"], "manual", "{made}");
    assert_eq!(group["confidence"], 1.0);
    assert_eq!(group["manual_group_id"], gid.as_str());
    assert_eq!(book_ids(group), vec![a as i64, b as i64]);
    assert!(group["evidence"][0]["detail"].as_str().unwrap().contains("grouped by you"));
    assert!(group["suggested_dest"].is_i64(), "a keeper is suggested");

    // It is in the paged snapshot, filterable by reason, and the snapshot is not stale.
    let page = l.groups("").await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["stale"], false, "{page}");
    assert_eq!(page["facets"]["reason"]["manual"], 1);
    assert_eq!(l.groups("?reason=manual").await["total"], 1);
    assert_eq!(l.groups("?reason=content").await["total"], 0);
    assert_eq!(l.groups("?sort=reason").await["groups"][0]["reason"], "manual");

    // GET lists it with live members.
    let (_, list) = l.get("/duplicates/manual-groups/default").await;
    assert_eq!(list["count"], 1);
    assert_eq!(list["groups"][0]["id"], gid.as_str());
    assert_eq!(list["groups"][0]["book_ids"].as_array().unwrap().len(), 2);
    assert_eq!(list["groups"][0]["snapshot_group_id"], group["id"]);

    // Add a third, take it out again, then take out one of two: the group goes.
    let (_, added) = l.post("/duplicates/manual-groups/default", json!({"group_id": gid, "add": [c]})).await;
    assert_eq!(book_ids(&added["group"]), vec![a as i64, b as i64, c as i64]);
    assert_eq!(l.groups("").await["total"], 1);
    let (_, taken) = l.post("/duplicates/manual-groups/default", json!({"group_id": gid, "remove": [c]})).await;
    assert_eq!(book_ids(&taken["group"]), vec![a as i64, b as i64]);
    let (_, gone) = l.post("/duplicates/manual-groups/default", json!({"group_id": gid, "remove": [a]})).await;
    assert_eq!(gone["removed"], true, "{gone}");
    assert!(gone["manual_group"].is_null() && gone["group"].is_null());
    assert_eq!(l.groups("").await["total"], 0);
    assert_eq!(l.get("/duplicates/manual-groups/default").await.1["count"], 0);
    assert_eq!(l.post("/duplicates/manual-groups/default", json!({"group_id": gid, "remove": [b]})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(l.post("/duplicates/manual-groups/default", json!({"group_id": "mg-nope"})).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_group_removes_it_from_the_snapshot_and_a_group_works_before_any_scan() {
    let l = lib();
    let [a, b, _] = three_unrelated(&l).await;
    // No scan yet: the group is stored, there is no snapshot group to answer with.
    let (status, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [a, b]})).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert!(made["group"].is_null());
    let gid = made["manual_group"]["id"].as_str().unwrap().to_string();
    // The next scan includes it.
    let scan = l.scan().await;
    assert_eq!(scan["groups"][0]["reason"], "manual", "{scan}");

    let (status, _) = l.post("/duplicates/manual-groups/remove/default", json!({"group_id": gid})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(l.groups("").await["total"], 0);
    assert_eq!(l.post("/duplicates/manual-groups/remove/default", json!({"group_id": gid})).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_manual_group_that_overlaps_a_detected_one_is_one_group_keeping_all_the_evidence() {
    let l = lib();
    let d1 = l.add("d1.txt", "Dune", "Frank Herbert", b"dune one");
    let d2 = l.add("d2.txt", "Dune", "Frank Herbert", b"dune two");
    let other = l.add("o.txt", "Unrelated Thing", "Someone Else", b"nothing in common");
    let first = l.scan().await;
    assert_eq!(first["groups"].as_array().unwrap().len(), 1);
    assert_eq!(first["groups"][0]["reason"], "title_author_exact");

    let (_, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [d1, other]})).await;
    let group = &made["group"];
    assert_eq!(book_ids(group), vec![d1 as i64, d2 as i64, other as i64], "{made}");
    assert_eq!(group["reason"], "manual");
    let reasons = evidence_reasons(group);
    assert!(reasons.contains(&"title_author_exact".to_string()), "the detected evidence is kept: {reasons:?}");
    assert!(reasons.contains(&"manual".to_string()), "{reasons:?}");
    assert!(group["evidence"].as_array().unwrap().iter().any(|e| e["detail"] == "grouped by you"));
    let page = l.groups("").await;
    assert_eq!(page["total"], 1, "one group, not two: {page}");
    // The detected group is no longer listed on its own.
    assert_eq!(l.groups("?reason=title_author_exact").await["total"], 0);

    // Un-grouping the user's pair brings the detected group back as it was.
    let gid = made["manual_group"]["id"].as_str().unwrap();
    let (_, undone) = l.post("/duplicates/manual-groups/remove/default", json!({"group_id": gid})).await;
    assert_eq!(undone["removed"], true);
    let back = l.groups("").await;
    assert_eq!(back["total"], 1);
    assert_eq!(back["groups"][0]["reason"], "title_author_exact");
    assert_eq!(book_ids(&back["groups"][0]), vec![d1 as i64, d2 as i64]);
}

#[tokio::test]
async fn grouping_books_overrides_the_exemption_that_covers_exactly_them_and_not_duplicates_wins_afterwards() {
    let l = lib();
    let x = l.add("x.txt", "The Great Test", "Ada Lovelace", b"alpha");
    let y = l.add("y.txt", "The Great Test", "Ada Lovelace", b"bravo");
    l.scan().await;
    assert_eq!(l.groups("").await["total"], 1);

    let (status, ex) = l.post("/duplicates/exempt/default", json!({"book_ids": [x, y]})).await;
    assert_eq!(status, StatusCode::OK, "{ex}");
    assert_eq!(l.groups("").await["total"], 0, "dismissed");
    assert_eq!(l.get("/duplicates/exemptions/default").await.1["count"], 1);

    // The user then groups them: the newest action wins, and the pair is not split.
    let (_, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [x, y]})).await;
    assert_eq!(made["group"]["reason"], "manual", "{made}");
    assert_eq!(l.get("/duplicates/exemptions/default").await.1["count"], 0, "the exemption was overruled");
    let page = l.groups("").await;
    assert_eq!(page["total"], 1);
    // Even after a rescan (the matcher would have split nothing, and the manual group stands).
    l.scan().await;
    let page = l.groups("").await;
    assert_eq!((page["total"].as_u64(), page["groups"][0]["reason"].as_str()), (Some(1), Some("manual")), "{page}");

    // Then "not duplicates" again: that is the newest action, and the group goes.
    let (_, ex2) = l.post("/duplicates/exempt/default", json!({"group_id": page["groups"][0]["id"]})).await;
    assert_eq!(ex2["manual_groups_changed"].as_array().unwrap().len(), 1, "{ex2}");
    assert_eq!(l.groups("").await["total"], 0);
    assert_eq!(l.get("/duplicates/manual-groups/default").await.1["count"], 0);
}

#[tokio::test]
async fn manual_groups_survive_reopening_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, gid);
    {
        let l = lib_in(dir.path());
        let ids = three_unrelated(&l).await;
        a = ids[0];
        b = ids[1];
        let (_, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [a, b]})).await;
        gid = made["manual_group"]["id"].as_str().unwrap().to_string();
    }
    let l = lib_in(dir.path());
    let (_, list) = l.get("/duplicates/manual-groups/default").await;
    assert_eq!(list["groups"][0]["id"], gid.as_str(), "{list}");
    let scan = l.scan().await;
    assert_eq!(scan["groups"][0]["reason"], "manual");
    assert_eq!(book_ids(&scan["groups"][0]), vec![a as i64, b as i64]);
}

#[tokio::test]
async fn a_group_with_fewer_than_two_live_books_disappears() {
    let l = lib();
    let [a, b, c] = three_unrelated(&l).await;
    let (_, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [a, b, c]})).await;
    let gid = made["manual_group"]["id"].as_str().unwrap().to_string();
    assert_eq!(l.get("/duplicates/manual-groups/default").await.1["groups"][0]["book_ids"].as_array().unwrap().len(), 3);

    lexmechanic_db::removal::delete_with_file(&l.cache, c).unwrap();
    let (_, two) = l.get("/duplicates/manual-groups/default").await;
    assert_eq!(two["groups"][0]["book_ids"].as_array().unwrap().len(), 2, "{two}");
    assert_eq!(l.scan().await["groups"][0]["size"], 2);

    lexmechanic_db::removal::delete_with_file(&l.cache, b).unwrap();
    assert_eq!(l.get("/duplicates/manual-groups/default").await.1["count"], 0);
    assert_eq!(l.scan().await["groups"].as_array().unwrap().len(), 0);
    assert!(lexmechanic_db::dedupe_manual_groups::get(&l.cache, &gid).unwrap().is_none(), "pruned from storage too");
}

#[tokio::test]
async fn resolve_treats_manual_as_a_metadata_only_tier_and_resolving_ends_the_group() {
    let l = lib();
    let [a, b, _] = three_unrelated(&l).await;
    l.scan().await;
    let (_, made) = l.post("/duplicates/manual-groups/default", json!({"book_ids": [a, b]})).await;
    let group_id = made["group"]["id"].as_str().unwrap().to_string();

    // Not in the default tiers.
    let (_, dry) = l.post("/duplicates/resolve/default", json!({})).await;
    assert_eq!(dry["totals"]["groups"], 0, "{dry}");
    // Only on an explicit tick.
    let (status, dry) = l.post("/duplicates/resolve/default", json!({"tiers": ["manual"]})).await;
    assert_eq!(status, StatusCode::OK, "{dry}");
    assert_eq!(dry["totals"], json!({"groups": 1, "books": 1, "bytes_freed": dry["totals"]["bytes_freed"]}));
    assert_eq!(dry["groups"][0]["reason"], "manual");

    // Execute for exactly this group, keeping the one the user picked.
    let (_, plan) = l.post("/duplicates/resolve/default", json!({"group_ids": [group_id], "keepers": {group_id.clone(): b}})).await;
    assert_eq!(plan["groups"][0]["keeper"]["book_id"], b as i64, "{plan}");
    let fp = plan["plan_fingerprint"].clone();
    let (status, started) = l.post("/duplicates/resolve/default", json!({"dry_run": false, "group_ids": [group_id], "keepers": {group_id.clone(): b}, "plan_fingerprint": fp})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["books_trashed"], 1, "{job}");

    assert!(l.cache.book_uuid(a).unwrap().is_none(), "the other copy is in the trash");
    assert_eq!(l.groups("").await["total"], 0, "the group is resolved");
    assert_eq!(l.get("/duplicates/manual-groups/default").await.1["count"], 0, "and so is the stored one");
}

// ---------------------------------------------------------------------------
// POST /duplicates/refresh-books
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refreshing_books_after_the_callers_own_edit_keeps_the_snapshot_current() {
    let l = lib();
    let x = l.add("x.txt", "The Great Test", "Ada Lovelace", b"alpha");
    let y = l.add("y.txt", "The Great Test", "Ada Lovelace", b"bravo");
    l.scan().await;
    // An edit made outside the duplicates routes goes stale...
    l.cache.set_field(x, "title", "The Great Test, Revised").unwrap();
    assert_eq!(l.groups("").await["stale"], true);
    // ... until the caller says the edit was its own.
    let (status, out) = l.post("/duplicates/refresh-books/default", json!({"book_ids": [x]})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["refreshed"], 1);
    let page = l.groups("").await;
    assert_eq!(page["stale"], false, "{page}");
    let title = page["groups"][0]["books"].as_array().unwrap().iter().find(|b| b["book_id"] == x).unwrap()["title"].clone();
    assert_eq!(title, "The Great Test, Revised");
    let _ = y;
}

// ---------------------------------------------------------------------------
// POST /move-books
// ---------------------------------------------------------------------------

#[tokio::test]
async fn move_books_dry_run_says_what_it_would_do_and_changes_nothing() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let old_dir = l.folder_of(a);
    let (status, plan) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Sorted/Here"})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!((plan["books"].as_u64(), plan["files"].as_u64()), (Some(1), Some(1)));
    assert_eq!(plan["folder"], "Sorted/Here");
    assert!(plan["moves"][0]["to"].as_str().unwrap().starts_with("Sorted/Here/"), "{plan}");
    assert!(plan["moves"][0]["from"].as_str().unwrap().starts_with(&old_dir));
    assert!(plan["plan_fingerprint"].is_string());
    assert_eq!(l.folder_of(a), old_dir);
    assert!(!l.path.join("Sorted").exists(), "a dry run moves nothing");
}

#[tokio::test]
async fn move_books_rejects_a_folder_outside_the_library_and_bad_books() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    for bad in ["../outside", "a/../../b", "/etc", "\\\\server\\share", "C:/Windows", "C:\\temp", "", "  ", "..", ".lexmechanic", "x/.caltrash", "bad|name", "NUL"] {
        for dry_run in [true, false] {
            let (status, body) = l.post("/move-books/default", json!({"book_ids": [a], "folder": bad, "dry_run": dry_run})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?} dry_run {dry_run}: {body}");
        }
    }
    assert_eq!(l.post("/move-books/default", json!({"book_ids": [], "folder": "ok"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(l.post("/move-books/default", json!({"book_ids": [a, 99999], "folder": "ok"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(l.post("/move-books/default", json!({"folder": "ok"})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!l.path.join("outside").exists());
}

#[tokio::test]
async fn move_books_applies_as_a_job_is_recorded_and_is_undone_by_unorganize() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let b = l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    let (old_a, old_b) = (l.folder_of(a), l.folder_of(b));
    let (_, plan) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Keep/These"})).await;

    // A wrong fingerprint is refused.
    let (status, _) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Keep/These", "dry_run": false, "plan_fingerprint": "nope"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // So is executing with no fingerprint at all: that is moving files
    // without a confirmed preview.
    let (status, _) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Keep/These", "dry_run": false})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(l.folder_of(a), old_a, "nothing moved");
    let (status, started) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Keep/These", "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["books_moved"], 1, "{job}");
    let run_id = job["report"]["run_id"].as_str().unwrap().to_string();

    assert_eq!(l.folder_of(a), "Keep/These");
    assert!(l.path.join("Keep/These/a.txt").is_file(), "the file keeps its name");
    assert!(!l.path.join("a.txt").exists());
    assert_eq!(l.folder_of(b), old_b, "other books stay");
    // Already there: nothing to move.
    let (status, _) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Keep/These", "dry_run": false})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Recorded, and undone through the organize routes.
    let (_, runs) = l.get("/organize-runs/default").await;
    assert!(runs["runs"].as_array().unwrap().iter().any(|r| r["run_id"] == run_id.as_str() && r["undone"] == false), "{runs}");
    // The listing says what the run did, so a move is not mistaken for a rename.
    let listed = runs["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run_id.as_str()).unwrap();
    assert_eq!(listed["template"], "move to Keep/These", "{runs}");
    let (status, undo) = l.post("/unorganize-library/default", json!({"run_id": run_id})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{undo}");
    let done = l.wait_job(&undo).await;
    assert_eq!(done["report"]["books_restored"], 1, "{done}");
    assert_eq!(l.folder_of(a), old_a);
    assert!(l.path.join(&old_a).join("a.txt").is_file());
    assert!(!l.path.join("Keep").exists(), "the folders the move made are gone again");
}

#[tokio::test]
async fn move_books_numbers_a_taken_name_and_the_move_survives_a_rebuild() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let b = l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    // a goes to Pile; then b's file is named like it, so b's move finds the name taken.
    let (_, plan) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Pile"})).await;
    let (_, started) = l.post("/move-books/default", json!({"book_ids": [a], "folder": "Pile", "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    l.wait_job(&started).await;
    l.cache.rename_format_files(b, "a").unwrap();

    let (_, plan) = l.post("/move-books/default", json!({"book_ids": [b], "folder": "Pile"})).await;
    assert_eq!((plan["books"].as_u64(), plan["conflicts"].as_u64()), (Some(1), Some(1)), "{plan}");
    assert_eq!(plan["moves"][0]["to"], "Pile/a (2).txt");
    let (status, started) = l.post("/move-books/default", json!({"book_ids": [b], "folder": "Pile", "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!((job["report"]["books_moved"].as_u64(), job["report"]["failed"].as_array().map(|f| f.len())), (Some(1), Some(0)), "{job}");
    let mut names: Vec<String> = std::fs::read_dir(l.path.join("Pile")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["a (2).txt", "a.txt"]);
    assert_eq!(std::fs::read(l.path.join("Pile/a.txt")).unwrap(), b"alpha", "nothing was overwritten");

    l.cache.rebuild_from_change_log().unwrap();
    for id in l.cache.all_book_ids().unwrap() {
        assert_eq!(l.folder_of(id), "Pile", "book {id} after a rebuild");
    }
}

/// Applying a plan that was just previewed reuses it (no second planning pass)
/// while the library has not changed; once it has, the plan is made again and
/// the fingerprint decides, as it always did.
#[tokio::test]
async fn a_previewed_plan_is_reused_to_apply_until_the_library_changes() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let (_, plan) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Sorted/{title}"})).await;
    let fingerprint = plan["plan_fingerprint"].as_str().unwrap().to_string();
    let key = json!({"organize": "Sorted/{title}", "book_ids": [a], "search": null, "path_prefix": null}).to_string();
    let remembered = crate::organize_library::recall_plan(&l.cache, &key, &fingerprint).expect("the dry run's plan is remembered");
    assert_eq!(remembered.fingerprint, fingerprint);
    assert!(crate::organize_library::recall_plan(&l.cache, &key, "other").is_none());
    let other_request = json!({"organize": "Other/{title}", "book_ids": [a], "search": null, "path_prefix": null}).to_string();
    assert!(crate::organize_library::recall_plan(&l.cache, &other_request, &fingerprint).is_none(), "a plan is only reused for the request that made it");

    // The library changes: the remembered plan is no longer trusted.
    l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    assert!(crate::organize_library::recall_plan(&l.cache, &key, &fingerprint).is_none());

    // Applying still works (it plans again and finds the same fingerprint) ...
    let (status, started) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Sorted/{title}", "dry_run": false, "plan_fingerprint": fingerprint})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["books_moved"], 1, "{job}");
    assert!(l.path.join("Sorted/Alpha Primer.txt").is_file());

    // ... and the same with an unchanged library: preview, then apply straight away.
    let (_, plan) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Moved/{title}"})).await;
    let key = json!({"organize": "Moved/{title}", "book_ids": [a], "search": null, "path_prefix": null}).to_string();
    assert!(crate::organize_library::recall_plan(&l.cache, &key, plan["plan_fingerprint"].as_str().unwrap()).is_some());
    let (status, started) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Moved/{title}", "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(job["report"]["books_moved"], 1, "{job}");
    assert!(l.path.join("Moved/Alpha Primer.txt").is_file());
}

/// A write by another process (calibre desktop on the same `metadata.db`)
/// moves neither the change log nor the row counts; a remembered plan must not
/// survive it. And an applied plan is spent.
#[tokio::test]
async fn a_remembered_plan_is_dropped_after_another_process_writes_and_after_it_is_applied() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let request = |template: &str| json!({"organize": template, "book_ids": [a], "search": null, "path_prefix": null}).to_string();

    let (_, plan) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Sorted/{title}"})).await;
    let fingerprint = plan["plan_fingerprint"].as_str().unwrap().to_string();
    assert!(crate::organize_library::recall_plan(&l.cache, &request("Sorted/{title}"), &fingerprint).is_some());
    {
        let other = rusqlite::Connection::open(l.path.join("metadata.db")).unwrap();
        other.execute("INSERT INTO tags (name) VALUES ('written by another process')", []).unwrap();
    }
    assert!(crate::organize_library::recall_plan(&l.cache, &request("Sorted/{title}"), &fingerprint).is_none(), "an edit from another process invalidates the plan");

    let (_, plan) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Moved/{title}"})).await;
    let fingerprint = plan["plan_fingerprint"].as_str().unwrap().to_string();
    assert!(crate::organize_library::recall_plan(&l.cache, &request("Moved/{title}"), &fingerprint).is_some());
    let (status, started) = l.post("/organize-library/default", json!({"book_ids": [a], "template": "Moved/{title}", "dry_run": false, "plan_fingerprint": fingerprint})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert!(crate::organize_library::recall_plan(&l.cache, &request("Moved/{title}"), &fingerprint).is_none(), "an applied plan is forgotten");
    l.wait_job(&started).await;
}

// ---------------------------------------------------------------------------
// Folders and the naming convention (tranche 19b)
// ---------------------------------------------------------------------------

async fn move_into(l: &Lib, id: i32, folder: &str) {
    let (_, plan) = l.post("/move-books/default", json!({"book_ids": [id], "folder": folder})).await;
    let (_, started) = l.post("/move-books/default", json!({"book_ids": [id], "folder": folder, "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    l.wait_job(&started).await;
}

#[tokio::test]
async fn library_folders_lists_nested_counts_and_an_empty_library_has_none() {
    let l = lib();
    let (status, tree) = l.get("/library-folders/default").await;
    assert_eq!((status, tree["total"].as_u64(), tree["folders"].as_array().unwrap().len()), (StatusCode::OK, Some(0), 0));
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let b = l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    move_into(&l, a, "Gaming/RPG/Rifts").await;
    move_into(&l, b, "Gaming/RPG/Rifts/Sub").await;
    let (_, tree) = l.get("/library-folders/default").await;
    let rows: Vec<(String, u64, u64)> = tree["folders"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap().to_string(), f["books"].as_u64().unwrap(), f["books_here"].as_u64().unwrap())).collect();
    assert_eq!(rows, [("Gaming".to_string(), 2, 0), ("Gaming/RPG".to_string(), 2, 0), ("Gaming/RPG/Rifts".to_string(), 2, 1), ("Gaming/RPG/Rifts/Sub".to_string(), 1, 1)]);

    let (status, made) = l.post("/library-folders/default", json!({"path": "Gaming/RPG/Later"})).await;
    assert_eq!((status, made["path"].as_str()), (StatusCode::OK, Some("Gaming/RPG/Later")));
    assert!(l.path.join("Gaming/RPG/Later").is_dir());
    assert_eq!(l.post("/library-folders/default", json!({"path": "../x"})).await.0, StatusCode::BAD_REQUEST);
    let (_, tree) = l.get("/library-folders/default").await;
    assert!(tree["folders"].as_array().unwrap().iter().any(|f| f["path"] == "Gaming/RPG/Later" && f["empty"] == true));
    assert_eq!(l.post("/library-folders/delete/default", json!({"path": "Gaming/RPG/Later"})).await.0, StatusCode::OK);
    assert!(!l.path.join("Gaming/RPG/Later").exists());
}

#[tokio::test]
async fn rename_folder_previews_applies_undoes_and_refuses_a_collision() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let b = l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    move_into(&l, a, "Gaming/Rifts").await;
    move_into(&l, b, "Gaming/Palladium").await;
    let (status, _) = l.post("/rename-folder/default", json!({"from": "Gaming/Rifts", "to": "Gaming/Palladium"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a rename onto an existing folder is refused");
    let (status, plan) = l.post("/rename-folder/default", json!({"from": "Gaming/Rifts", "to": "Gaming/Rifts RPG"})).await;
    assert_eq!((status, plan["books"].as_u64()), (StatusCode::OK, Some(1)), "{plan}");
    assert!(l.path.join("Gaming/Rifts").exists(), "a dry run moves nothing");
    assert_eq!(l.post("/rename-folder/default", json!({"from": "Gaming/Rifts", "to": "Gaming/Rifts RPG", "dry_run": false, "plan_fingerprint": "nope"})).await.0, StatusCode::CONFLICT);
    let (status, started) = l.post("/rename-folder/default", json!({"from": "Gaming/Rifts", "to": "Gaming/Rifts RPG", "dry_run": false, "plan_fingerprint": plan["plan_fingerprint"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = l.wait_job(&started).await;
    assert_eq!(l.folder_of(a), "Gaming/Rifts RPG");
    let (status, undone) = l.post("/unorganize-library/default", json!({"run_id": job["report"]["run_id"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{undone}");
    l.wait_job(&undone).await;
    assert_eq!(l.folder_of(a), "Gaming/Rifts");
}

#[tokio::test]
async fn the_naming_convention_round_trips_and_previews() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let (_, n) = l.get("/naming/default").await;
    assert_eq!((n["combined"].as_str(), n["folder_template"].as_str()), (Some("{author_sort}/{title}"), Some("{author_sort}")));
    let (status, saved) = l.call("PUT", "/naming/default", Some(json!({"folder_template": "Gaming/{series}", "file_template": "{series:|| }{title}"}))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(l.get("/naming/default").await.1["combined"], "Gaming/{series}/{series:|| }{title}");
    assert_eq!(l.call("PUT", "/naming/default", Some(json!({"folder_template": "x", "file_template": ""}))).await.0, StatusCode::BAD_REQUEST);
    let (status, p) = l.post("/naming-preview/default", json!({"folder_template": "Gaming/{series}", "file_template": "{series:|| }{title}", "book_ids": [a]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["previews"][0]["path"], "Gaming/Alpha Primer.txt", "{p}");
    // No template given to organize: the convention is the default.
    let (_, plan) = l.post("/organize-library/default", json!({"book_ids": [a]})).await;
    assert_eq!(plan["template"], "Gaming/{series}/{series:|| }{title}");
}

#[tokio::test]
async fn a_text_custom_column_with_values_is_a_category_without_a_reload() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    l.cache.add_custom_column("game_line", "Game line", "text", false).unwrap();
    let (_, cats) = l.get("/ajax/categories").await;
    assert!(!cats.as_array().unwrap().iter().any(|c| c["name"] == "Game line"), "no values yet, so nothing to browse");
    l.cache.set_custom_column_value(a, "game_line", "Rifts").unwrap();
    let (_, cats) = l.get("/ajax/categories").await;
    let entry = cats.as_array().unwrap().iter().find(|c| c["name"] == "Game line").expect("the column is a category");
    assert_eq!(entry["url"], "/ajax/category/#game_line");
    let (status, page) = l.get("/ajax/category/%23game_line").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!((page["category_name"].as_str(), page["items"][0]["name"].as_str(), page["items"][0]["count"].as_i64()), (Some("Game line"), Some("Rifts"), Some(1)));
}

/// calibre's `Author/Title (id)` folders are storage, not folders anyone made: the tree leaves them out
/// unless asked, and says how many there are so the page can offer them.
#[tokio::test]
async fn library_folders_leaves_out_author_folders_unless_asked() {
    let l = lib();
    let a = l.add("a.txt", "Alpha Primer", "Ada Lovelace", b"alpha");
    let b = l.add("b.txt", "Totally Different", "Bob Writer", b"bravo");
    l.cache.set_book_path(a, &format!("Lovelace, Ada/Alpha Primer ({a})")).unwrap();
    move_into(&l, b, "Gaming").await;

    let (_, tree) = l.get("/library-folders/default").await;
    let paths = |t: &serde_json::Value| t["folders"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(paths(&tree), ["Gaming"], "{tree}");
    assert_eq!(tree["total"], 2, "the books are all still counted");
    assert_eq!((tree["author_folders"].as_u64(), tree["author_folders_hidden"].as_bool()), (Some(1), Some(true)));

    let (_, all) = l.get("/library-folders/default?author_folders=true").await;
    assert_eq!(paths(&all), ["Gaming", "Lovelace, Ada"], "{all}");
    assert!(all["folders"][1]["author"].as_bool().unwrap());
    assert!(all["folders"][0].get("author").is_none(), "only an author folder says so");
}
