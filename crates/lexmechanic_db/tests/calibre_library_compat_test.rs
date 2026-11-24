//! Opening a library that real calibre made.
//!
//! Every other test in this crate builds its library with Lexmechanic's own
//! code, so none of them can notice a place where Lexmechanic's idea of the
//! schema differs from calibre's. These open a library built from
//! calibre's own `metadata_sqlite.sql` and `create_custom_column` (see
//! `common/mod.rs`) and check Lexmechanic reads it, writes it without
//! damaging it, and can copy out of it.
//!
//! Note on journal mode: Lexmechanic switches `metadata.db` to
//! `journal_mode=WAL` (and `synchronous=FULL`) on open. calibre leaves
//! it in the default rollback journal. WAL is persistent in the file, so
//! simply opening a library with Lexmechanic changes that pragma; it is the one
//! deliberate difference, asserted below rather than hidden.

mod common;

use lexmechanic_db::cache::Cache;
use lexmechanic_db::copy_to_library::copy_one_book_detailed;
use common::*;
use rusqlite::Connection;
use std::collections::BTreeSet;

fn open() -> (CalibreFixture, Cache) {
    let fixture = build_calibre_library();
    let cache = Cache::new(fixture.root()).expect("Lexmechanic could not open a calibre-made library");
    (fixture, cache)
}

/// The library without its normalized custom columns (see
/// `copy_from_a_library_with_normalized_custom_columns_works`).
fn open_plain() -> (CalibreFixture, Cache) {
    let fixture = build_calibre_library_with(true);
    let cache = Cache::new(fixture.root()).expect("Lexmechanic could not open a calibre-made library");
    (fixture, cache)
}

fn book(id: i32) -> &'static FixtureBook {
    BOOKS.iter().find(|b| b.id == id).unwrap()
}

fn set_of(s: Option<String>, sep: &str) -> BTreeSet<String> {
    s.map(|s| s.split(sep).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()).unwrap_or_default()
}

fn raw(root: &std::path::Path) -> Connection {
    Connection::open(root.join("metadata.db")).unwrap()
}

// ---------------------------------------------------------------------
// The fixture itself
// ---------------------------------------------------------------------

#[test]
fn the_fixture_is_a_valid_calibre_database() {
    let fixture = build_calibre_library();
    let conn = raw(fixture.root());
    let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok");
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 26, "metadata_sqlite.sql ends with `pragma user_version=26`");
    let columns: i64 = conn.query_row("SELECT COUNT(*) FROM custom_columns", [], |r| r.get(0)).unwrap();
    assert_eq!(columns as usize, CUSTOM_COLUMNS.len());
}

// ---------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------

#[test]
fn opening_a_calibre_library_does_not_alter_its_data() {
    let fixture = build_calibre_library();
    let before = dump_database(fixture.root());
    {
        let cache = Cache::new(fixture.root()).expect("open");
        // Read everything, as a UI would, so lazy migrations would fire.
        for id in cache.all_book_ids().unwrap() {
            for f in ["title", "authors", "tags", "series", "formats", "identifiers", "comments"] {
                let _ = cache.field_for(id, f).unwrap();
            }
        }
    }
    let after = dump_database(fixture.root());
    assert_eq!(before, after, "opening and reading must not change a single schema object or row");

    // The one intended difference: Lexmechanic's WAL mode, which persists.
    let mode: String = raw(fixture.root()).query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
}

