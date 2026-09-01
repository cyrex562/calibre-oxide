use calibre_db::cli::cmd_list::CmdList;
use calibre_db::cli::cmd_list_categories::CmdListCategories;
use calibre_db::cli::cmd_search::CmdSearch;
use calibre_db::Library;

#[test]
fn test_cli_search_stubs() {
    let db = Library::open_test().unwrap();

    let cats = CmdListCategories::new();
    assert!(cats.run(&db, &Vec::new()).is_ok());
    assert!(cats.run(&db, &vec!["--csv".to_string()]).is_ok());

    let list = CmdList::new();
    assert!(list.run(&db, &Vec::new()).is_ok());

    let search = CmdSearch::new();
    // An empty library with no matches is still a successful search
    // (zero results), not an error.
    assert!(search.run(&db, &["title:nonexistent".to_string()]).is_ok());
}
