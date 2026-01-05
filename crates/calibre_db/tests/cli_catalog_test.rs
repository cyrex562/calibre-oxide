use calibre_db::cli::cmd_catalog;
use calibre_db::Library;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_catalog_csv() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("metadata.db");
    std::fs::File::create(&db_path).unwrap();

    // Setup DB
    {
        let mut lib = Library::open(temp_dir.path().to_path_buf()).unwrap();
        lib.conn().execute_batch(
            "CREATE TABLE books ( id INTEGER PRIMARY KEY, title TEXT, sort TEXT, timestamp TEXT, pubdate TEXT, series_index REAL, author_sort TEXT, isbn TEXT, lccn TEXT, path TEXT, has_cover INTEGER, uuid TEXT );
             CREATE TABLE authors ( id INTEGER PRIMARY KEY, name TEXT UNIQUE, sort TEXT, link TEXT );
             CREATE TABLE books_authors_link ( id INTEGER PRIMARY KEY, book INTEGER, author INTEGER );"
        ).unwrap();

        lib.conn().execute(
            "INSERT INTO books (title, sort, author_sort, path, has_cover, timestamp, pubdate, uuid, series_index)
             VALUES ('Test Book', 'Test Book', 'Author A', 'Path/To/Book', 0, '2023-01-01', '2023-01-01', 'uuid1', 1.0)",
            (),
        ).unwrap();
    }

    let lib = Library::open(temp_dir.path().to_path_buf()).unwrap();
    let output_path = temp_dir.path().join("catalog.csv");

    let args = cmd_catalog::RunArgs {
        output_file: output_path.clone(),
        ids: None,
        search: None,
        verbose: false,
    };

    let cmd = cmd_catalog::CmdCatalog::new();
    cmd.run(&lib, &args).unwrap();

    assert!(output_path.exists());
    let content = fs::read_to_string(&output_path).unwrap();
    assert!(content.contains("Test Book"));
    assert!(content.contains("Author A"));
    assert!(content.contains("Path/To/Book"));
}