#[test]
fn every_book_is_listed() {
    let (_f, cache) = open();
    let mut ids = cache.all_book_ids().unwrap();
    ids.sort();
    assert_eq!(ids, BOOKS.iter().map(|b| b.id).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------
// Standard fields
// ---------------------------------------------------------------------

#[test]
fn standard_fields_read_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        let id = b.id;
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some(b.title), "title of {id}");
        assert_eq!(cache.field_for(id, "sort").unwrap().as_deref(), Some(b.sort), "sort of {id}");
        assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some(b.author_sort), "author_sort of {id}");
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some(b.rel_dir().as_str()), "path of {id}");
        assert_eq!(cache.field_for(id, "comments").unwrap().as_deref(), b.comments, "comments of {id}");
        assert_eq!(cache.field_for(id, "publisher").unwrap().as_deref(), b.publisher, "publisher of {id}");

        let authors: Vec<&str> = b.authors.iter().map(|a| a.0).collect();
        assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some(authors.join(" & ").as_str()), "authors of {id}");

        let tags: BTreeSet<String> = b.tags.iter().map(|t| t.to_string()).collect();
        assert_eq!(set_of(cache.field_for(id, "tags").unwrap(), ","), tags, "tags of {id}");

        let langs: Vec<&str> = b.languages.to_vec();
        let got_langs = cache.field_for(id, "languages").unwrap().map(|s| s.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(got_langs, langs, "languages of {id}, in item_order");

        let idents: BTreeSet<String> = b.identifiers.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        assert_eq!(set_of(cache.field_for(id, "identifiers").unwrap(), ","), idents, "identifiers of {id}");

        let formats: BTreeSet<String> = b.formats.iter().map(|f| f.to_string()).collect();
        assert_eq!(set_of(cache.field_for(id, "formats").unwrap(), ","), formats, "formats of {id}");

        let rating = cache.field_for(id, "rating").unwrap().map(|r| r.parse::<f64>().unwrap() as i32);
        assert_eq!(rating, b.rating, "rating of {id}");
    }
}

#[test]
fn series_and_series_index_read_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(cache.field_for(b.id, "series").unwrap().as_deref(), b.series.map(|s| s.0), "series of {}", b.id);
        if let Some((_, idx)) = b.series {
            let got: f64 = cache.field_for(b.id, "series_index").unwrap().unwrap().parse().unwrap();
            assert_eq!(got, idx, "series_index of {}", b.id);
        }
    }
}

// ---------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------

#[test]
fn formats_and_covers_resolve_to_real_files() {
    let (f, cache) = open();
    for b in BOOKS {
        let infos = cache.format_file_info(b.id).unwrap();
        let got: BTreeSet<String> = infos.iter().map(|i| i.fmt.to_uppercase()).collect();
        assert_eq!(got, b.formats.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(), "formats of {}", b.id);
        for info in &infos {
            assert!(info.path.is_file(), "format {} of book {} resolved to {} which is not a file", info.fmt, b.id, info.path.display());
            assert_eq!(std::fs::read(&info.path).unwrap(), b.format_bytes(&info.fmt.to_uppercase()), "content of {}", info.path.display());
            assert_eq!(info.size as usize, b.format_bytes(&info.fmt.to_uppercase()).len(), "recorded size of {}", info.path.display());
            assert_eq!(info.path, b.format_path(f.root(), &info.fmt.to_uppercase()));
        }

        assert_eq!(cache.has_cover(b.id).unwrap(), b.has_cover, "has_cover of {}", b.id);
        if b.has_cover {
            let cover = lexmechanic_db::covers::cover_path(&cache, b.id).unwrap();
            assert!(cover.is_file(), "cover of book {} resolved to {} which is not a file", b.id, cover.display());
            assert_eq!(std::fs::read(&cover).unwrap(), b.cover_bytes());
        }
    }
}

// ---------------------------------------------------------------------
// Custom columns, one test per datatype so a failure names the type
// ---------------------------------------------------------------------

fn custom(cache: &Cache, id: i32, label: &str) -> Option<String> {
    cache.get_custom_column_value(id, label).unwrap_or_else(|e| panic!("reading #{label} of book {id} failed: {e:#}"))
}

