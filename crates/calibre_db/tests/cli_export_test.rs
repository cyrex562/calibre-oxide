use calibre_db::cli::cmd_export;
use calibre_db::Library;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_export() {
    let temp_dir = tempdir().unwrap();

    // Setup DB and Source File
    let db_path = temp_dir.path().join("metadata.db");
    std::fs::File::create(&db_path).unwrap();
    let mut lib = Library::open(temp_dir.path().to_path_buf()).unwrap();
    lib.conn().execute_batch(
        "CREATE TABLE books ( id INTEGER PRIMARY KEY, title TEXT, sort TEXT, timestamp TEXT, pubdate TEXT, series_index REAL, author_sort TEXT, isbn TEXT, lccn TEXT, path TEXT, has_cover INTEGER, uuid TEXT );
         CREATE TABLE authors ( id INTEGER PRIMARY KEY, name TEXT UNIQUE, sort TEXT, link TEXT );
         CREATE TABLE books_authors_link ( id INTEGER PRIMARY KEY, book INTEGER, author INTEGER );
         CREATE TABLE data ( id INTEGER PRIMARY KEY, book INTEGER, format TEXT, uncompressed_size INTEGER, name TEXT );"
    ).unwrap();

    let rel_path = "Author_A/Test_Book";
    let full_src_dir = temp_dir.path().join(rel_path);
    fs::create_dir_all(&full_src_dir).unwrap();
    let src_file = full_src_dir.join("Test Book.epub");
    fs::write(&src_file, "epub content").unwrap();

    lib.conn().execute(
        "INSERT INTO books (title, sort, author_sort, path, has_cover, timestamp, pubdate, uuid, series_index)
         VALUES ('Test Book', 'Test Book', 'Author A', ?1, 0, '2023-01-01', '2023-01-01', 'uuid1', 1.0)",
        (rel_path,),
    ).unwrap();
    let book_id = lib.conn().last_insert_rowid() as i32;

    lib.conn().execute(
        "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 100, 'Test Book')",
        (book_id,),
    ).unwrap();

    let export_dir = temp_dir.path().join("export");
    let args = cmd_export::RunArgs {
        ids: vec![book_id.to_string()],
        all: false,
        to_dir: export_dir.to_string_lossy().to_string(),
        single_dir: false,
        progress: false,
    };

    let cmd = cmd_export::CmdExport::new();
    cmd.run(&lib, &args).unwrap();

    // Verify Export
    // Path structure: export_dir/Author A/Author A - Test Book.epub
    // Note: sanitization might change spaces to something else depending on implementation
    // "Author A" -> "Author A" (maybe?)
    // "Test Book" -> "Test Book"

    // Assuming strict sanitization isn't butchering it too much for this test input.
    // Let's iterate `export_dir` to find the file.
    let found = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.path().extension().map_or(false, |x| x == "epub")
                && fs::read_to_string(e.path()).unwrap() == "epub content"
        });

    assert!(found, "Exported file not found with expected content");
}
