use calibre_db::legacy::LegacyDB;
use tempfile::tempdir;

#[test]
fn test_legacy_check() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("metadata.db");

    let legacy = LegacyDB::new();

    // Non-existent DB is compatible (fresh start)
    assert!(legacy.check_compatibility(&db_path).unwrap());

    // Migration always fails (stub)
    assert!(legacy.migrate(&db_path).is_err());
}