#[test]
fn custom_columns_are_listed_with_their_calibre_definitions() {
    let (_f, cache) = open();
    let map = cache.custom_column_label_map().unwrap();
    assert_eq!(map.len(), CUSTOM_COLUMNS.len());
    for c in CUSTOM_COLUMNS {
        let def = &map[c.label];
        assert_eq!(def["datatype"], c.datatype, "{}", c.label);
        assert_eq!(def["name"], c.name, "{}", c.label);
        assert_eq!(def["is_multiple"], c.is_multiple, "{}", c.label);
        let normalized = !matches!(c.datatype, "datetime" | "comments" | "int" | "bool" | "float");
        assert_eq!(def["normalized"], normalized, "{}", c.label);
    }
}

#[test]
fn custom_column_comments_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(custom(&cache, b.id, "notes").as_deref(), b.custom.notes, "book {}", b.id);
    }
}

#[test]
fn custom_column_int_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(custom(&cache, b.id, "pages"), b.custom.pages.map(|p| p.to_string()), "book {}", b.id);
    }
}

#[test]
fn custom_column_float_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        let got = custom(&cache, b.id, "score").map(|s| s.parse::<f64>().unwrap());
        assert_eq!(got, b.custom.score, "book {}", b.id);
    }
}

#[test]
fn custom_column_bool_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(custom(&cache, b.id, "read"), b.custom.read.map(|v| (v as i32).to_string()), "book {}", b.id);
    }
}

#[test]
fn custom_column_datetime_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(custom(&cache, b.id, "finished").as_deref(), b.custom.finished, "book {}", b.id);
    }
}

#[test]
fn custom_column_multiple_text_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        let expect: BTreeSet<String> = b.custom.genre.iter().map(|s| s.to_string()).collect();
        assert_eq!(set_of(custom(&cache, b.id, "genre"), ","), expect, "book {}", b.id);
    }
}

#[test]
fn custom_column_series_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        let got = custom(&cache, b.id, "saga");
        match b.custom.saga {
            None => assert_eq!(got, None, "book {}", b.id),
            Some((name, _extra)) => assert!(got.unwrap().contains(name), "book {}", b.id),
        }
    }
}

#[test]
fn custom_column_enumeration_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        assert_eq!(custom(&cache, b.id, "mood").as_deref(), b.custom.mood, "book {}", b.id);
    }
}

