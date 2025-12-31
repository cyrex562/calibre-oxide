use calibre_db::backend::Backend;
use calibre_db::fts::connection::FtsConnection;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_fts_basic_flow() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();

    // Setup Backend (creates metadata.db)
    let backend = Backend::new(&library_path).unwrap();

    // Create FTS Connection
    // Note: FtsConnection expects main_db_path (metadata.db)
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);

    // Initialize (ATTACH and CREATE tables)
    fts.initialize().expect("Failed to initialize FTS");

    // Add content
    fts.add_document(1, "EPUB", "This is a book about Rust programming.")
        .expect("Failed to add doc 1");
    fts.add_document(2, "MOBI", "Python is also a great language.")
        .expect("Failed to add doc 2");
    fts.add_document(
        3,
        "TXT",
        "Rust allows for memory safety without garbage collection.",
    )
    .expect("Failed to add doc 3");

    // Search for "Rust"
    let results = fts.search("Rust").expect("Search failed");
    assert_eq!(results.len(), 2);

    // Verify results contain book IDs 1 and 3
    let book_ids: Vec<i32> = results.iter().map(|r| r.1).collect();
    assert!(book_ids.contains(&1));
    assert!(book_ids.contains(&3));
    assert!(!book_ids.contains(&2));
}
