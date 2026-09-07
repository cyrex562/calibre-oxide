use calibre_db::backend::Backend;
use calibre_db::notes::connection::NotesConnection;
use std::collections::HashSet;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> NotesConnection {
    let backend = Backend::new(dir).unwrap();
    let notes = NotesConnection::new(backend, dir);
    notes.initialize().expect("Failed to init notes");
    notes
}

#[test]
fn notes_db_is_attached_in_wal_mode_with_synchronous_full() {
    // docs/FAULT_TOLERANCE.md §3 (issue #260): real, not just
    // documented -- query the pragmas back from the live connection.
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let notes = NotesConnection::new(backend.clone(), dir.path());
    notes.initialize().unwrap();

    let conn = backend.conn.lock().unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA notes_db.journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_lowercase(), "wal");
    let synchronous: i64 = conn
        .query_row("PRAGMA notes_db.synchronous", [], |row| row.get(0))
        .unwrap();
    assert_eq!(synchronous, 2);
}

#[test]
fn test_notes_crud() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    let book_id = 1;
    let field = "title";
    let doc = "<div>My Note</div>";

    notes
        .set_note(field, book_id, "Book Title", doc, &HashSet::new())
        .expect("Failed to set note");

    let retrieved = notes.get_note(field, book_id).expect("Failed to get note");
    assert_eq!(retrieved, Some(doc.to_string()));

    let new_doc = "<div>Updated Note</div>";
    notes
        .set_note(field, book_id, "Book Title", new_doc, &HashSet::new())
        .expect("Failed to update note");

    let retrieved_updated = notes
        .get_note(field, book_id)
        .expect("Failed to get updated note");
    assert_eq!(retrieved_updated, Some(new_doc.to_string()));
}

#[test]
fn setting_an_empty_note_deletes_it_and_returns_negative_one() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    notes
        .set_note("tags", 1, "fiction", "<p>a note</p>", &HashSet::new())
        .unwrap();
    assert!(notes.get_note("tags", 1).unwrap().is_some());

    let id = notes
        .set_note("tags", 1, "fiction", "", &HashSet::new())
        .unwrap();
    assert_eq!(id, -1);
    assert_eq!(notes.get_note("tags", 1).unwrap(), None);
}

#[test]
fn get_note_data_includes_ctime_mtime_and_resource_hashes() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    let hash = notes.add_resource(b"image bytes", "cover.jpg").unwrap();
    let mut resources = HashSet::new();
    resources.insert(hash.clone());

    notes
        .set_note(
            "authors",
            1,
            "Jane Doe",
            "<p>note with image</p>",
            &resources,
        )
        .unwrap();

    let data = notes.get_note_data("authors", 1).unwrap().unwrap();
    assert_eq!(data.doc, "<p>note with image</p>");
    assert!(data.searchable_text.contains("Jane Doe"));
    assert!(data.searchable_text.contains("note with image"));
    assert!(data.ctime > 0.0);
    assert_eq!(data.resource_hashes, resources);
}

#[test]
fn updating_a_note_to_drop_a_resource_removes_the_now_unreferenced_file() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    let hash = notes.add_resource(b"some bytes", "a.txt").unwrap();
    let mut resources = HashSet::new();
    resources.insert(hash.clone());
    notes
        .set_note("tags", 1, "fiction", "<p>v1</p>", &resources)
        .unwrap();

    let path = notes.path_for_resource(&hash);
    assert!(path.exists());

    // Re-set with no resources -- the file is no longer referenced by
    // anything and should be cleaned up.
    notes
        .set_note("tags", 1, "fiction", "<p>v2</p>", &HashSet::new())
        .unwrap();
    assert!(!path.exists());
}

#[test]
fn add_resource_disambiguates_a_name_collision_between_different_resources() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    let hash_a = notes.add_resource(b"content A", "image.jpg").unwrap();
    let hash_b = notes.add_resource(b"content B", "image.jpg").unwrap();
    assert_ne!(hash_a, hash_b);

    let a = notes.get_resource_data(&hash_a).unwrap().unwrap();
    let b = notes.get_resource_data(&hash_b).unwrap().unwrap();
    assert_ne!(a.name, b.name);
    assert_eq!(a.data, b"content A");
    assert_eq!(b.data, b"content B");
}

