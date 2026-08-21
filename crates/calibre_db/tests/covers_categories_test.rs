use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::{categories, covers};
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_covers_and_categories() {
    let dir = tempdir().unwrap();

    // Setup DB: `Backend::new` creates the real calibre schema for a
    // fresh dir -- seed one book (path set, for the cover test below)
    // and link it to an author/series (a real category only appears
    // for an item with at least one linked book, same as upstream's
    // tag browser -- an orphaned author/series with zero books isn't
    // shown).
    let book_id;
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO authors (name) VALUES ('Author A'), ('Author B')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO series (name) VALUES ('Series X'), ('Series Y')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books (title, path) VALUES ('My Book', 'Author A/My Book')",
            [],
        )
        .unwrap();
        book_id = conn.last_insert_rowid() as i32;
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, 1)",
            [book_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_series_link (book, series) VALUES (?1, 1)",
            [book_id],
        )
        .unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    // 1. Test Categories
    let cats = categories::get_categories(&cache, "name", None).expect("get_categories failed");
    let authors = cats.get("authors").unwrap();
    let series = cats.get("series").unwrap();

    assert!(authors.iter().any(|t| t.name == "Author A" && t.count == 1));
    assert!(series.iter().any(|t| t.name == "Series X" && t.count == 1));

    // 2. Test Covers

    // Set Cover
    let fake_image_data = b"fake image data";
    covers::set_cover(&cache, book_id, fake_image_data).expect("set_cover failed");

    // Verify File Exists
    let cover_path = covers::cover_path(&cache, book_id).expect("cover_path failed");
    assert!(cover_path.exists());
    let read_data = fs::read(&cover_path).unwrap();
    assert_eq!(read_data, fake_image_data);

    // set_cover also records a real BLAKE3 checksum (issue #93 §8) --
    // verifying against the file as it stands now must match, and a
    // direct on-disk tamper must be caught.
    {
        let guard = cache.lock().unwrap();
        assert_eq!(
            guard
                .checksums()
                .verify_file(book_id, "cover", "", &cover_path)
                .unwrap(),
            calibre_db::checksums::VerifyOutcome::Match
        );
    }
    fs::write(&cover_path, b"tampered").unwrap();
    {
        let guard = cache.lock().unwrap();
        assert!(matches!(
            guard
                .checksums()
                .verify_file(book_id, "cover", "", &cover_path),
            Err(calibre_db::checksums::ChecksumError::Mismatch { .. })
        ));
    }
}
