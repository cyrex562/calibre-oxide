//! Black-box tests of a really-spawned `calibre_srv` hosting more than
//! one library (#959, #816 item 1.4).
//!
//! The route and the broker were both real and unit-tested before this;
//! what was missing was a *running server* that ever had a second
//! library, because `main.rs` hardcoded `libraries: None`. That is
//! exactly the kind of gap only a black-box test sees.

use std::path::{Path, PathBuf};
use std::time::Duration;

use calibre_oxide_e2e::{find_free_port, oxide_bin, spawn_calibre_srv_with_libraries, wait_until_ready, TestLibrary};

fn web_dist() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.join("web").join("dist");
    dir.join("index.html").is_file().then_some(dir)
}

fn add_book(library: &Path, name: &str) {
    let book = library.join(format!("{name}.txt"));
    std::fs::write(&book, format!("{name}\n\n\nJane Doe\n")).unwrap();
    let out = std::process::Command::new(oxide_bin("calibredb"))
        .arg("add")
        .arg(&book)
        .arg("--with-library")
        .arg(library)
        .output()
        .expect("failed to run calibredb");
    assert!(out.status.success(), "calibredb add failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// The whole point of item 1.4: a book moves from one library to another
/// through the real server.
#[test]
fn a_real_server_copies_a_book_between_two_hosted_libraries() {
    let Some(static_dir) = web_dist() else {
        eprintln!("skipping: web/dist not built -- run `npm run build` in web/ first");
        return;
    };

    let home = TestLibrary::new();
    let archive = TestLibrary::new();
    add_book(home.path(), "Travelling Book");

    // `library_id` comes from the folder's own name, so read it back
    // rather than guessing at the temp directory's name.
    let archive_id = archive.path().file_name().unwrap().to_str().unwrap().replace(' ', "_");

    let port = find_free_port();
    let _srv = spawn_calibre_srv_with_libraries(home.path(), &[archive.path()], &static_dir, port);
    assert!(wait_until_ready(port, Duration::from_secs(10)), "calibre_srv never became ready on port {port}");

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();

    // Both libraries are really advertised, which is what the UI's
    // library switcher reads.
    let info: serde_json::Value = client.get(format!("{base}/ajax/library-info")).send().unwrap().json().unwrap();
    let map = info["library_map"].as_object().expect("expected a library_map");
    assert_eq!(map.len(), 2, "the server should advertise both libraries: {info}");
    assert!(map.contains_key(&archive_id), "expected {archive_id:?} in {info}");

    // Exactly the request web/src/library/api.ts builds for copy-to-library.
    let response: serde_json::Value = client
        .post(format!("{base}/cdb/copy-to-library/{archive_id}/-"))
        .json(&serde_json::json!({"book_ids": [1], "move_books": false, "duplicate_action": "add"}))
        .send()
        .expect("copy-to-library request failed")
        .json()
        .expect("response was not valid JSON");
    assert_eq!(response["1"]["ok"], true, "{response}");

    // Really in the other library, read back through a second server
    // pointed at it -- not from the response that claimed success.
    let check_port = find_free_port();
    let _check = spawn_calibre_srv_with_libraries(archive.path(), &[], &static_dir, check_port);
    assert!(wait_until_ready(check_port, Duration::from_secs(10)), "the checking server never became ready");
    let search: serde_json::Value = client
        .get(format!("http://127.0.0.1:{check_port}/ajax/search"))
        .query(&[("query", ""), ("num", "24"), ("offset", "0")])
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(search["total_num"], 1, "the book did not arrive in the archive library: {search}");
}

/// The regression this change had to avoid: the web UI fills every
/// `{library_id}` segment with `-`, and the broker knows no library by
/// that name.
#[test]
fn a_dash_library_id_still_reaches_the_default_library_over_http() {
    let Some(static_dir) = web_dist() else {
        eprintln!("skipping: web/dist not built -- run `npm run build` in web/ first");
        return;
    };

    let home = TestLibrary::new();
    let archive = TestLibrary::new();
    add_book(home.path(), "Default Library Book");

    let port = find_free_port();
    let _srv = spawn_calibre_srv_with_libraries(home.path(), &[archive.path()], &static_dir, port);
    assert!(wait_until_ready(port, Duration::from_secs(10)), "calibre_srv never became ready on port {port}");

    // `/orphans/{library_id}` is a real library_id-taking route that
    // returns JSON, and the UI calls this family with `-`.
    let client = reqwest::blocking::Client::new();
    let response = client.get(format!("http://127.0.0.1:{port}/orphans/-")).send().expect("orphans request failed");
    assert_eq!(response.status(), 200, "a `-` library id must reach the default library rather than 404");
    let body: serde_json::Value = response.json().expect("response was not valid JSON");
    assert!(body.is_object() || body.is_array(), "expected a JSON body, got: {body}");

    // And a library the server really does host is still addressable by
    // name, so normalising `-` did not flatten every id to the default.
    let archive_id = archive.path().file_name().unwrap().to_str().unwrap().replace(' ', "_");
    let named = client.get(format!("http://127.0.0.1:{port}/orphans/{archive_id}")).send().unwrap();
    assert_eq!(named.status(), 200, "the second library should be addressable by its own id");

    let unknown = client.get(format!("http://127.0.0.1:{port}/orphans/NoSuchLibrary")).send().unwrap();
    assert_eq!(unknown.status(), 404, "an unknown library id must still 404 rather than silently using the default");
}
