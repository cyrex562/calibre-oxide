use calibre_db::backend::Backend;
use calibre_db::fts::connection::FtsConnection;
use std::collections::HashSet;
use tempfile::tempdir;

fn add_text(fts: &FtsConnection, book_id: i32, fmt: &str, text: &str) {
    fts.add_text(book_id, fmt, 0.0, Some(text), "", 0, "", None)
        .expect("add_text failed");
}

#[test]
fn fts_db_is_attached_in_wal_mode_with_synchronous_full() {
    // docs/FAULT_TOLERANCE.md §3 (issue #260): real, not just
    // documented -- query the pragmas back from the live connection.
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().expect("Failed to initialize FTS");

    let conn = backend.conn.lock().unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA fts_db.journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_lowercase(), "wal");
    let synchronous: i64 = conn
        .query_row("PRAGMA fts_db.synchronous", [], |row| row.get(0))
        .unwrap();
    assert_eq!(synchronous, 2);
}

#[test]
fn test_fts_basic_flow() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().expect("Failed to initialize FTS");

    add_text(&fts, 1, "EPUB", "This is a book about Rust programming.");
    add_text(&fts, 2, "MOBI", "Python is also a great language.");
    add_text(
        &fts,
        3,
        "TXT",
        "Rust allows for memory safety without garbage collection.",
    );

    let results = fts
        .search("Rust", false, None, None, None, true)
        .expect("Search failed");
    assert_eq!(results.len(), 2);

    let book_ids: Vec<i32> = results.iter().map(|r| r.book_id).collect();
    assert!(book_ids.contains(&1));
    assert!(book_ids.contains(&3));
    assert!(!book_ids.contains(&2));
}

#[test]
fn dirty_tracking_reflects_insertions_into_main_data_via_the_temp_triggers() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES ('T')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (1, 'EPUB', 10, 'x')",
            [],
        )
        .unwrap();
    }

    assert_eq!(fts.number_dirtied().unwrap(), 1);
    let dirty = fts.all_currently_dirty().unwrap();
    assert_eq!(dirty, vec![(1, "EPUB".to_string())]);

    // Indexing the text (via `add_text`) clears the dirty row, same
    // as `books_fts_insert_trg`.
    add_text(&fts, 1, "EPUB", "some searchable text");
    assert_eq!(fts.number_dirtied().unwrap(), 0);
    assert_eq!(fts.number_indexed().unwrap(), 1);
}

#[test]
fn deleting_a_book_cleans_up_its_indexed_text_and_dirty_rows() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (id, title) VALUES (1, 'T')", [])
            .unwrap();
    }
    add_text(&fts, 1, "EPUB", "some text");
    assert_eq!(fts.number_indexed().unwrap(), 1);

    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("DELETE FROM books WHERE id = 1", []).unwrap();
    }
    assert_eq!(fts.number_indexed().unwrap(), 0);
}

#[test]
fn dirty_book_and_remove_dirty_and_clear_all_dirty_manage_the_queue() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    fts.dirty_book(1, &["epub", "mobi"]).unwrap();
    assert_eq!(fts.number_dirtied().unwrap(), 2);

    fts.remove_dirty(1, "epub").unwrap();
    assert_eq!(fts.number_dirtied().unwrap(), 1);

    fts.dirty_book(2, &["txt"]).unwrap();
    fts.clear_all_dirty().unwrap();
    assert_eq!(fts.number_dirtied().unwrap(), 0);
}

#[test]
fn unindex_removes_a_single_format_or_every_format() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    add_text(&fts, 1, "EPUB", "epub text");
    add_text(&fts, 1, "MOBI", "mobi text");
    assert_eq!(fts.number_indexed().unwrap(), 2);

    fts.unindex(1, Some("epub")).unwrap();
    assert_eq!(fts.number_indexed().unwrap(), 1);

    fts.unindex(1, None).unwrap();
    assert_eq!(fts.number_indexed().unwrap(), 0);
}

#[test]
fn add_text_with_an_error_message_records_the_failure_not_searchable_text() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    fts.add_text(
        1,
        "epub",
        0.0,
        None,
        "",
        100,
        "hash",
        Some("extraction failed"),
    )
    .unwrap();

    let results = fts.search("failed", false, None, None, None, true).unwrap();
    // The error message isn't indexed as searchable text.
    assert!(results.is_empty());
    assert_eq!(fts.number_indexed().unwrap(), 1);
}

#[test]
fn search_restricted_to_a_single_book_id_only_matches_that_book() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    add_text(&fts, 1, "EPUB", "shared word apple");
    add_text(&fts, 2, "EPUB", "shared word banana");

    let restrict: HashSet<i32> = [1].into_iter().collect();
    let results = fts
        .search("shared", false, None, None, Some(&restrict), true)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].book_id, 1);
}

#[test]
fn search_restricted_to_multiple_book_ids_uses_the_temp_table_path() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    add_text(&fts, 1, "EPUB", "shared word apple");
    add_text(&fts, 2, "EPUB", "shared word banana");
    add_text(&fts, 3, "EPUB", "shared word cherry");

    let restrict: HashSet<i32> = [1, 2].into_iter().collect();
    let mut results = fts
        .search("shared", false, None, None, Some(&restrict), true)
        .unwrap();
    results.sort_by_key(|r| r.book_id);
    assert_eq!(
        results.iter().map(|r| r.book_id).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn search_with_an_empty_restrict_set_returns_no_results() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();
    add_text(&fts, 1, "EPUB", "some text");

    let empty: HashSet<i32> = HashSet::new();
    let results = fts
        .search("text", false, None, None, Some(&empty), true)
        .unwrap();
    assert!(results.is_empty());
}

#[test]
fn search_with_highlight_markers_wraps_the_match() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();
    add_text(&fts, 1, "EPUB", "a sentence about rust programming");

    let results = fts
        .search("rust", false, Some(("[[", "]]")), None, None, true)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].text.as_deref().unwrap().contains("[[rust]]"));
}

#[test]
fn search_without_return_text_omits_the_text_field() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();
    add_text(&fts, 1, "EPUB", "some searchable text");

    let results = fts
        .search("searchable", false, None, None, None, false)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].text, None);
}

#[test]
fn a_malformed_fts_query_returns_a_real_fts_query_error() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();
    add_text(&fts, 1, "EPUB", "some text");

    // An unbalanced quote is invalid FTS5 MATCH syntax.
    let err = fts
        .search("\"unterminated", false, None, None, None, true)
        .unwrap_err();
    assert_eq!(err.query, "\"unterminated");
}

#[test]
fn already_indexed_detects_a_matching_format_size_and_hash() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let fts = FtsConnection::new(backend.conn.clone(), &backend.db_path);
    fts.initialize().unwrap();

    fts.add_text(1, "epub", 0.0, Some("text"), "th", 50, "fh", None)
        .unwrap();

    assert!(fts.already_indexed(1, "epub", 50, "fh").unwrap());
    assert!(!fts.already_indexed(1, "epub", 51, "fh").unwrap());
    assert!(!fts.already_indexed(1, "epub", 50, "other").unwrap());
}