#[test]
fn path_for_resource_sanitizes_a_path_traversal_hash() {
    // `resource_hash` can come straight from an untrusted HTTP
    // request's path segments (see `calibre_srv::notes::get_note_resource`,
    // issue #60) -- a malicious `scheme`/`digest` must not be able to
    // escape `resources_dir`.
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    let evil = "../../../../../../../../tmp/notes-traversal-poc:../../../../../../../../tmp/notes-traversal-poc2";
    let path = notes.path_for_resource(evil);
    assert!(path.starts_with(dir.path()), "expected the sanitized path to stay under {:?}, got: {path:?}", dir.path());
}

#[test]
fn add_resource_is_idempotent_for_the_same_content_and_name() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    let hash1 = notes.add_resource(b"same bytes", "file.txt").unwrap();
    let hash2 = notes.add_resource(b"same bytes", "file.txt").unwrap();
    assert_eq!(hash1, hash2);

    let data = notes.get_resource_data(&hash1).unwrap().unwrap();
    assert_eq!(data.name, "file.txt");
}

#[test]
fn add_resource_journals_a_real_write_through_the_library_handle() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    notes.add_resource(b"resource bytes", "file.txt").unwrap();

    // add_resource now goes through the real LibraryHandle (issue
    // #93's crate-wide write-path retrofit), not a raw `fs::write` --
    // prove it by checking a real journal entry landed.
    let journal_dir = dir.path().join(".calibre-oxide").join("journal");
    let op_files: Vec<_> = std::fs::read_dir(&journal_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
        .collect();
    assert_eq!(op_files.len(), 1, "expected a real journaled write");
}

#[test]
fn remove_unreferenced_resources_journals_a_real_delete_through_the_library_handle() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    let hash = notes.add_resource(b"data", "f.txt").unwrap();
    let mut resources = HashSet::new();
    resources.insert(hash);
    notes
        .set_note("tags", 1, "v", "<p>note</p>", &resources)
        .unwrap();
    // Drop the note so its resource becomes unreferenced.
    notes.set_note("tags", 1, "v", "", &HashSet::new()).unwrap();

    notes.remove_unreferenced_resources().unwrap();

    let journal_dir = dir.path().join(".calibre-oxide").join("journal");
    let delete_entries = std::fs::read_dir(&journal_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
        .filter(|e| {
            std::fs::read_to_string(e.path())
                .map(|content| content.contains("DeleteFile"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(delete_entries, 1, "expected a real journaled delete");
}

#[test]
fn items_with_notes_for_field_and_all_items_with_notes_reflect_real_rows() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());

    notes
        .set_note("tags", 1, "fiction", "<p>a</p>", &HashSet::new())
        .unwrap();
    notes
        .set_note("tags", 2, "adventure", "<p>b</p>", &HashSet::new())
        .unwrap();
    notes
        .set_note("authors", 5, "Jane Doe", "<p>c</p>", &HashSet::new())
        .unwrap();

    let tags_items = notes.items_with_notes_for_field("tags").unwrap();
    assert_eq!(tags_items, HashSet::from([1, 2]));

    let all = notes.all_items_with_notes().unwrap();
    assert_eq!(all.get("tags"), Some(&HashSet::from([1, 2])));
    assert_eq!(all.get("authors"), Some(&HashSet::from([5])));
}

#[test]
fn rename_note_moves_the_note_to_the_new_item_id() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "old-name", "<p>a real note</p>", &HashSet::new())
        .unwrap();

    notes.rename_note("tags", 1, 2, "new-name").unwrap();

    assert_eq!(notes.get_note("tags", 1).unwrap(), None);
    let moved = notes.get_note("tags", 2).unwrap();
    assert_eq!(moved, Some("<p>a real note</p>".to_string()));
}

#[test]
fn rename_note_is_a_no_op_when_the_target_already_has_a_note() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "old", "<p>from</p>", &HashSet::new())
        .unwrap();
    notes
        .set_note("tags", 2, "new", "<p>already here</p>", &HashSet::new())
        .unwrap();

    notes.rename_note("tags", 1, 2, "new").unwrap();

    // Neither side changed.
    assert_eq!(
        notes.get_note("tags", 1).unwrap(),
        Some("<p>from</p>".to_string())
    );
    assert_eq!(
        notes.get_note("tags", 2).unwrap(),
        Some("<p>already here</p>".to_string())
    );
}

