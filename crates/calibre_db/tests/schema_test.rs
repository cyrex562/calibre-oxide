use calibre_db::backend::Backend;
use calibre_db::schema_upgrades::SchemaUpgrade;
use tempfile::tempdir;

/// `SchemaUpgrade` is real now (see #201/schema_upgrades.rs's own unit
/// tests for the full version-1-to-26 migration chain, exercised
/// against a hand-derived starting schema). What this integration
/// test can usefully check, working only with what's public outside
/// the crate: a database claiming to be at the *latest* version (a
/// real library `Backend::new` just created, not a fabricated
/// "version 25" claim over an empty database with no tables -- which
/// used to pass here only because the old code was a no-op stub that
/// never looked at the tables at all) is a genuine no-op when
/// `upgrade_to_latest` runs again.
#[test]
fn test_schema_upgrade_is_a_noop_on_an_already_current_library() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();

    let mut conn = backend.conn.lock().unwrap();
    let result = SchemaUpgrade::upgrade_to_latest(&mut conn, dir.path());
    assert!(result.is_ok(), "{result:?}");

    let uv: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(uv, 26);
}
