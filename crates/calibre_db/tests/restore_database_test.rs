use calibre_db::library::Library;
use calibre_db::restore;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_restore_database_flow() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();

    // 1. Initialize Library and Add a Book
    {
        let mut library = Library::create(library_path.clone()).expect("Failed to create library");
        let book_dir = library_path.join("Test Author").join("Test Book");
        fs::create_dir_all(&book_dir).unwrap();

        let opf_path = book_dir.join("metadata.opf");
        let mut meta = MetaInformation::default();
        meta.title = "Test Book".to_string();
        meta.authors = vec!["Test Author".to_string()];
        meta.uuid = Some("test-uuid-123".to_string());

        fs::write(&opf_path, meta.to_xml()).unwrap();

        // 2. Corrupt/Delete DB
        // We close library implicitly by drop, but file is still open?
        // Dropping library struct closes connection.
    }

    println!("Library path: {:?}", library_path);
    let db_path = library_path.join("metadata.db");
    if db_path.exists() {
        println!("Deleting existing DB: {:?}", db_path);
        fs::remove_file(&db_path).unwrap();
    } else {
        println!("DB does not exist as expected");
    }

    let opf_check = library_path
        .join("Test Author")
        .join("Test Book")
        .join("metadata.opf");
    println!(
        "Checking OPF exists: {:?} -> {}",
        opf_check,
        opf_check.exists()
    );

    println!("Starting restore...");
    // 3. Run Restore
    restore::restore_database(&library_path, |msg| {
        println!("Callback: {}", msg);
    })
    .expect("Restore failed");
    println!("Restore finished");

    // 4. Verify DB restored
    let library = Library::open(library_path.clone()).expect("Failed to open restored library");
    let books = library.list_books().expect("Failed to list books");

    assert_eq!(books.len(), 1);
    let book = &books[0];
    assert_eq!(book.title, "Test Book");
    assert_eq!(book.author_sort.as_deref(), Some("Test Author"));
    assert_eq!(book.uuid.as_deref(), Some("test-uuid-123"));
}
