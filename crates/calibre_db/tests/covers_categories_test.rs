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
    // fresh dir -- just seed reference data into it.
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();

        // Insert dummy reference data
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
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    // 1. Test Categories
    let cats = categories::get_categories(&cache).expect("get_categories failed");
    let authors = cats.get("authors").unwrap();
    let series = cats.get("series").unwrap();

    assert!(authors.contains(&"Author A".to_string()));
    assert!(series.contains(&"Series X".to_string()));

    // 2. Test Covers
    // First, insert a book via backend (bypassing full adding logic to set 'path' manually)
    {
        let guard = cache.lock().unwrap();
        let conn = guard.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (title, path) VALUES ('My Book', 'Author A/My Book')",
            [],
        )
        .unwrap();
    }
    // We assume ID 1
    let book_id = 1;

    // Set Cover
    let fake_image_data = b"fake image data";
    covers::set_cover(&cache, book_id, fake_image_data).expect("set_cover failed");

    // Verify File Exists
    let cover_path = covers::cover_path(&cache, book_id).expect("cover_path failed");
    assert!(cover_path.exists());
    let read_data = fs::read(cover_path).unwrap();
    assert_eq!(read_data, fake_image_data);
}
