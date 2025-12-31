use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::write;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_write_title_author() {
    let dir = tempdir().unwrap();

    // Setup DB
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT, series_index REAL DEFAULT 1.0)", 
            []
        ).unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES 
            (1, 'Old Title', 'Old Title', 'Old Author', 'u1')",
            [],
        )
        .unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    // Verify Initial State
    {
        let guard = cache.lock().unwrap();
        let title = guard.field_for(1, "title").unwrap().unwrap();
        assert_eq!(title, "Old Title");
    }

    // Test set_title
    write::set_title(&cache, 1, "New Title").expect("set_title failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        let title = guard.field_for(1, "title").unwrap().unwrap();
        assert_eq!(title, "New Title");
    }

    // Test set_author_sort
    write::set_author_sort(&cache, 1, "New Author").expect("set_author_sort failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        let author = guard.field_for(1, "author_sort").unwrap().unwrap();
        assert_eq!(author, "New Author");
    }

    // Test series_index (generic update_field)
    write::update_field(&cache, 1, "series_index", "2.5")
        .expect("update_field series_index failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        // field_for for series_index returns String (see backend.rs implementation)
        let idx = guard.field_for(1, "series_index").unwrap().unwrap();
        assert_eq!(idx, "2.5");
    }
}