#[test]
fn custom_column_rating_reads_back() {
    let (_f, cache) = open();
    for b in BOOKS {
        let got = custom(&cache, b.id, "stars").map(|s| s.parse::<f64>().unwrap() as i64);
        assert_eq!(got, b.custom.stars, "book {}", b.id);
    }
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

fn schema_objects(root: &std::path::Path) -> BTreeSet<String> {
    let conn = raw(root);
    let mut stmt = conn.prepare("SELECT type || ' ' || name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'").unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// Everything calibre's schema promises, checked on the file itself.
fn assert_still_a_valid_calibre_library(root: &std::path::Path, schema_before: &BTreeSet<String>) {
    let conn = raw(root);
    let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok", "PRAGMA integrity_check");
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 26);

    let schema_after = schema_objects(root);
    let lost: Vec<_> = schema_before.difference(&schema_after).collect();
    assert!(lost.is_empty(), "tables/triggers/views/indexes calibre created went missing: {lost:?}");

    // calibre's own foreign-key triggers are still in force: a link to a
    // book that does not exist must be refused.
    let err = conn.execute("INSERT INTO books_tags_link(book, tag) VALUES (99999, 1)", []).unwrap_err();
    assert!(err.to_string().contains("Foreign key violation"), "the fkc_ trigger no longer fires: {err}");
}

#[test]
fn writing_a_standard_field_and_a_tag_round_trips_and_leaves_calibres_schema_intact() {
    let fixture = build_calibre_library();
    let schema_before = schema_objects(fixture.root());
    {
        let cache = Cache::new(fixture.root()).unwrap();
        cache.set_field(1, "title", "The Hobbit, or There and Back Again").unwrap();
        cache.set_field(4, "tags", "Brand New, Another").unwrap();
        cache.set_field(2, "tags", "Fantasy, Sequel").unwrap();
    }
    {
        let cache = Cache::new(fixture.root()).unwrap();
        assert_eq!(cache.field_for(1, "title").unwrap().as_deref(), Some("The Hobbit, or There and Back Again"));
        assert_eq!(set_of(cache.field_for(4, "tags").unwrap(), ","), ["Another", "Brand New"].map(String::from).into());
        assert_eq!(set_of(cache.field_for(2, "tags").unwrap(), ","), ["Fantasy", "Sequel"].map(String::from).into());
        // Untouched books are untouched.
        assert_eq!(set_of(cache.field_for(3, "tags").unwrap(), ","), ["Fantasy", "Humor"].map(String::from).into());
    }
    // calibre's own tables, read straight: the link rows are really there.
    let tags: i64 = raw(fixture.root())
        .query_row("SELECT COUNT(*) FROM books_tags_link l JOIN tags t ON t.id=l.tag WHERE l.book=4 AND t.name IN ('Brand New','Another')", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tags, 2);
    assert_still_a_valid_calibre_library(fixture.root(), &schema_before);
}

#[test]
fn writing_a_plain_custom_column_round_trips_and_leaves_calibres_schema_intact() {
    let fixture = build_calibre_library();
    let schema_before = schema_objects(fixture.root());
    {
        let cache = Cache::new(fixture.root()).unwrap();
        cache.set_custom_column_value(4, "pages", "128").unwrap();
        cache.set_custom_column_value(1, "pages", "311").unwrap();
        cache.set_custom_column_value(4, "notes", "<p>new</p>").unwrap();
    }
    let cache = Cache::new(fixture.root()).unwrap();
    assert_eq!(custom(&cache, 4, "pages").as_deref(), Some("128"));
    assert_eq!(custom(&cache, 1, "pages").as_deref(), Some("311"), "an overwrite replaces, it does not add a second row");
    assert_eq!(custom(&cache, 4, "notes").as_deref(), Some("<p>new</p>"));
    let rows: i64 = raw(fixture.root()).query_row("SELECT COUNT(*) FROM custom_column_5 WHERE book=1", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1);
    assert_still_a_valid_calibre_library(fixture.root(), &schema_before);
}

#[test]
fn writing_a_normalized_custom_column_round_trips() {
    let fixture = build_calibre_library();
    let schema_before = schema_objects(fixture.root());
    {
        let cache = Cache::new(fixture.root()).unwrap();
        cache.set_custom_column_value(4, "genre", "Noir, Classic").unwrap();
    }
    let cache = Cache::new(fixture.root()).unwrap();
    assert_eq!(set_of(custom(&cache, 4, "genre"), ","), ["Classic", "Noir"].map(String::from).into());
    // calibre's own link table holds the values, not Lexmechanic's.
    let n: i64 = raw(fixture.root()).query_row("SELECT COUNT(*) FROM books_custom_column_1_link WHERE book=4", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
    assert_still_a_valid_calibre_library(fixture.root(), &schema_before);
}

// ---------------------------------------------------------------------
// Copy to library
// ---------------------------------------------------------------------

fn assert_copy_carries_files(src: &Cache, dest: &Cache, src_id: i32, dest_id: i32, b: &FixtureBook) {
    assert_eq!(dest.field_for(dest_id, "title").unwrap().as_deref(), Some(b.title));
    assert_eq!(dest.field_for(dest_id, "authors").unwrap(), src.field_for(src_id, "authors").unwrap());
    assert_eq!(set_of(dest.field_for(dest_id, "tags").unwrap(), ","), b.tags.iter().map(|t| t.to_string()).collect());
    assert_eq!(dest.field_for(dest_id, "series").unwrap().as_deref(), b.series.map(|s| s.0));
    assert_eq!(dest.field_for(dest_id, "publisher").unwrap().as_deref(), b.publisher);
    assert_eq!(dest.field_for(dest_id, "comments").unwrap().as_deref(), b.comments);

    let idents: BTreeSet<String> = b.identifiers.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    assert_eq!(set_of(dest.field_for(dest_id, "identifiers").unwrap(), ","), idents);

    let mut formats = dest.format_file_info(dest_id).unwrap();
    formats.sort_by(|a, b| a.fmt.cmp(&b.fmt));
    assert_eq!(formats.iter().map(|f| f.fmt.to_uppercase()).collect::<Vec<_>>(), b.formats.iter().map(|s| s.to_string()).collect::<Vec<_>>().iter().cloned().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>());
    for f in &formats {
        assert!(f.path.is_file(), "copied format {} is not a file", f.path.display());
        assert_eq!(std::fs::read(&f.path).unwrap(), b.format_bytes(&f.fmt.to_uppercase()));
    }

    assert_eq!(dest.has_cover(dest_id).unwrap(), b.has_cover);
    if b.has_cover {
        let cover = lexmechanic_db::covers::cover_path(dest, dest_id).unwrap();
        assert_eq!(std::fs::read(&cover).unwrap(), b.cover_bytes());
    }
}

#[test]
fn copy_to_a_fresh_lexmechanic_library_carries_formats_cover_and_metadata() {
    let (_src_fixture, src) = open_plain();
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    for b in BOOKS {
        let copied = copy_one_book_detailed(&src, &dest, b.id, false).unwrap().expect("not a duplicate");
        assert_copy_carries_files(&src, &dest, b.id, copied.book_id, b);
    }
}

#[test]
fn copy_to_a_calibre_made_library_with_matching_columns_carries_plain_custom_values() {
    let (_src_fixture, src) = open_plain();
    let dest_fixture = build_empty_calibre_library_with(true);
    let dest = Cache::new(dest_fixture.root()).unwrap();
    let b = book(1);
    let copied = copy_one_book_detailed(&src, &dest, b.id, false).unwrap().unwrap();
    assert_copy_carries_files(&src, &dest, b.id, copied.book_id, b);
    assert_eq!(custom(&dest, copied.book_id, "pages").as_deref(), Some("310"));
    assert_eq!(custom(&dest, copied.book_id, "notes").as_deref(), b.custom.notes);
    assert_eq!(custom(&dest, copied.book_id, "read").as_deref(), Some("1"));
    assert_eq!(custom(&dest, copied.book_id, "finished").as_deref(), b.custom.finished);
    let score: f64 = custom(&dest, copied.book_id, "score").unwrap().parse().unwrap();
    assert_eq!(score, 9.5);
}

#[test]
fn copy_to_a_calibre_made_library_with_matching_columns_carries_normalized_custom_values() {
    let (_src_fixture, src) = open();
    let dest_fixture = build_empty_calibre_library();
    let dest = Cache::new(dest_fixture.root()).unwrap();
    let b = book(1);
    let copied = copy_one_book_detailed(&src, &dest, b.id, false).unwrap().unwrap();
    assert!(copied.skipped_custom_columns.is_empty(), "skipped: {:?}", copied.skipped_custom_columns);
    assert_eq!(set_of(custom(&dest, copied.book_id, "genre"), ","), b.custom.genre.iter().map(|s| s.to_string()).collect());
    assert_eq!(custom(&dest, copied.book_id, "mood").as_deref(), Some("Happy"));
    assert_still_a_valid_calibre_library(dest_fixture.root(), &BTreeSet::new());
}

#[test]
fn copy_leaves_the_source_library_unchanged() {
    let (src_fixture, src) = open_plain();
    let before = dump_database(src_fixture.root());
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    copy_one_book_detailed(&src, &dest, 1, false).unwrap().unwrap();
    drop(src);
    assert_eq!(dump_database(src_fixture.root()), before);
    assert!(book(1).format_path(src_fixture.root(), "EPUB").is_file());
}

#[test]
fn copy_from_a_library_with_normalized_custom_columns_works() {
    let (_src_fixture, src) = open();
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    let b = book(1);
    let copied = copy_one_book_detailed(&src, &dest, b.id, false).expect("a calibre library with series/enum/rating columns must be copyable").unwrap();
    assert_copy_carries_files(&src, &dest, b.id, copied.book_id, b);
}
