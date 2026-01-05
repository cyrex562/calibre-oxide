use calibre_db::cli::cmd_list_categories::CmdListCategories;
use calibre_db::library::Library;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;

#[test]
fn test_list_categories() {
    let temp_dir = std::env::temp_dir().join("calibre_oxide_test_categories");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).unwrap();
    }
    fs::create_dir_all(&temp_dir).unwrap();
    fs::File::create(temp_dir.join("metadata.db")).unwrap();

    let mut lib = Library::open(temp_dir.clone()).unwrap();
    lib.conn()
        .execute_batch(
            "CREATE TABLE books (
            id INTEGER PRIMARY KEY,
            title TEXT,
            sort TEXT,
            timestamp TEXT,
            pubdate TEXT,
            series_index REAL,
            author_sort TEXT,
            isbn TEXT,
            lccn TEXT,
            path TEXT,
            has_cover INTEGER,
            uuid TEXT
        );
        CREATE TABLE authors (
            id INTEGER PRIMARY KEY,
            name TEXT UNIQUE,
            sort TEXT,
            link TEXT
        );
        CREATE TABLE books_authors_link (
            id INTEGER PRIMARY KEY,
            book INTEGER,
            author INTEGER
        );",
        )
        .unwrap();

    let meta = MetaInformation {
        title: "Test Book".to_string(),
        authors: vec!["Author One".to_string()],
        ..Default::default()
    };
    let source_path = temp_dir.join("source.epub");
    fs::write(&source_path, "dummy").unwrap();
    lib.add_book(&source_path, &meta).unwrap();

    let meta2 = MetaInformation {
        title: "Test Book 2".to_string(),
        authors: vec!["Author Two".to_string()],
        ..Default::default()
    };
    lib.add_book(&source_path, &meta2).unwrap();

    // Test Library Logic
    let categories = lib.get_categories().unwrap();
    assert!(categories.contains_key("authors"));
    let authors = categories.get("authors").unwrap();

    // We expect 2 authors
    assert_eq!(authors.len(), 2);
    // Sort logic in backend might not be strict in my implementation (iter order), but names should match
    let names: Vec<String> = authors.iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"Author One".to_string()));
    assert!(names.contains(&"Author Two".to_string()));

    // Test Command Runs (at least doesn't panic)
    let cmd = CmdListCategories::new();
    cmd.run(&lib, &Vec::new()).unwrap();
    cmd.run(&lib, &vec!["--csv".to_string()]).unwrap();
}
