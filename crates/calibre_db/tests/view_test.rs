use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::view::View;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_view_search_and_sort() {
    let dir = tempdir().unwrap();

    // Setup DB
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT, sort TEXT, author_sort TEXT, uuid TEXT)", 
            []
        ).unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES 
            (1, 'The Rust Book', 'Rust Book, The', 'Klabnik, Steve', 'u1'),
            (2, 'The Python Book', 'Python Book, The', 'Rossum, Guido', 'u2'),
            (3, 'C++ Programming', 'C++ Programming', 'Stroustrup, Bjarne', 'u3')",
            [],
        )
        .unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));
    let mut view = View::new(cache);

    // Check initial count
    assert_eq!(view.count(), 3);

    // Test Search
    view.search("Rust");
    assert_eq!(view.count(), 1);
    assert_eq!(view.get_ids(), &[1]);

    // Reset (re-create view or implementing clear logic, for now re-create)
    // We didn't implement 'clear_search' in View yet, so simpler to make new view or assume 'search("")' might work if we implemented it that way.
    // Our search implementation uses LIKE, so "%"" is ALL.
    view.search("");
    assert_eq!(view.count(), 3);

    view.search("Book");
    assert_eq!(view.count(), 2); // Rust Book, Python Book

    // Test Sort (ID only for now)
    view.sort("id", false); // Descending
    let ids = view.get_ids();
    assert_eq!(ids, &[2, 1]); // Based on the "Book" search result, IDs were 1 and 2.
                              // Wait, IDs might be returned in DB order (1, 2).
                              // Sort desc -> 2, 1.
}
