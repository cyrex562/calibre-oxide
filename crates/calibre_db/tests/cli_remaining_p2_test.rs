use calibre_db::cli::cmd_saved_searches::CmdSavedSearches;
use calibre_db::cli::cmd_set_custom::CmdSetCustom;
use calibre_db::cli::cmd_set_metadata::CmdSetMetadata;
use calibre_db::cli::cmd_switch::CmdSwitch;
use calibre_db::Library;

#[test]
fn test_cli_remaining_p2_stubs() {
    let mut db = Library::open_test().unwrap();

    let saved = CmdSavedSearches::new();
    // Too few args for "add" is a real usage error.
    assert!(saved.run(&mut db, &["add".to_string()]).is_err());
    // A well-formed add succeeds.
    assert!(saved
        .run(&mut db, &["add".to_string(), "my_search".to_string(), "title:foo".to_string()])
        .is_ok());

    let set_cust = CmdSetCustom::new();
    // A book id that doesn't exist is a real error.
    assert!(set_cust
        .run(&mut db, &["999".to_string(), "col".to_string(), "val".to_string()])
        .is_err());

    let set_meta = CmdSetMetadata::new();
    assert!(set_meta
        .run(&mut db, &["999".to_string(), "title".to_string(), "New Title".to_string()])
        .is_err());

    let switch = CmdSwitch::new();
    // No args is a real usage error.
    assert!(switch.run(&[]).is_err());
}
