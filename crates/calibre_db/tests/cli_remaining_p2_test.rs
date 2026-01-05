use calibre_db::cli::cmd_saved_searches::CmdSavedSearches;
use calibre_db::cli::cmd_set_custom::CmdSetCustom;
use calibre_db::cli::cmd_set_metadata::CmdSetMetadata;
use calibre_db::cli::cmd_show_metadata::CmdShowMetadata;
use calibre_db::cli::cmd_switch::CmdSwitch;
use std::path::Path;

#[test]
fn test_cli_remaining_p2_stubs() {
    let saved = CmdSavedSearches::new();
    assert!(saved.run("my_search", "add").is_err());

    let set_cust = CmdSetCustom::new();
    assert!(set_cust.run(1, "col", "val").is_err());

    let set_meta = CmdSetMetadata::new();
    assert!(set_meta.run(1, "title", "New Title").is_err());

    let switch = CmdSwitch::new();
    assert!(switch.run(Path::new("new_lib")).is_err());
}
