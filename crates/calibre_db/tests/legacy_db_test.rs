use calibre_db::legacy::{LegacyDB, LegacyDb};
use tempfile::tempdir;

#[test]
fn test_legacy_check() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("metadata.db");

    let legacy = LegacyDB::new();

    // Non-existent DB is compatible (fresh start)
    assert!(legacy.check_compatibility(&db_path).unwrap());

    // Migration always fails (stub)
    assert!(legacy.migrate(&db_path).is_err());
}

fn insert_book(db: &LegacyDb, title: &str) -> i32 {
    let cache = db.new_api.lock().unwrap();
    let conn = cache.backend.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO books (title, author_sort, timestamp, pubdate, series_index) VALUES (?1, 'Unknown', '2020-01-01', '2020-01-01', 1.0)",
        [title],
    )
    .unwrap();
    conn.last_insert_rowid() as i32
}

#[test]
fn id_index_resolution_round_trips_through_the_view() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let a = insert_book(&db, "Book A");
    let b = insert_book(&db, "Book B");
    db.refresh();

    assert!(!db.is_empty());
    assert!(db.all_ids().contains(&a));
    assert!(db.all_ids().contains(&b));

    let idx_a = db.index(a).unwrap();
    assert_eq!(db.id(idx_a), Some(a));
    assert!(db.has_id(a));
    assert!(!db.has_id(9999));
}

#[test]
fn legacy_getters_resolve_by_index_and_by_id() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let book_id = insert_book(&db, "The Great Book");
    db.refresh();

    assert_eq!(db.title(book_id, true), Some("The Great Book".to_string()));
    let idx = db.index(book_id).unwrap();
    assert_eq!(
        db.title(idx as i32, false),
        Some("The Great Book".to_string())
    );
    // `comment`/`comments` are both aliases for the same field.
    assert_eq!(db.comment(book_id, true), None);
    assert_eq!(db.comments(book_id, true), None);
}

#[test]
fn setters_round_trip_through_getters_for_every_standard_field() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Original Title");

    db.set_title(id, "New Title").unwrap();
    db.set_title_sort(id, "Title, New").unwrap();
    db.set_author_sort(id, "Doe, Jane").unwrap();
    db.set_authors(id, "Jane Doe & John Roe").unwrap();
    db.set_comment(id, "Great read").unwrap();
    db.set_has_cover(id, true).unwrap();
    db.set_identifiers(id, "isbn:1234567890,doi:abc").unwrap();
    db.set_languages(id, "eng, fra").unwrap();
    db.set_publisher(id, "Acme Books").unwrap();
    db.set_rating(id, 8).unwrap();
    db.set_series(id, "The Series").unwrap();
    db.set_series_index(id, 2.5).unwrap();
    db.set_tags(id, "fiction, adventure").unwrap();
    db.set_uuid(id, "abc-123").unwrap();

    assert_eq!(db.title(id, true), Some("New Title".to_string()));
    assert_eq!(db.title_sort(id, true), Some("Title, New".to_string()));
    assert_eq!(db.author_sort(id, true), Some("Doe, Jane".to_string()));
    assert_eq!(
        db.authors(id, true),
        Some("Jane Doe & John Roe".to_string())
    );
    assert_eq!(db.comment(id, true), Some("Great read".to_string()));
    assert!(db.has_cover(id));
    let idents = db.get_identifiers(id, true);
    assert_eq!(idents.get("isbn"), Some(&"1234567890".to_string()));
    assert_eq!(idents.get("doi"), Some(&"abc".to_string()));
    assert_eq!(db.isbn(id, true), Some("1234567890".to_string()));
    assert_eq!(db.languages(id, true), Some("eng, fra".to_string()));
    assert_eq!(db.publisher(id, true), Some("Acme Books".to_string()));
    assert_eq!(db.rating(id, true), Some("8".to_string()));
    assert_eq!(db.series(id, true), Some("The Series".to_string()));
    assert_eq!(db.series_index(id, true), Some("2.5".to_string()));
    let tags = db.get_tags(id);
    assert!(tags.contains("fiction"));
    assert!(tags.contains("adventure"));
    assert_eq!(db.uuid(id, true), Some("abc-123".to_string()));
}

#[test]
fn setting_title_or_uuid_to_empty_is_a_no_op_matching_upstream() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Keep Me");
    db.set_uuid(id, "real-uuid").unwrap();

    db.set_title(id, "").unwrap();
    db.set_uuid(id, "").unwrap();

    assert_eq!(db.title(id, true), Some("Keep Me".to_string()));
    assert_eq!(db.uuid(id, true), Some("real-uuid".to_string()));
}

#[test]
fn setting_series_or_rating_to_empty_clears_the_link() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Book");
    db.set_series(id, "Some Series").unwrap();
    db.set_rating(id, 5).unwrap();
    assert_eq!(db.series(id, true), Some("Some Series".to_string()));
    assert_eq!(db.rating(id, true), Some("5".to_string()));

    db.set_series(id, "").unwrap();
    db.set_rating(id, 0).unwrap();
    assert_eq!(db.series(id, true), None);
    assert_eq!(db.rating(id, true), None);
}

