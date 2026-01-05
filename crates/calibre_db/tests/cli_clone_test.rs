use calibre_db::cli::cmd_clone::CmdClone;
use calibre_db::library::Library;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;

#[test]
fn test_clone_library() {
    let temp_dir_src = std::env::temp_dir().join("calibre_oxide_test_clone_src");
    let temp_dir_dest = std::env::temp_dir().join("calibre_oxide_test_clone_dest");

    if temp_dir_src.exists() {
        fs::remove_dir_all(&temp_dir_src).unwrap();
    }
    if temp_dir_dest.exists() {
        fs::remove_dir_all(&temp_dir_dest).unwrap();
    }

    fs::create_dir_all(&temp_dir_src).unwrap();
    fs::File::create(temp_dir_src.join("metadata.db")).unwrap();

    let mut lib = Library::open(temp_dir_src.clone()).unwrap();
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

    // Add a book
    let meta = MetaInformation {
        title: "Clone Me".to_string(),
        authors: vec!["Me".to_string()],
        ..Default::default()
    };
    let source_path = temp_dir_src.join("book.epub");
    fs::write(&source_path, "content").unwrap();
    lib.add_book(&source_path, &meta).unwrap();

    // Run Clone
    let cmd = CmdClone::new();
    let run_args = calibre_db::cli::cmd_clone::RunArgs {
        path: temp_dir_dest.clone(),
    };
    cmd.run(&lib, &run_args).unwrap();

    // Verify Dest
    assert!(temp_dir_dest.join("metadata.db").exists());
    let lib2 = Library::open(temp_dir_dest.clone()).unwrap();
    assert_eq!(lib2.book_count().unwrap(), 1);

    // Verify File Copy
    // We need to check if the book file exists in dest
    // Ideally we query the DB to find the path, but 'add_book' puts it in Author/Title/Title.epub usually
    // Let's verify recursively that at least one epub exists

    // Just blindly checking if ANY epub exists in subdirs
    fn find_epub(dir: &std::path::Path) -> bool {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if find_epub(&path) {
                        return true;
                    }
                } else if path.extension().map_or(false, |e| e == "epub") {
                    return true;
                }
            }
        }
        false
    }

    assert!(find_epub(&temp_dir_dest), "Cloned book file should exist");
}
