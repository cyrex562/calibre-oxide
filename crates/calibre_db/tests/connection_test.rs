use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use rusqlite::Connection;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_connection_and_init() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("metadata.db");

    // Initialize a dummy sqlite db
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE preferences (id INTEGER PRIMARY KEY, key TEXT, val TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO preferences (key, val) VALUES (?1, ?2)",
            ("library_id", "12345"),
        )
        .unwrap();
    }

    // Test Backend
    let mut backend = Backend::new(dir.path()).unwrap();
    assert!(backend.db_path.exists());
    backend.load_prefs().unwrap();
    assert_eq!(backend.prefs.get("library_id").unwrap(), "12345");

    // Test Cache
    let cache = Cache::new(dir.path()).unwrap();
    assert_eq!(cache.library_id(), "12345");
}
