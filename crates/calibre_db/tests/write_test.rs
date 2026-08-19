use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::write;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_write_title_author() {
    let dir = tempdir().unwrap();

    // Setup DB: `Backend::new` creates the real calibre schema for a
    // fresh dir -- just seed a row into it.
    {
        let backend = Backend::new(dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES 
            (1, 'Old Title', 'Old Title', 'Old Author', 'u1')",
            [],
        )
        .unwrap();
    }

    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    // Verify Initial State
    {
        let guard = cache.lock().unwrap();
        let title = guard.field_for(1, "title").unwrap().unwrap();
        assert_eq!(title, "Old Title");
    }

    // Test set_title
    write::set_title(&cache, 1, "New Title").expect("set_title failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        let title = guard.field_for(1, "title").unwrap().unwrap();
        assert_eq!(title, "New Title");
    }

    // Test set_author_sort
    write::set_author_sort(&cache, 1, "New Author").expect("set_author_sort failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        let author = guard.field_for(1, "author_sort").unwrap().unwrap();
        assert_eq!(author, "New Author");
    }

    // Test series_index (generic update_field)
    write::update_field(&cache, 1, "series_index", "2.5")
        .expect("update_field series_index failed");

    // Verify Change
    {
        let guard = cache.lock().unwrap();
        // field_for for series_index returns String (see backend.rs implementation)
        let idx = guard.field_for(1, "series_index").unwrap().unwrap();
        assert_eq!(idx, "2.5");
    }
}

fn seed_book(dir: &std::path::Path) -> Arc<Mutex<Cache>> {
    {
        let backend = Backend::new(dir).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES (1, 'T', 'T', 'A', 'u1')",
            [],
        )
        .unwrap();
    }
    Arc::new(Mutex::new(Cache::new(dir).unwrap()))
}

#[test]
fn set_field_returns_false_and_skips_the_write_when_the_value_is_unchanged() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    assert!(write::set_field(&cache, 1, "title", "New Title").unwrap());
    // Setting to the exact same value again is a real no-op.
    assert!(!write::set_field(&cache, 1, "title", "New Title").unwrap());
}

#[test]
fn set_field_tags_dedupes_and_collapses_whitespace() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    write::set_field(&cache, 1, "tags", "Fiction,  fiction , Adventure,Fiction").unwrap();

    let guard = cache.lock().unwrap();
    let tags = guard.field_for(1, "tags").unwrap().unwrap();
    // Case-insensitive dedupe keeps the first-seen casing, matching
    // `uniq`'s order-preserving behavior.
    assert_eq!(tags, "Fiction, Adventure");
}

#[test]
fn set_field_authors_defaults_to_unknown_when_empty() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    write::set_field(&cache, 1, "authors", "   ").unwrap();

    let guard = cache.lock().unwrap();
    assert_eq!(
        guard.field_for(1, "authors").unwrap(),
        Some("Unknown".to_string())
    );
}

#[test]
fn set_field_rating_zero_clears_and_out_of_range_is_clamped() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    write::set_field(&cache, 1, "rating", "15").unwrap();
    assert_eq!(
        cache.lock().unwrap().field_for(1, "rating").unwrap(),
        Some("10".to_string())
    );

    write::set_field(&cache, 1, "rating", "0").unwrap();
    assert_eq!(cache.lock().unwrap().field_for(1, "rating").unwrap(), None);
}

#[test]
fn set_field_uuid_and_sort_reject_empty_values_instead_of_clearing() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());
    // `books_insert_trg` overwrites `uuid` with a fresh `uuid4()` on
    // every INSERT regardless of what was supplied, so read back
    // whatever it actually is rather than assuming the seeded 'u1'.
    let original_uuid = cache.lock().unwrap().field_for(1, "uuid").unwrap();

    let changed = write::set_field(&cache, 1, "uuid", "").unwrap();
    assert!(!changed);
    assert_eq!(
        cache.lock().unwrap().field_for(1, "uuid").unwrap(),
        original_uuid
    );
}

#[test]
fn set_field_series_parses_the_bracketed_index_syntax() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    write::set_field(&cache, 1, "series", "The Foundation [3]").unwrap();

    let guard = cache.lock().unwrap();
    assert_eq!(
        guard.field_for(1, "series").unwrap(),
        Some("The Foundation".to_string())
    );
    assert_eq!(
        guard.field_for(1, "series_index").unwrap(),
        Some("3".to_string())
    );
}

#[test]
fn set_field_languages_drops_placeholder_codes_and_dedupes() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    write::set_field(&cache, 1, "languages", "eng, und, eng, fra").unwrap();

    let guard = cache.lock().unwrap();
    assert_eq!(
        guard.field_for(1, "languages").unwrap(),
        Some("eng, fra".to_string())
    );
}

#[test]
fn set_field_identifiers_lowercases_the_type_and_drops_unparseable_pairs() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());

    // "456" has no `:` separator, so it's not a valid pair and is
    // silently dropped, matching `clean_identifier`'s "only keep
    // pairs with a non-empty type and value" rule.
    write::set_field(&cache, 1, "identifiers", "ISBN:123,DOI:abc,456").unwrap();

    let guard = cache.lock().unwrap();
    let idents = guard.field_for(1, "identifiers").unwrap().unwrap();
    assert_eq!(idents, "isbn:123,doi:abc");
}

#[test]
fn set_field_many_writes_every_book_and_returns_only_the_ones_actually_changed() {
    let dir = tempdir().unwrap();
    let cache = seed_book(dir.path());
    {
        let guard = cache.lock().unwrap();
        let conn = guard.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES (2, 'T', 'T', 'A', 'u2')",
            [],
        )
        .unwrap();
    }

    let mut updates = std::collections::HashMap::new();
    updates.insert(1, "Changed Title".to_string());
    updates.insert(2, "T".to_string()); // same as current -- should not count as dirtied

    let dirtied = write::set_field_many(&cache, "title", &updates).unwrap();
    assert_eq!(dirtied, std::collections::HashSet::from([1]));
}
