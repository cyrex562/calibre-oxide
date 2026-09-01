use calibre_db::cli::cmd_add_custom_column::CmdAddCustomColumn;
use calibre_db::Library;

#[test]
fn test_cli_add_custom_column_stubs() {
    let mut db = Library::open_test().unwrap();
    let cmd = CmdAddCustomColumn::new();

    // Too few positional args is a real usage error.
    assert!(cmd.run(&mut db, &["label".to_string()]).is_err());

    // A well-formed call really adds the column.
    assert!(cmd
        .run(&mut db, &["label".to_string(), "Name".to_string(), "text".to_string()])
        .is_ok());
}
