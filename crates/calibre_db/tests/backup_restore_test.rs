use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::{backup, restore};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_backup_restore_roundtrip() {
    let dir = tempdir().unwrap();

    // Setup DB: `Backend::new` creates the real calibre schema for a
    // fresh dir -- just seed one row into it.
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
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

    // backup_metadata also records a real BLAKE3 checksum for the
    // sidecar OPF (issue #93 §8's "sidecar files: same rule").
    {
        let guard = cache.lock().unwrap();
        assert_eq!(
            guard
                .checksums()
                .verify_file(book_id, "opf", "", &opf_path)
                .unwrap(),
            calibre_db::checksums::VerifyOutcome::Match
        );
    }

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
