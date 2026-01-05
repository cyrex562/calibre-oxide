use calibre_db::cli::cmd_remove_format::CmdRemoveFormat;
use calibre_db::library::Library;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;

#[test]
fn test_remove_format() {
    let temp_dir = std::env::temp_dir().join("calibre_oxide_test_remove_fmt");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).unwrap();
    }
    fs::create_dir_all(&temp_dir).unwrap();
    // Create DB
    fs::File::create(temp_dir.join("metadata.db")).unwrap();

    let mut lib = Library::open(temp_dir.clone()).unwrap();
    // Init DB schema (simplest way is to use existing migration logic or manual as in library.rs tests)
    // For now manual manual schema setup as in library.rs tests seems safest without migration system setup
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
        authors: vec!["Author Name".to_string()],
        ..Default::default()
    };

    // Need source file
    let source_path = temp_dir.join("source.epub");
    fs::write(&source_path, "dummy content").unwrap();

    let book_id = lib.add_book(&source_path, &meta).unwrap();

    // Verify file exists
    let book = lib.get_book(book_id).unwrap().unwrap();
    let book_dir = temp_dir.join(book.path);
    // Sanitize filename logic from library might produce "Test Book.epub"
    // Let's find it
    let mut found = false;
    for entry in fs::read_dir(&book_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().unwrap().to_str().unwrap() == "epub" {
            found = true;
            break;
        }
    }
    assert!(found, "Book file should exist before removal");

    // Run Command - Test Library Logic Directly to avoid DB Locking issues in test env
    // The CLI command just wraps this call.
    println!("Calling remove_format...");
    let cmd = CmdRemoveFormat::new();
    let args = vec![book_id.to_string(), "epub".to_string()];
    cmd.run(&mut lib, &args)
        .expect("CmdRemoveFormat run failed");
    println!("remove_format returned Ok");

    // Verify file is gone
    let mut found_after = false;
    let entries = fs::read_dir(&book_dir).unwrap();
    for entry in entries {
        let path = entry.unwrap().path();
        println!("Found file after removal: {:?}", path);
        if let Some(ext) = path.extension() {
            if ext.to_str().unwrap() == "epub" {
                found_after = true;
                break;
            }
        }
    }
    assert!(!found_after, "Book file should be removed");
}

#[test]
fn test_remove_books() {
    use calibre_db::cli::cmd_remove::{CmdRemove, RunArgs};

    let temp_dir = std::env::temp_dir().join("calibre_oxide_test_remove_books");
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

    // Add books
    lib.conn()
        .execute("INSERT INTO books (id, title) VALUES (1, 'Book 1')", [])
        .unwrap();
    lib.conn()
        .execute("INSERT INTO books (id, title) VALUES (2, 'Book 2')", [])
        .unwrap();
    lib.conn()
        .execute("INSERT INTO books (id, title) VALUES (3, 'Book 3')", [])
        .unwrap();

    assert_eq!(lib.book_count().unwrap(), 3);

    // Remove Book 1 and 3
    let cmd = CmdRemove::new();
    let args = RunArgs {
        ids: vec!["1".to_string(), "3".to_string()],
        permanent: true,
    };
    cmd.run(&mut lib, &args).unwrap();

    assert_eq!(lib.book_count().unwrap(), 1);

    // Setup for range removal
    lib.conn()
        .execute("INSERT INTO books (id, title) VALUES (4, 'Book 4')", [])
        .unwrap();
    lib.conn()
        .execute("INSERT INTO books (id, title) VALUES (5, 'Book 5')", [])
        .unwrap();

    // Remove range 4-5
    let args_range = RunArgs {
        ids: vec!["4-5".to_string()],
        permanent: true,
    };
    cmd.run(&mut lib, &args_range).unwrap();

    // Should be back to 1 (Book 2 remained)
    assert_eq!(lib.book_count().unwrap(), 1);

    // Verify book 2 exists
    let exists: i32 = lib
        .conn()
        .query_row("SELECT count(*) FROM books WHERE id=2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(exists, 1);
}
