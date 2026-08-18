use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use tempfile::tempdir;

#[test]
fn test_connection_and_init() {
    let dir = tempdir().unwrap();

    // `Backend::new` creates the real calibre schema for a fresh dir
    // (including the dedicated `library_id` table -- `library_id` is
    // not stored as a regular preference, unlike what this test used
    // to assume).
    let backend = Backend::new(dir.path()).unwrap();
    assert!(backend.db_path.exists());
    let id = backend.library_id().unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok(), "{id}");

    // Test Cache: `Cache::library_id` reads the same real table and
    // must agree with `Backend::library_id` (get-or-create is stable
    // across both call sites since they share the same DB file).
    let cache = Cache::new(dir.path()).unwrap();
    assert_eq!(cache.library_id(), id);
}
