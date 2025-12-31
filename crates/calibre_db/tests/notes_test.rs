use calibre_db::backend::Backend;
use calibre_db::notes::connection::NotesConnection;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_notes_crud() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();

    // Setup Backend
    let backend = Backend::new(&library_path).unwrap();

    // Setup Notes Connection
    let notes_conn = NotesConnection::new(backend.conn.clone(), &library_path);
    notes_conn.initialize().expect("Failed to init notes");

    // Set Note
    let book_id = 1;
    let field = "title"; // Notes can be attached to fields or the book itself (field="")?
                         // Python: colname can be 'title' or whatever.
    let doc = "<div>My Note</div>";
    let search_text = "My Note";

    notes_conn
        .set_note(field, book_id, doc, search_text)
        .expect("Failed to set note");

    // Get Note
    let retrieved = notes_conn
        .get_note(field, book_id)
        .expect("Failed to get note");
    assert_eq!(retrieved, Some(doc.to_string()));

    // Update Note
    let new_doc = "<div>Updated Note</div>";
    notes_conn
        .set_note(field, book_id, new_doc, "Updated Note")
        .expect("Failed to update note");

    let retrieved_updated = notes_conn
        .get_note(field, book_id)
        .expect("Failed to get updated note");
    assert_eq!(retrieved_updated, Some(new_doc.to_string()));
}
