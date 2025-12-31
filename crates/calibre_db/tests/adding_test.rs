use calibre_db::adding;
use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_add_book() {
    let dir = tempdir().unwrap();

    // Setup DB
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        // Schema simplified for test, but ensure fields match insert_book
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT, series_index REAL DEFAULT 1.0)", 
            []
        ).unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    let authors = vec!["Author One".to_string(), "Author Two".to_string()];
    let book_id = adding::add_book(&cache, "New Book Title", &authors).expect("add_book failed");

    assert!(book_id > 0);

    // Verify Data
    let guard = cache.lock().unwrap();
    let title = guard.field_for(book_id, "title").unwrap().unwrap();
    let author_sort = guard.field_for(book_id, "author_sort").unwrap().unwrap();
    let uuid = guard.field_for(book_id, "uuid").unwrap();

    assert_eq!(title, "New Book Title");
    assert_eq!(author_sort, "Author One & Author Two");
    assert!(uuid.is_some());
    println!("Generated UUID: {}", uuid.unwrap());
}