#[test]
fn delete_field_removes_every_note_for_that_field_and_orphaned_resources() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    let hash = notes.add_resource(b"data", "f.txt").unwrap();
    let mut resources = HashSet::new();
    resources.insert(hash.clone());
    notes
        .set_note("#mycol", 1, "v", "<p>note</p>", &resources)
        .unwrap();
    notes
        .set_note("tags", 1, "fiction", "<p>keep me</p>", &HashSet::new())
        .unwrap();

    notes.delete_field("#mycol").unwrap();

    assert_eq!(notes.get_note("#mycol", 1).unwrap(), None);
    assert_eq!(
        notes.get_note("tags", 1).unwrap(),
        Some("<p>keep me</p>".to_string())
    );
    assert!(!notes.path_for_resource(&hash).exists());
}

#[test]
fn all_notes_orders_by_mtime_descending_and_respects_field_restriction() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "a", "<p>first</p>", &HashSet::new())
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    notes
        .set_note("authors", 2, "b", "<p>second</p>", &HashSet::new())
        .unwrap();

    let all = notes.all_notes(&[], None, 64).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].field, "authors");
    assert_eq!(all[1].field, "tags");

    let restricted = notes.all_notes(&["tags"], None, 64).unwrap();
    assert_eq!(restricted.len(), 1);
    assert_eq!(restricted[0].field, "tags");
}

#[test]
fn search_finds_notes_by_searchable_text() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note(
            "tags",
            1,
            "fiction",
            "<p>a note about dragons</p>",
            &HashSet::new(),
        )
        .unwrap();
    notes
        .set_note(
            "tags",
            2,
            "nonfiction",
            "<p>a note about history</p>",
            &HashSet::new(),
        )
        .unwrap();

    let results = notes
        .search("dragons", false, None, None, &[], true, None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].item_id, 1);
}

#[test]
fn stemmed_and_non_stemmed_search_now_genuinely_differ() {
    // Issue #566: notes_fts/notes_fts_stemmed previously had no
    // `tokenize=` clause, so `use_stemming` selected between two
    // byte-for-byte identical tables. Confirm the real fix here too.
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "fiction", "<p>the athlete went running yesterday</p>", &HashSet::new())
        .unwrap();

    let non_stemmed = notes.search("run", false, None, None, &[], true, None).unwrap();
    assert!(non_stemmed.is_empty(), "the plain 'calibre' tokenizer should not stem, so 'run' should not match 'running'");

    let stemmed = notes.search("run", true, None, None, &[], true, None).unwrap();
    assert_eq!(stemmed.len(), 1, "the 'porter calibre' tokenizer should stem 'running' down to 'run'");
    assert_eq!(stemmed[0].item_id, 1);
}

#[test]
fn search_with_an_empty_query_falls_back_to_all_notes() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "fiction", "<p>a note</p>", &HashSet::new())
        .unwrap();

    let results = notes
        .search("", false, None, None, &[], true, None)
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn search_with_highlight_markers_wraps_the_match() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note(
            "tags",
            1,
            "fiction",
            "<p>a note about dragons and knights</p>",
            &HashSet::new(),
        )
        .unwrap();

    let results = notes
        .search("dragons", false, Some(("[[", "]]")), None, &[], true, None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].text.as_deref().unwrap().contains("[[dragons]]"));
}

#[test]
fn a_malformed_fts_query_returns_a_real_fts_query_error() {
    let dir = tempdir().unwrap();
    let notes = open(dir.path());
    notes
        .set_note("tags", 1, "fiction", "<p>a note</p>", &HashSet::new())
        .unwrap();

    let err = notes
        .search("\"unterminated", false, None, None, &[], true, None)
        .unwrap_err();
    assert_eq!(err.query, "\"unterminated");
}

#[test]
fn library_notes_shares_the_librarys_live_connection() {
    use calibre_db::library::Library;

    let mut lib = Library::open_test().unwrap();
    lib.notes().initialize().unwrap();
    lib.notes()
        .set_note("tags", 1, "fiction", "<p>via Library</p>", &HashSet::new())
        .unwrap();

    assert_eq!(
        lib.notes().get_note("tags", 1).unwrap(),
        Some("<p>via Library</p>".to_string())
    );
}
