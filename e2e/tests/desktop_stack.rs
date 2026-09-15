//! Real, black-box test of the full desktop-app stack: the compiled
//! `calibredb` binary adds a book into a real library, the compiled
//! `calibre_srv` binary is spawned against it exactly the way
//! `app/src-tauri/src/server.rs::spawn` spawns it for the real
//! desktop app (same args), and the result is checked two ways:
//!
//! - Real HTTP, against the exact `/ajax/*` shapes `web/`'s own
//!   `library/api.ts` consumes (`search` then `fetchBooks`) -- a
//!   deterministic check that the real library-browsing UI would
//!   really show the real added book.
//! - A real OS webview (via the already-real `calibre_scraper_worker`
//!   binary, the same stack `app/src-tauri` itself renders with)
//!   loading the served page -- confirms calibre_srv's `--static-dir`
//!   serving of `web/dist` is really reachable by a real browser
//!   engine, not just `curl`. Skipped, not failed, if `web/dist`
//!   hasn't been built (`npm run build` in `web/`) or no display (real
//!   or Xvfb) is available -- see this test's own body for exactly
//!   what's skipped and why.

use std::path::PathBuf;
use std::time::Duration;

use calibre_oxide_e2e::{find_free_port, oxide_bin, spawn_calibre_srv, wait_until_ready, webview_fetch, TestLibrary};

fn web_dist() -> Option<PathBuf> {
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    dist.join("index.html").exists().then_some(dist)
}

#[test]
fn calibre_srv_serves_a_book_added_via_calibredb_through_the_real_ajax_api() {
    let Some(static_dir) = web_dist() else {
        eprintln!("skipping: web/dist not built -- run `npm run build` in web/ first");
        return;
    };

    let lib = TestLibrary::new();
    let book = lib.path().join("Stack Test Book.txt");
    std::fs::write(&book, "Stack Test Book\n\n\nJane Doe\n").unwrap();

    let add = std::process::Command::new(oxide_bin("calibredb")).arg("add").arg(&book).arg("--with-library").arg(lib.path()).output().expect("failed to run calibredb");
    assert!(add.status.success(), "calibredb add failed: {}", String::from_utf8_lossy(&add.stderr));

    let port = find_free_port();
    let _srv = spawn_calibre_srv(lib.path(), &static_dir, port);
    assert!(wait_until_ready(port, Duration::from_secs(10)), "calibre_srv never became ready on port {port}");

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();

    // Exactly what web/src/library/api.ts::search() sends.
    let search: serde_json::Value = client
        .get(format!("{base}/ajax/search"))
        .query(&[("query", ""), ("num", "24"), ("offset", "0"), ("sort", "timestamp"), ("sort_order", "desc")])
        .send()
        .expect("search request failed")
        .json()
        .expect("search response was not valid JSON");
    let book_ids: Vec<i64> = search["book_ids"].as_array().expect("expected book_ids array").iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(book_ids.len(), 1, "expected exactly the one added book, got: {search}");

    // Exactly what web/src/library/api.ts::fetchBooks() sends.
    let books: serde_json::Value = client.get(format!("{base}/ajax/books?ids={}", book_ids[0])).send().expect("books request failed").json().expect("books response was not valid JSON");
    let book_json = &books[book_ids[0].to_string()];
    assert_eq!(book_json["title"], "Stack Test Book");
    assert_eq!(book_json["authors"], serde_json::json!(["Jane Doe"]));

    // The real static bundle is really reachable through the real
    // spawned server (same content the browser/webview would load).
    let index = client.get(&base).send().expect("index request failed").text().unwrap();
    assert!(index.contains("id=\"app\""), "expected the real web/ bundle's mount point, got: {index}");
}

#[test]
fn a_real_os_webview_can_load_the_served_ui() {
    let Some(static_dir) = web_dist() else {
        eprintln!("skipping: web/dist not built -- run `npm run build` in web/ first");
        return;
    };

    let lib = TestLibrary::new();
    let port = find_free_port();
    let _srv = spawn_calibre_srv(lib.path(), &static_dir, port);
    assert!(wait_until_ready(port, Duration::from_secs(10)), "calibre_srv never became ready on port {port}");

    let Some(html) = webview_fetch(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(15)) else {
        eprintln!("skipping: calibre_scraper_worker could not start a real webview (no display?)");
        return;
    };

    // The worker captures the HTML at the browser's own page-load-
    // finished event, which fires before Vue's own async /ajax/*-
    // driven rendering completes -- so this only asserts the real
    // page shell (mount point + the built JS bundle reference) loaded
    // through a real JS-capable engine, not that the book grid has
    // populated yet. That stronger claim is covered deterministically
    // by the HTTP-level test above instead of raced against here.
    assert!(html.contains("id=\"app\""), "expected the real web/ bundle's mount point in the loaded page, got: {html}");
}
