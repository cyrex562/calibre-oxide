use calibre_db::schema_upgrades::SchemaUpgrade;
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn test_schema_upgrade_stub() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("metadata.db");

    // Create an empty DB with a user_version
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 25", []).unwrap();
    }

    let mut conn = Connection::open(&db_path).unwrap();

    // Should not panic/fail
    let result = SchemaUpgrade::upgrade_to_latest(&mut conn, dir.path());
    assert!(result.is_ok());
}
