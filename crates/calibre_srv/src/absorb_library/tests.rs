use axum::http::StatusCode;
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::library_trash::tests::{app, call};

/// calibre's own schema, as calibre executes it on a new library.
const METADATA_SQLITE: &str = include_str!("../../../../old_src/resources/metadata_sqlite.sql");

/// A calibre-made library with two books, at `root`.
fn calibre_library(root: &std::path::Path) {
    std::fs::create_dir_all(root).unwrap();
    let conn = Connection::open(root.join("metadata.db")).unwrap();
    conn.create_scalar_function("title_sort", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, |ctx| ctx.get::<String>(0)).unwrap();
    conn.create_scalar_function("uuid4", 0, FunctionFlags::SQLITE_UTF8, |_| Ok(format!("{:032x}", rand::random::<u128>()))).unwrap();
    conn.create_scalar_function("books_list_filter", 1, FunctionFlags::SQLITE_UTF8, |_| Ok(true)).unwrap();
    conn.execute_batch(METADATA_SQLITE).unwrap();
    for (id, title, author) in [(1, "First Book", "Ann Author"), (2, "Second Book", "Ben Writer")] {
        let rel = format!("{author}/{title} ({id})");
        conn.execute("INSERT INTO books(id, title, author_sort, path, has_cover, timestamp, pubdate, last_modified) VALUES (?1, ?2, ?3, ?4, 0, '2020-01-02 03:04:05+00:00', '2019-01-01 00:00:00+00:00', '2020-01-02 03:04:05+00:00')", params![id, title, author, rel]).unwrap();
        conn.execute("INSERT INTO authors(name, sort) VALUES (?1, ?1)", [author]).unwrap();
        let aid = conn.last_insert_rowid();
        conn.execute("INSERT INTO books_authors_link(book, author) VALUES (?1, ?2)", params![id, aid]).unwrap();
        let stem = format!("{title} - {author}");
        std::fs::create_dir_all(root.join(&rel)).unwrap();
        let bytes = format!("pdf bytes of {title}");
        std::fs::write(root.join(&rel).join(format!("{stem}.pdf")), &bytes).unwrap();
        conn.execute("INSERT INTO data(book, format, uncompressed_size, name) VALUES (?1, 'PDF', ?2, ?3)", params![id, bytes.len() as i64, stem]).unwrap();
    }
}

async fn finished(router: &axum::Router, job_id: u64) -> Value {
    for _ in 0..400 {
        let (_, job) = call(router, "GET", &format!("/library-jobs/{job_id}"), None).await;
        if job["state"] != "running" {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the job did not finish");
}

#[tokio::test]
async fn a_nested_calibre_library_is_listed_absorbed_and_listed_again() {
    let (dir, cache, router) = app();
    calibre_library(&dir.path().join("NAS/Calibre"));
    std::fs::create_dir_all(dir.path().join("Loose")).unwrap();

    let (status, listing) = call(&router, "GET", "/absorbable-libraries/-", None).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["conclusive"], true);
    assert_eq!(listing["libraries"], json!([{"path": "NAS/Calibre", "state": "available", "books": 2, "already_absorbed": 0, "problem": null}]));

    let (status, started) = call(&router, "POST", "/absorb-library/-", Some(json!({"path": "NAS/Calibre"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["kind"], "absorb");
    let job = finished(&router, started["job_id"].as_u64().unwrap()).await;
    assert_eq!(job["state"], "finished", "{job}");
    assert_eq!(job["report"]["absorbed"], 2, "{job}");
    assert_eq!(job["report"]["renamed"], true);
    assert_eq!(job["progress"]["total"], 2);
    assert_eq!(job["progress"]["added"], 2);
    assert_eq!(job["progress"]["hashed"], 2);

    assert_eq!(cache.all_book_ids().unwrap().len(), 2);
    assert_eq!(cache.field_for(1, "path").unwrap().as_deref(), Some("NAS/Calibre/Ann Author/First Book (1)"));
    assert!(dir.path().join("NAS/Calibre/metadata.db.absorbed-by-calibre-oxide").is_file());
    assert!(!dir.path().join("NAS/Calibre/metadata.db").exists());

    let (_, listing) = call(&router, "GET", "/absorbable-libraries/-", None).await;
    assert_eq!(listing["libraries"][0]["state"], "absorbed");
    assert_eq!(listing["libraries"][0]["books"], 2);

    // Undo, as a job.
    let (status, started) = call(&router, "POST", "/unabsorb-library/-", Some(json!({"path": "NAS/Calibre"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = finished(&router, started["job_id"].as_u64().unwrap()).await;
    assert_eq!(job["report"]["removed"], 2, "{job}");
    assert!(cache.all_book_ids().unwrap().is_empty());
    assert!(dir.path().join("NAS/Calibre/metadata.db").is_file());
    assert!(dir.path().join("NAS/Calibre/Ann Author/First Book (1)").is_dir(), "files are never removed");
}

#[tokio::test]
async fn refusals_are_answered_before_a_job_starts() {
    let (dir, cache, router) = app();
    calibre_library(&dir.path().join("Calibre"));
    std::fs::create_dir_all(dir.path().join("Loose")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    calibre_library(&outside.path().join("Calibre"));
    // Calibre mid-write.
    calibre_library(&dir.path().join("Busy"));
    std::fs::write(dir.path().join("Busy/metadata.db-journal"), b"hot").unwrap();

    for (body, why) in [
        (json!({"path": ""}), "empty"),
        (json!({"path": outside.path().join("Calibre").to_string_lossy()}), "outside the library"),
        (json!({"path": "../Calibre"}), "dot dot"),
        (json!({"path": "Loose"}), "no database"),
        (json!({"path": "Missing"}), "missing"),
        (json!({"path": dir.path().to_string_lossy()}), "the library itself"),
        (json!({"path": "Busy"}), "in use"),
    ] {
        let (status, answer) = call(&router, "POST", "/absorb-library/-", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {answer}");
    }
    assert!(cache.all_book_ids().unwrap().is_empty());
}
