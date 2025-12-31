use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::copy_to_library::copy_one_book;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_copy_book_basic() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    // Setup Src DB
    let src_cache = {
        let backend = Backend::new(src_dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT, path TEXT, series_index REAL DEFAULT 1.0)", 
            []
        ).unwrap();
        conn.execute("INSERT INTO books (title, sort, author_sort, uuid, path) VALUES ('Source Book', 'Source Book', 'Author A', 'uuid-src', 'book_path')", []).unwrap();
        Arc::new(Mutex::new(Cache::new(src_dir.path()).unwrap()))
    };

    // Setup Dest DB
    let dest_cache = {
        let backend = Backend::new(dest_dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT, path TEXT, series_index REAL DEFAULT 1.0)", 
            []
        ).unwrap();
        // Dest is empty
        Arc::new(Mutex::new(Cache::new(dest_dir.path()).unwrap()))
    };

    // Perform Copy
    let new_id = copy_one_book(&src_cache, &dest_cache, 1, false)
        .expect("Copy failed")
        .unwrap();

    // Verify
    {
        let guard = dest_cache.lock().unwrap();
        let title = guard.field_for(new_id, "title").unwrap().unwrap();
        let author = guard.field_for(new_id, "author_sort").unwrap().unwrap();

        assert_eq!(title, "Source Book");
        assert_eq!(author, "Author A");
    }
}
