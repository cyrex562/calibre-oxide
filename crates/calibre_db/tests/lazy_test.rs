use calibre_db::backend::Backend;
use calibre_db::lazy::ProxyMetadata;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_proxy_metadata_basics() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();

    // Populate DB with a book
    {
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, isbn TEXT, path TEXT, series_index REAL, uuid TEXT)", 
            []
        ).unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES (1, 'The Rust Book', 'Rust Book, The', 'Klabnik, Steve', '123-uuid')",
            []
        ).unwrap();
    }

    let backend_ref = Arc::new(Mutex::new(backend));
    let mut proxy = ProxyMetadata::new(1, backend_ref);

    // Initial state, should fetch from DB
    let title = proxy.get_title();
    assert_eq!(title, "The Rust Book");

    // Second fetch should hit cache
    let title2 = proxy.get_title();
    assert_eq!(title2, "The Rust Book");

    // Check another field
    let uuid = proxy.get_field("uuid");
    assert_eq!(uuid, Some("123-uuid".to_string()));

    // Check Missing field
    let missing = proxy.get_field("path"); // NULL in DB
    assert_eq!(missing, None);
}

#[test]
fn test_proxy_metadata_manual_cache() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let backend_ref = Arc::new(Mutex::new(backend));

    let mut proxy = ProxyMetadata::new(2, backend_ref);

    // We can't set cache directly as it's private, but if we had a setter...
    // For now, let's just rely on the getter behavior.
    assert_eq!(proxy.get_field("random_field"), None);
}
