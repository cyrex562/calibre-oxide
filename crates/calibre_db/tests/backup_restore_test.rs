use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::{backup, restore};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_backup_restore_roundtrip() {
    let dir = tempdir().unwrap();

    // Setup DB
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT, path TEXT, series_index REAL DEFAULT 1.0)", 
            []
        ).unwrap();
        conn.execute("INSERT INTO books (title, author_sort, uuid, path) VALUES ('Original Title', 'Original Author', 'uuid-1', 'BookPath')", []).unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));
    let book_id = 1;

    // 1. Basic Backup
    backup::backup_metadata(&cache, book_id).expect("Backup failed");

    let opf_path = dir.path().join("BookPath").join("metadata.opf");
    assert!(opf_path.exists());
    let opf_content = std::fs::read_to_string(&opf_path).unwrap();
    assert!(opf_content.contains("<dc:title>Original Title</dc:title>"));

    // 2. Modify DB (Simulate data loss/change)
    {
        let guard = cache.lock().unwrap();
        guard
            .backend
            .update(book_id, "title", "Corrupted Title")
            .unwrap();
        guard
            .backend
            .update(book_id, "author_sort", "Wrong Author")
            .unwrap();
    }

    // Verify modification happened
    {
        let guard = cache.lock().unwrap();
        let t = guard.field_for(book_id, "title").unwrap().unwrap();
        assert_eq!(t, "Corrupted Title");
    }

    // 3. Restore
    restore::restore_from_opf(&cache, book_id).expect("Restore failed");

    // 4. Verify Restoration
    {
        let guard = cache.lock().unwrap();
        let t = guard.field_for(book_id, "title").unwrap().unwrap();
        let a = guard.field_for(book_id, "author_sort").unwrap().unwrap();

        assert_eq!(t, "Original Title");
        assert_eq!(a, "Original Author");
    }
}