#[test]
fn item_id_maps_and_names_reflect_real_rows() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Book");
    db.set_tags(id, "fiction, adventure").unwrap();
    db.set_series(id, "The Series").unwrap();

    assert!(db.all_tag_names().contains(&"fiction".to_string()));
    assert!(db.all_series_names().contains(&"The Series".to_string()));

    let series_id = db.series_id(id, true).unwrap();
    assert_eq!(db.series_name(series_id), Some("The Series".to_string()));

    let tag_ids: Vec<i32> = db
        .get_tags_with_ids()
        .into_iter()
        .filter(|(_, name)| name == "fiction")
        .map(|(tid, _)| tid)
        .collect();
    assert_eq!(tag_ids.len(), 1);
    assert_eq!(db.tag_name(tag_ids[0]), Some("fiction".to_string()));
}

#[test]
fn delete_tag_using_id_removes_it_from_the_book_and_the_table() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Book");
    db.set_tags(id, "fiction").unwrap();
    let tag_id = db
        .get_tags_with_ids()
        .into_iter()
        .find(|(_, n)| n == "fiction")
        .unwrap()
        .0;

    db.delete_tag_using_id(tag_id);

    assert!(db.get_tags(id).is_empty());
    assert!(!db.all_tag_names().contains(&"fiction".to_string()));
}

#[test]
fn rename_tag_merges_into_an_existing_tag_with_the_same_new_name() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let a = insert_book(&db, "Book A");
    let b = insert_book(&db, "Book B");
    db.set_tags(a, "sci-fi").unwrap();
    db.set_tags(b, "scifi").unwrap();

    let sci_fi_id = db
        .get_tags_with_ids()
        .into_iter()
        .find(|(_, n)| n == "sci-fi")
        .unwrap()
        .0;
    // Renaming "sci-fi" -> "scifi" collides with the existing "scifi"
    // tag, so book A's link should be re-pointed at it rather than
    // erroring or duplicating the tag row.
    db.rename_tag(sci_fi_id, "scifi");

    assert!(db.get_tags(a).contains("scifi"));
    assert!(db.get_tags(b).contains("scifi"));
    assert_eq!(
        db.all_tag_names()
            .iter()
            .filter(|n| n.as_str() == "scifi")
            .count(),
        1
    );
}

#[test]
fn get_categories_delegates_to_the_real_categories_module() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "Book");
    db.set_tags(id, "fiction").unwrap();

    let cats = db.get_categories("name", None).unwrap();
    let tags = cats.get("tags").expect("tags category present");
    assert!(tags.iter().any(|t| t.name == "fiction"));
}

#[test]
fn find_identical_books_matches_same_author_and_fuzzy_title() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let id = insert_book(&db, "The Great Book");
    db.set_authors(id, "Jane Doe").unwrap();

    let found = db
        .find_identical_books("the   great book", &["Jane Doe".to_string()])
        .unwrap();
    assert!(found.contains(&id));

    let not_found = db
        .find_identical_books("A Totally Different Title", &["Jane Doe".to_string()])
        .unwrap();
    assert!(not_found.is_empty());
}

#[test]
fn get_data_as_dict_includes_every_book() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    insert_book(&db, "Book One");
    insert_book(&db, "Book Two");

    let data = db.get_data_as_dict().unwrap();
    assert_eq!(data.len(), 2);
}

#[test]
fn has_book_is_case_insensitive() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    insert_book(&db, "The Great Book");

    assert!(db.has_book("the great book"));
    assert!(!db.has_book("Some Other Book"));
}

#[test]
fn get_next_series_num_for_increments_from_the_highest_existing_index() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let a = insert_book(&db, "Book One");
    let b = insert_book(&db, "Book Two");
    db.set_series(a, "The Series").unwrap();
    db.set_series_index(a, 1.0).unwrap();
    db.set_series(b, "The Series").unwrap();
    db.set_series_index(b, 2.0).unwrap();

    assert_eq!(db.get_next_series_num_for("The Series"), 3.0);
    // A series with no books yet starts fresh.
    assert_eq!(db.get_next_series_num_for("Unknown Series"), 1.0);
}

#[test]
fn author_sort_from_authors_matches_the_real_join_convention() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let sort = db.author_sort_from_authors(&["Jane Doe".to_string(), "John Roe".to_string()]);
    assert_eq!(sort, "Doe, Jane & Roe, John");
}

#[test]
fn standard_field_keys_includes_the_fields_field_for_supports() {
    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();
    let keys = db.standard_field_keys();
    assert!(keys.contains(&"title"));
    assert!(keys.contains(&"tags"));
    assert!(keys.contains(&"identifiers"));
}

#[test]
fn find_books_in_directory_and_import_book_directory_delegate_to_adding_rs() {
    let src = tempdir().unwrap();
    std::fs::write(src.path().join("book.epub"), b"content").unwrap();
    std::fs::write(src.path().join("book.mobi"), b"content").unwrap();

    let dir = tempdir().unwrap();
    let db = LegacyDb::new(dir.path()).unwrap();

    let groups = db.find_books_in_directory(src.path(), true);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);

    let book_id = db.import_book_directory(src.path()).unwrap();
    assert!(book_id.is_some());
}
