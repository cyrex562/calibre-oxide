use axum::http::StatusCode;
use serde_json::{json, Value};

use crate::library_trash::tests::{app, call};

async fn finished(router: &axum::Router, job_id: u64) -> Value {
    for _ in 0..200 {
        let (_, job) = call(router, "GET", &format!("/library-jobs/{job_id}"), None).await;
        if job["state"] != "running" {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the import job did not finish");
}

#[tokio::test]
async fn a_folder_is_imported_as_a_job_with_a_report() {
    let (dir, cache, router) = app();
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("nested/deeper")).unwrap();
    std::fs::write(src.path().join("one.pdf"), b"%PDF one").unwrap();
    std::fs::write(src.path().join("nested/two.pdf"), b"%PDF two").unwrap();
    std::fs::write(src.path().join("nested/deeper/three.pdf"), b"%PDF three").unwrap();

    let (status, started) = call(&router, "POST", "/import-folder/-", Some(json!({"source": src.path().to_string_lossy()}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["kind"], "import");
    let job = finished(&router, started["job_id"].as_u64().unwrap()).await;

    assert_eq!(job["state"], "finished", "{job}");
    assert_eq!(job["report"]["books_added"], 3, "{job}");
    assert_eq!(job["report"]["conclusive"], true);
    // The import ends with the pass that renders the covers it left for later.
    assert_eq!(job["progress"]["phase"], "enriching");
    assert!(job["report"]["enrichment"].is_object(), "{job}");
    assert_eq!(cache.all_book_ids().unwrap().len(), 3);
    // Copied in, in calibre's layout, with the source left alone.
    let ids = cache.all_book_ids().unwrap();
    let path = cache.field_for(ids[0], "path").unwrap().unwrap();
    assert!(path.starts_with("Unknown/") && path.ends_with(&format!(" ({})", ids[0])), "{path}");
    assert!(dir.path().join(&path).is_dir());
    assert_eq!(std::fs::read(src.path().join("one.pdf")).unwrap(), b"%PDF one");
}

#[tokio::test]
async fn the_options_are_taken_from_the_body() {
    let (_dir, cache, router) = app();
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("sub")).unwrap();
    std::fs::write(src.path().join("top.pdf"), b"%PDF top").unwrap();
    std::fs::write(src.path().join("sub/deep.pdf"), b"%PDF deep").unwrap();

    let (_, started) = call(&router, "POST", "/import-folder/-", Some(json!({"source": src.path().to_string_lossy(), "recursive": false, "mode": "one_book_per_folder", "policy": "add_anyway"}))).await;
    let job = finished(&router, started["job_id"].as_u64().unwrap()).await;

    assert_eq!(job["report"]["books_added"], 1, "{job}");
    assert_eq!(cache.all_book_ids().unwrap().len(), 1);
}

/// Every mode and policy the web UI offers (`web/src/library/importFolder.ts`)
/// is accepted, spelled as it sends them. The UI's default once failed with a
/// 422 because the server spelled the mode `multiple_per_folder`.
#[tokio::test]
async fn every_option_the_web_ui_sends_is_accepted() {
    for mode in ["multiple_books_per_folder", "one_book_per_folder", "multiple_per_folder"] {
        for policy in ["skip_exact_content", "skip_title_author", "add_formats_to_existing", "add_anyway"] {
            let (_dir, cache, router) = app();
            let src = tempfile::tempdir().unwrap();
            std::fs::write(src.path().join("a.pdf"), b"%PDF a").unwrap();
            let body = json!({"source": src.path().to_string_lossy(), "mode": mode, "policy": policy, "recursive": true});
            let (status, started) = call(&router, "POST", "/import-folder/-", Some(body)).await;
            assert_eq!(status, StatusCode::ACCEPTED, "{mode}/{policy}: {started}");
            let job = finished(&router, started["job_id"].as_u64().unwrap()).await;
            assert_eq!(job["report"]["books_added"], 1, "{mode}/{policy}: {job}");
            assert_eq!(cache.all_book_ids().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn a_bad_source_is_refused_before_a_job_starts() {
    let (dir, _cache, router) = app();
    for (body, why) in [
        (json!({"source": ""}), "empty"),
        (json!({"source": "relative/folder"}), "relative"),
        (json!({"source": dir.path().join("does-not-exist").to_string_lossy()}), "missing"),
        (json!({"source": dir.path().to_string_lossy()}), "the library itself"),
    ] {
        let (status, body) = call(&router, "POST", "/import-folder/-", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    }
    let (status, _) = call(&router, "POST", "/import-folder/-", Some(json!({"source": "/x", "mode": "sideways"}))).await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn running_it_again_imports_nothing_new() {
    let (_dir, cache, router) = app();
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("one.pdf"), b"%PDF one").unwrap();
    for expected in [1, 0] {
        let (_, started) = call(&router, "POST", "/import-folder/-", Some(json!({"source": src.path().to_string_lossy()}))).await;
        let job = finished(&router, started["job_id"].as_u64().unwrap()).await;
        assert_eq!(job["report"]["books_added"], expected, "{job}");
    }
    assert_eq!(cache.all_book_ids().unwrap().len(), 1);
}
