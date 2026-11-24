//! Tranche 26a regression tests: the findings of the design and usability
//! review that live in the engine.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::cache::Cache;
use crate::organize::{self, Scope};
use crate::scan::{walk, ScanOptions};

fn library() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn index(dir: &Path, cache: &Cache) {
    let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
    crate::scan::index_scan(cache, dir, &report);
}

fn only_book(cache: &Cache) -> i32 {
    let ids = cache.all_book_ids().unwrap();
    assert_eq!(ids.len(), 1);
    ids[0]
}

// --- B1: changing authors updates author_sort and last_modified -----------

#[test]
fn changing_authors_rederives_author_sort_so_organize_stops_filing_under_unknown() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Left Hand.pdf"), b"%PDF left hand");
    index(dir.path(), &cache);
    let id = only_book(&cache);
    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Unknown"));

    cache.set_field(id, "authors", "Frank Herbert").unwrap();

    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Herbert, Frank"));
    let plan = organize::plan(&cache, "{author_sort}/{title}", &Scope::All).unwrap();
    assert_eq!(plan.books.len(), 1);
    assert!(plan.books[0].new_path.starts_with("Herbert, Frank"), "{}", plan.books[0].new_path);
}

#[test]
fn several_authors_join_their_sorts_in_order() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Two.pdf"), b"%PDF two");
    index(dir.path(), &cache);
    let id = only_book(&cache);

    cache.set_field(id, "authors", "Jane Doe & John Smith").unwrap();

    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Doe, Jane & Smith, John"));
}

#[test]
fn an_explicit_author_sort_in_the_same_batch_wins_whatever_the_order() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Batch.pdf"), b"%PDF batch");
    index(dir.path(), &cache);
    let id = only_book(&cache);

    // Listed before the authors, which is the order a naive loop would let
    // the derived value overwrite.
    let changes = [("author_sort".to_string(), "Typed, Explicitly".to_string()), ("authors".to_string(), "Jane Doe".to_string())];
    for r in cache.set_fields_for_books(&[id], &changes) {
        r.unwrap();
    }
    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Typed, Explicitly"));

    // Authors alone, in the batch path, derive it.
    let changes = [("authors".to_string(), "Ann Other".to_string())];
    for r in cache.set_fields_for_books(&[id], &changes) {
        r.unwrap();
    }
    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Other, Ann"));
}

#[test]
fn every_field_write_bumps_last_modified() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Stamp.pdf"), b"%PDF stamp");
    index(dir.path(), &cache);
    let id = only_book(&cache);
    // Put it somewhere clearly in the past, so a bump is unmistakable.
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET last_modified = '2000-01-01 00:00:00+00:00' WHERE id = ?1", [id]).unwrap();
    }
    let modified = |cache: &Cache| -> String {
        let conn = cache.backend.conn.lock().unwrap();
        conn.query_row("SELECT last_modified FROM books WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    };
    assert!(modified(&cache).starts_with("2000-01-01"));

    cache.set_field(id, "tags", "one").unwrap();
    assert!(!modified(&cache).starts_with("2000-01-01"), "a field write must stamp last_modified, got {}", modified(&cache));

    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET last_modified = '2000-01-01 00:00:00+00:00' WHERE id = ?1", [id]).unwrap();
    }
    for r in cache.set_fields_for_books(&[id], &[("publisher".to_string(), "P".to_string())]) {
        r.unwrap();
    }
    assert!(!modified(&cache).starts_with("2000-01-01"), "the batch path too, got {}", modified(&cache));
}

// --- B9: pubdate stays undefined when the file has none --------------------

#[test]
fn an_imported_book_with_no_publication_date_has_the_undefined_date() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Dateless.pdf"), b"%PDF dateless");
    index(dir.path(), &cache);
    let id = only_book(&cache);

    let pubdate = cache.field_for(id, "pubdate").unwrap().unwrap_or_default();
    assert!(pubdate.starts_with("0101-01-01"), "pubdate was {pubdate:?}");
    // The timestamp (when the book was added) is a real, recent date.
    let timestamp = cache.field_for(id, "timestamp").unwrap().unwrap_or_default();
    assert!(!timestamp.starts_with("0101"), "{timestamp:?}");
}
