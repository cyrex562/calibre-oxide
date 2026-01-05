use calibre_db::cli::cmd_list::CmdList;
use calibre_db::cli::cmd_list_categories::CmdListCategories;
use calibre_db::cli::cmd_search::CmdSearch;

#[test]
fn test_cli_search_stubs() {
    let cats = CmdListCategories::new();
    assert!(cats.run().is_err());
}
