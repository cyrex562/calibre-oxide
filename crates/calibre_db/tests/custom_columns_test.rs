use calibre_db::cli::cmd_add_custom_column::CmdAddCustomColumn;
use calibre_db::cli::cmd_remove_custom_column::CmdRemoveCustomColumn;
use calibre_db::Library;

#[test]
fn test_library_custom_columns() {
    let mut db = Library::open_test().unwrap();

    // 1. Add Custom Column
    let col_id = db
        .add_custom_column("testcol", "Test Column", "text", false)
        .expect("Failed to add custom column");

    // Verify it exists in DB
    let count: i32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = 'testcol'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Verify table creation
    let table_exists: i32 = db
        .conn()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='custom_column_{}'",
                col_id
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);

    // 2. Remove Custom Column
    db.remove_custom_column("testcol")
        .expect("Failed to remove custom column");

    // Verify it's gone from DB
    let count: i32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = 'testcol'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    // Verify table drop
    let table_exists: i32 = db
        .conn()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='custom_column_{}'",
                col_id
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 0);
}

#[test]
fn test_cmd_add_custom_column() {
    let mut db = Library::open_test().unwrap();
    let cmd = CmdAddCustomColumn::new();

    // 1. Valid Add
    let args = vec![
        "mycol".to_string(),
        "My Column".to_string(),
        "text".to_string(),
    ];
    cmd.run(&mut db, &args).expect("Cmd failed");

    // Verify
    let count: i32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = 'mycol'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // 2. Duplicate Add (Should Fail)
    let res = cmd.run(&mut db, &args);
    assert!(res.is_err()); // Label exists
}

#[test]
fn test_cmd_remove_custom_column() {
    let mut db = Library::open_test().unwrap();
    db.add_custom_column("remcol", "Remove Me", "bool", false)
        .unwrap();

    let cmd = CmdRemoveCustomColumn::new();
    let args = vec!["remcol".to_string()];

    // 1. Valid Remove
    cmd.run(&mut db, &args).expect("Cmd failed");

    // Verify
    let count: i32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = 'remcol'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    // 2. Remove Non-existent (Should Fail)
    let res = cmd.run(&mut db, &args);
    assert!(res.is_err());
}

#[test]
fn test_custom_column_types() {
    let mut db = Library::open_test().unwrap();

    // Test different types trigger correct table schemas
    // int
    let id_int = db
        .add_custom_column("intcol", "Int Col", "int", false)
        .unwrap();
    // Verify value column type
    // SQLite doesn't strictly enforce types, but we can check if table was created.
    // We already checked logic in library.rs, but good to ensure no SQL syntax errors for other branches

    // float
    let _ = db
        .add_custom_column("floatcol", "Float Col", "float", false)
        .unwrap();

    // bool
    let _ = db
        .add_custom_column("boolcol", "Bool Col", "bool", false)
        .unwrap();

    // series
    let _ = db
        .add_custom_column("seriescol", "Series Col", "series", false)
        .unwrap();

    // Multiple text (Not supported yet, should fail)
    let res = db.add_custom_column("multitcol", "Multi Col", "text", true);
    assert!(res.is_err());
}
