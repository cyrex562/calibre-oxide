use calibre_db::cli::cmd_saved_searches::CmdSavedSearches;
use calibre_db::cli::cmd_set_custom::CmdSetCustom;
use calibre_db::cli::cmd_set_metadata::CmdSetMetadata;
use calibre_db::Library;

#[test]
fn test_saved_searches() {
    let mut library = Library::open_test().unwrap();
    let cmd = CmdSavedSearches::new();

    // Add
    cmd.run(
        &mut library,
        &[
            "add".to_string(),
            "mysearch".to_string(),
            "title:test".to_string(),
        ],
    )
    .unwrap();

    // Check
    let json = library.get_preference("saved_searches").unwrap().unwrap();
    assert!(json.contains("mysearch"));
    assert!(json.contains("title:test"));

    // Remove
    cmd.run(
        &mut library,
        &["remove".to_string(), "mysearch".to_string()],
    )
    .unwrap();
    let json = library.get_preference("saved_searches").unwrap().unwrap();
    assert!(!json.contains("mysearch"));
}

#[test]
fn test_set_custom() {
    let mut library = Library::open_test().unwrap();
    library.insert_test_book("Test Book").unwrap();
    let book_ids = library.search("Test").unwrap();
    let book_id = book_ids[0];

    library
        .add_custom_column("mycol", "My Column", "text", false)
        .unwrap();

    let cmd = CmdSetCustom::new();
    cmd.run(
        &mut library,
        &[
            book_id.to_string(),
            "mycol".to_string(),
            "My Value".to_string(),
        ],
    )
    .unwrap();

    // Verify
    let val = library.get_custom_column_value(book_id, "mycol").unwrap();
    assert_eq!(val, Some("My Value".to_string()));
}

#[test]
fn test_set_metadata() {
    let mut library = Library::open_test().unwrap();
    library.insert_test_book("Test Book").unwrap();
    let book_ids = library.search("Test").unwrap();
    let book_id = book_ids[0];

    let cmd = CmdSetMetadata::new();

    // Change title
    cmd.run(
        &mut library,
        &[
            book_id.to_string(),
            "title".to_string(),
            "New Title".to_string(),
        ],
    )
    .unwrap();
    let book = library.get_book(book_id).unwrap().unwrap();
    assert_eq!(book.title, "New Title");

    // Change author_sort
    cmd.run(
        &mut library,
        &[
            book_id.to_string(),
            "author_sort".to_string(),
            "Sorted, Author".to_string(),
        ],
    )
    .unwrap();
    let book = library.get_book(book_id).unwrap().unwrap();
    assert_eq!(book.author_sort.unwrap(), "Sorted, Author");
}
