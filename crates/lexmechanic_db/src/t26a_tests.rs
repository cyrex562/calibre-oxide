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

    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET last_modified = '2000-01-01 00:00:00+00:00' WHERE id = ?1", [id]).unwrap();
    }
    for r in cache.set_fields_per_book(&[(id, vec![("tags".to_string(), "one, two".to_string())])]) {
        r.unwrap();
    }
    assert!(!modified(&cache).starts_with("2000-01-01"), "the per-book batch path too, got {}", modified(&cache));
}

#[test]
fn per_book_batch_rederives_author_sort_and_an_explicit_one_wins() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Dune.pdf"), b"%PDF dune");
    index(dir.path(), &cache);
    let id = only_book(&cache);
    for r in cache.set_fields_per_book(&[(id, vec![("authors".to_string(), "Frank Herbert".to_string())])]) {
        r.unwrap();
    }
    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Herbert, Frank"));
    let changes = vec![("author_sort".to_string(), "Custom".to_string()), ("authors".to_string(), "Ann Other".to_string())];
    for r in cache.set_fields_per_book(&[(id, changes)]) {
        r.unwrap();
    }
    assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Custom"));
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

// --- #1 of the PR #1000 review: a book at `<path>/data` is not the data files of `<path>` ---

#[test]
fn a_book_scanned_in_a_data_subfolder_is_not_deleted_with_the_book_above_it() {
    let (dir, cache) = library();
    write(&dir.path().join("Reports/report.pdf"), b"%PDF report");
    write(&dir.path().join("Reports/data/dataset.pdf"), b"%PDF dataset");
    index(dir.path(), &cache);
    let ids = cache.all_book_ids().unwrap();
    assert_eq!(ids.len(), 2);
    let id_of = |t: &str| ids.iter().copied().find(|id| cache.field_for(*id, "title").unwrap().as_deref() == Some(t)).unwrap();
    let (a, b) = (id_of("report"), id_of("dataset"));
    assert_eq!(cache.field_for(b, "path").unwrap().as_deref(), Some("Reports/data"));

    assert!(!cache.folder_is_dedicated(a, "Reports").unwrap(), "a book lives under Reports/data");
    assert!(crate::extra_files::list_extra_files(&cache, a, crate::constants::DATA_FILE_PATTERN).unwrap().is_empty(), "dataset.pdf is a book's format, not an attached file");
    let own: Vec<String> = cache.own_files(a, "Reports", false).unwrap().into_iter().map(|f| f.rel).collect();
    assert_eq!(own, vec!["Reports/report.pdf".to_string()]);

    cache.delete_book(a).unwrap();
    assert!(!dir.path().join("Reports/report.pdf").exists());
    assert!(dir.path().join("Reports/data/dataset.pdf").is_file(), "the other book's file was trashed with this one");
}

// --- #2: add paths store the sort string ----------------------------------

#[test]
fn add_book_and_update_metadata_store_the_sort_string_and_a_rebuild_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("x.pdf");
        write(&source, b"%PDF x");
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = "Pair".to_string();
        meta.authors = vec!["Frank Herbert".to_string(), "Jane Doe".to_string()];
        let id = cache.add_book(&source, &meta).unwrap();
        assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Herbert, Frank & Doe, Jane"));

        cache.update_book_metadata(id, "Pair", "Mary Smith").unwrap();
        assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some("Smith, Mary"));
        id
    };
    let cache = Cache::new(dir.path()).unwrap();
    let before = cache.field_for(id, "author_sort").unwrap();
    cache.rebuild_from_change_log().unwrap();
    let id = cache.all_book_ids().unwrap()[0];
    assert_eq!(cache.field_for(id, "author_sort").unwrap(), before);
}

// --- #3: renaming or merging an author updates the books -------------------

#[test]
fn renaming_or_merging_an_author_updates_the_books_sort_and_stamp() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/One.pdf"), b"%PDF one");
    write(&dir.path().join("inbox/Two.pdf"), b"%PDF two");
    index(dir.path(), &cache);
    let ids = cache.all_book_ids().unwrap();
    cache.set_field(ids[0], "authors", "Jon Doe").unwrap();
    cache.set_field(ids[1], "authors", "Ann Lee").unwrap();
    assert_eq!(cache.field_for(ids[0], "author_sort").unwrap().as_deref(), Some("Doe, Jon"));
    let stamp = cache.field_for(ids[0], "last_modified").unwrap();
    std::thread::sleep(Duration::from_millis(5));

    let jon = cache.item_id_by_name("authors", "Jon Doe").unwrap().unwrap();
    cache.rename_author(jon, "Mary Smith").unwrap();
    assert_eq!(cache.field_for(ids[0], "author_sort").unwrap().as_deref(), Some("Smith, Mary"));
    assert_ne!(cache.field_for(ids[0], "last_modified").unwrap(), stamp);

    // Merge into an existing author.
    let mary = cache.item_id_by_name("authors", "Mary Smith").unwrap().unwrap();
    cache.rename_author(mary, "Ann Lee").unwrap();
    assert_eq!(cache.field_for(ids[0], "author_sort").unwrap().as_deref(), Some("Lee, Ann"));

    // A changed sort key reaches the books that follow it.
    cache.set_author_sort_key("Ann Lee", "Lee-Key, Ann").unwrap();
    assert_eq!(cache.field_for(ids[0], "author_sort").unwrap().as_deref(), Some("Lee-Key, Ann"));
    assert_eq!(cache.field_for(ids[1], "author_sort").unwrap().as_deref(), Some("Lee-Key, Ann"));
}

// --- #6: a relative list edit is computed inside the write transaction ------

#[test]
fn a_relative_list_edit_is_computed_from_what_the_book_has_when_it_is_written() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/One.pdf"), b"%PDF one");
    write(&dir.path().join("inbox/Two.pdf"), b"%PDF two");
    index(dir.path(), &cache);
    let ids = cache.all_book_ids().unwrap();
    cache.set_field(ids[0], "tags", "a, b").unwrap();
    // Written after any read the caller might have made earlier.
    cache.set_field(ids[1], "tags", "keep, me").unwrap();

    let results = cache.edit_lists_for_books(&ids, &|_, read| {
        let mut tags = read("tags")?;
        if tags.iter().any(|t| t == "b") {
            tags.retain(|t| t != "b");
            tags.push("c".to_string());
            Ok(vec![("tags".to_string(), tags.join(", "))])
        } else {
            Ok(Vec::new())
        }
    });
    assert_eq!(results[0].as_ref().unwrap().len(), 1);
    assert!(results[1].as_ref().unwrap().is_empty(), "nothing to change is not a write");
    assert_eq!(cache.field_for(ids[0], "tags").unwrap().as_deref(), Some("a, c"));
    assert_eq!(cache.field_for(ids[1], "tags").unwrap().as_deref(), Some("keep, me"));

    // A rebuild reproduces it.
    cache.rebuild_from_change_log().unwrap();
    let id = cache.all_book_ids().unwrap()[0];
    assert_eq!(cache.field_for(id, "tags").unwrap().as_deref(), Some("a, c"));
}
