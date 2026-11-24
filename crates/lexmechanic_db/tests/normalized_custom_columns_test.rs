//! Custom columns with calibre's on-disk semantics, checked against
//! libraries built by `common/mod.rs` (a transcription of calibre's own
//! `create_custom_column`) and by reading the tables back directly.

mod common;

use lexmechanic_db::cache::Cache;
use lexmechanic_db::copy_to_library::copy_one_book_detailed;
use common::*;
use rusqlite::Connection;
use std::collections::BTreeSet;

fn open() -> (CalibreFixture, Cache) {
    let fixture = build_calibre_library();
    let cache = Cache::new(fixture.root()).unwrap();
    (fixture, cache)
}

fn raw(root: &std::path::Path) -> Connection {
    Connection::open(root.join("metadata.db")).unwrap()
}

fn col_num(root: &std::path::Path, label: &str) -> i64 {
    raw(root).query_row("SELECT id FROM custom_columns WHERE label = ?1", [label], |r| r.get(0)).unwrap()
}

fn count(root: &std::path::Path, sql: &str) -> i64 {
    raw(root).query_row(sql, [], |r| r.get(0)).unwrap()
}

fn get(cache: &Cache, book: i32, label: &str) -> Option<String> {
    cache.get_custom_column_value(book, label).unwrap()
}

fn ids(v: Vec<i32>) -> BTreeSet<i32> {
    v.into_iter().collect()
}

fn assert_integrity(root: &std::path::Path) {
    let conn = raw(root);
    let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok");
}

// ---- series ---------------------------------------------------------

#[test]
fn series_index_reads_back_and_round_trips() {
    let (f, cache) = open();
    // What calibre wrote: 1.0 and 2.5 in `extra`.
    assert_eq!(cache.get_custom_series_index(1, "saga").unwrap(), Some(1.0));
    assert_eq!(cache.get_custom_series_index(2, "saga").unwrap(), Some(2.5));
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), None);
    assert_eq!(get(&cache, 2, "saga_index").as_deref(), Some("2.5"));
    assert_eq!(cache.field_for(2, "#saga_index").unwrap().as_deref(), Some("2.5"));

    // A name with its index in calibre's `Name [n]` form.
    cache.set_custom_column_value(4, "saga", "Tolkien Legendarium [7]").unwrap();
    assert_eq!(get(&cache, 4, "saga").as_deref(), Some("Tolkien Legendarium"));
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), Some(7.0));
    let link = format!("books_custom_column_{}_link", col_num(f.root(), "saga"));
    let extra: f64 = raw(f.root()).query_row(&format!("SELECT extra FROM {link} WHERE book=4"), [], |r| r.get(0)).unwrap();
    assert_eq!(extra, 7.0, "the index is in the link row's `extra`, as calibre keeps it");

    // The index alone.
    cache.set_custom_column_value(4, "saga_index", "9.5").unwrap();
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), Some(9.5));

    // Same series, no index given: the book keeps the one it has.
    cache.set_custom_column_value(4, "saga", "Tolkien Legendarium").unwrap();
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), Some(9.5));
    // A different series, no index given: the index carries over too.
    cache.set_custom_column_value(4, "saga", "Other Saga").unwrap();
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), Some(9.5));
    // A book that never had one starts at 1.0.
    cache.set_custom_column_value(3, "saga", "Other Saga").unwrap();
    assert_eq!(cache.get_custom_series_index(3, "saga").unwrap(), Some(1.0));

    // An index with no series is nothing.
    cache.set_custom_column_value(1, "saga", "").unwrap();
    cache.set_custom_column_value(1, "saga_index", "5").unwrap();
    assert_eq!(cache.get_custom_series_index(1, "saga").unwrap(), None);
    assert_integrity(f.root());
}

// ---- value rows -----------------------------------------------------

#[test]
fn a_value_row_is_reused_across_books_and_across_case() {
    let (f, cache) = open();
    let table = format!("custom_column_{}", col_num(f.root(), "genre"));
    let rows_before = count(f.root(), &format!("SELECT COUNT(*) FROM {table}"));
    assert_eq!(rows_before, 3); // High Fantasy, Classic, Comic Fantasy

    cache.set_custom_column_value(4, "genre", "high fantasy").unwrap();
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {table}")), rows_before, "no second row for the same value");
    assert_eq!(get(&cache, 4, "genre").as_deref(), Some("High Fantasy"), "the existing spelling is kept");
    let link = format!("books_custom_column_{}_link", col_num(f.root(), "genre"));
    let users: i64 = count(f.root(), &format!("SELECT COUNT(*) FROM {link} WHERE value=(SELECT id FROM {table} WHERE value='High Fantasy')"));
    assert_eq!(users, 3, "books 1, 2 and 4 share one row");

    cache.set_custom_column_value(4, "genre", "High Fantasy, Brand New").unwrap();
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {table}")), rows_before + 1);
    assert_integrity(f.root());
}

#[test]
fn clearing_removes_the_links_and_orphaned_values() {
    let (f, cache) = open();
    let table = format!("custom_column_{}", col_num(f.root(), "genre"));
    let link = format!("books_custom_column_{}_link", col_num(f.root(), "genre"));

    cache.set_custom_column_value(3, "genre", "").unwrap();
    assert_eq!(get(&cache, 3, "genre"), None);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {link} WHERE book=3")), 0);
    // "Comic Fantasy" belonged to book 3 alone and is gone; "Classic" is still book 1's.
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {table} WHERE value='Comic Fantasy'")), 0);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {table} WHERE value='Classic'")), 1);

    // The same for the other normalized kinds.
    cache.set_custom_column_value(2, "mood", "").unwrap();
    assert_eq!(get(&cache, 2, "mood"), None);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{} WHERE value='Tense'", col_num(f.root(), "mood"))), 0);
    cache.set_custom_column_value(2, "saga", "").unwrap();
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM books_custom_column_{}_link WHERE book=2", col_num(f.root(), "saga"))), 0);
    // And a plain column: the row is deleted, not set to an empty string.
    cache.set_custom_column_value(1, "pages", "").unwrap();
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{} WHERE book=1", col_num(f.root(), "pages"))), 0);
    assert_eq!(get(&cache, 1, "pages"), None);
    assert_integrity(f.root());
}

#[test]
fn a_multiple_value_column_keeps_the_order_it_was_written_in() {
    let (f, cache) = open();
    cache.set_custom_column_value(4, "genre", "Zebra, Apple,  Mango ,apple, ,Zebra").unwrap();
    // Order kept, case-insensitive duplicates and blanks dropped.
    assert_eq!(get(&cache, 4, "genre").as_deref(), Some("Zebra, Apple, Mango"));
    // It is the link rows' own order, not an accident of sorting.
    let link = format!("books_custom_column_{}_link", col_num(f.root(), "genre"));
    let table = format!("custom_column_{}", col_num(f.root(), "genre"));
    let conn = raw(f.root());
    let mut stmt = conn.prepare(&format!("SELECT v.value FROM {link} l JOIN {table} v ON v.id=l.value WHERE l.book=4 ORDER BY l.id")).unwrap();
    let in_link_order: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(in_link_order, ["Zebra", "Apple", "Mango"]);

    // Reordering is a real change and survives a reopen.
    cache.set_custom_column_value(4, "genre", "Mango, Zebra, Apple").unwrap();
    drop(cache);
    let cache = Cache::new(f.root()).unwrap();
    assert_eq!(get(&cache, 4, "genre").as_deref(), Some("Mango, Zebra, Apple"));
}

#[test]
fn a_rating_column_is_calibres_zero_to_ten_scale() {
    let (f, cache) = open();
    let table = format!("custom_column_{}", col_num(f.root(), "stars"));
    cache.set_custom_column_value(4, "stars", "7").unwrap();
    assert_eq!(get(&cache, 4, "stars").as_deref(), Some("7"));
    let stored: i64 = count(f.root(), &format!("SELECT value FROM {table} WHERE id=(SELECT value FROM books_custom_column_{}_link WHERE book=4)", col_num(f.root(), "stars")));
    assert_eq!(stored, 7, "stored as 0-10, not as stars");

    cache.set_custom_column_value(4, "stars", "15").unwrap();
    assert_eq!(get(&cache, 4, "stars").as_deref(), Some("10"), "clamped to 10");
    cache.set_custom_column_value(4, "stars", "0").unwrap();
    assert_eq!(get(&cache, 4, "stars"), None, "0 means no rating");
    // The value row 7 had was only used by book 4.
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {table} WHERE value=7")), 0);
    assert!(cache.set_custom_column_value(4, "stars", "lots").is_err());
}

#[test]
fn an_enumeration_only_takes_its_listed_values() {
    let (f, cache) = open();
    assert!(cache.set_custom_column_value(4, "mood", "Angry").is_err());
    assert_eq!(get(&cache, 4, "mood"), None);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{} WHERE value='Angry'", col_num(f.root(), "mood"))), 0);
    cache.set_custom_column_value(4, "mood", "Sad").unwrap();
    assert_eq!(get(&cache, 4, "mood").as_deref(), Some("Sad"));
    // A rejected write leaves the old value alone.
    assert!(cache.set_custom_column_value(4, "mood", "Angry").is_err());
    assert_eq!(get(&cache, 4, "mood").as_deref(), Some("Sad"));
}

#[test]
fn the_plain_datatypes_parse_what_calibre_parses() {
    let (_f, cache) = open();
    cache.set_custom_column_value(4, "read", "yes").unwrap_err();
    cache.set_custom_column_value(4, "read", "True").unwrap();
    assert_eq!(get(&cache, 4, "read").as_deref(), Some("1"));
    cache.set_custom_column_value(4, "read", "0").unwrap();
    assert_eq!(get(&cache, 4, "read").as_deref(), Some("0"));
    cache.set_custom_column_value(4, "read", "none").unwrap();
    assert_eq!(get(&cache, 4, "read"), None);
    cache.set_custom_column_value(4, "finished", "2020-05-06").unwrap();
    assert_eq!(get(&cache, 4, "finished").as_deref(), Some("2020-05-06 00:00:00+00:00"));
    assert!(cache.set_custom_column_value(4, "pages", "many").is_err());
    assert!(cache.set_custom_column_value(4, "score", "x").is_err());
}

// ---- search & categories -------------------------------------------

#[test]
fn search_finds_books_by_a_normalized_custom_column() {
    let (_f, cache) = open();
    let s = |q: &str| ids(lexmechanic_db::search::search(&cache, q).unwrap());
    assert_eq!(s("#genre:fantasy"), [1, 2, 3].into(), "contains match on any of a book's values");
    assert_eq!(s("#genre:\"=High Fantasy\""), [1, 2].into());
    assert_eq!(s("#genre:=Classic"), [1, 3].into());
    assert_eq!(s("#genre:fantasy and not #genre:classic"), [2].into());
    assert_eq!(s("#mood:happy"), [1].into());
    assert_eq!(s("#saga:legendarium"), [1, 2].into());
    assert_eq!(s("#stars:>=4"), [1, 2].into(), "searched in stars, stored 0-10");
    assert_eq!(s("#pages:>400"), [2].into());
    assert_eq!(s("#score:<1"), [3].into());
    assert_eq!(s("#genre:false"), [4].into(), "a book with no value");
    assert_eq!(s("#genre:true"), [1, 2, 3].into());
    assert_eq!(s("#read:yes"), [1].into());
    assert_eq!(s("#read:no"), [2].into());
    assert_eq!(s("#read:empty"), [3, 4].into());
    assert_eq!(s("#finished:2023"), [3].into());
    assert_eq!(s("#nonesuch:x"), BTreeSet::new());
}

#[test]
fn the_tag_browser_lists_normalized_custom_columns() {
    let (_f, cache) = open();
    let cats = lexmechanic_db::categories::get_categories(&cache, "name", None).unwrap();
    let genre = &cats["#genre"];
    let mut names: Vec<(&str, usize)> = genre.iter().map(|t| (t.name.as_str(), t.count)).collect();
    names.sort();
    assert_eq!(names, [("Classic", 2), ("Comic Fantasy", 1), ("High Fantasy", 2)]);
    assert_eq!(cats["#stars"].len(), 2, "rating values are items too");
    assert!(cats.contains_key("#saga") && cats.contains_key("#mood"));
    assert!(!cats.contains_key("#pages"), "a plain column is not a category");
    let id = lexmechanic_db::categories::get_item_id(&cache, "#genre", "Classic").unwrap().unwrap();
    assert_eq!(lexmechanic_db::categories::book_ids_for_category_item(&cache, "#genre", id).unwrap(), [1, 3].into());
}

// ---- copy -----------------------------------------------------------

#[test]
fn copy_between_two_calibre_made_libraries_carries_every_datatype() {
    let (_sf, src) = open();
    let dest_fixture = build_empty_calibre_library();
    let dest = Cache::new(dest_fixture.root()).unwrap();
    for b in BOOKS.iter().filter(|b| b.id <= 2) {
        let copied = copy_one_book_detailed(&src, &dest, b.id, false).unwrap().unwrap();
        assert!(copied.skipped_custom_columns.is_empty(), "skipped: {:?}", copied.skipped_custom_columns);
        for c in CUSTOM_COLUMNS {
            assert_eq!(get(&dest, copied.book_id, c.label), get(&src, b.id, c.label), "#{} of book {}", c.label, b.id);
        }
        assert_eq!(dest.get_custom_series_index(copied.book_id, "saga").unwrap(), src.get_custom_series_index(b.id, "saga").unwrap());
    }
    assert_eq!(get(&dest, 2, "genre").as_deref(), Some("High Fantasy"));
    assert_eq!(dest.get_custom_series_index(2, "saga").unwrap(), Some(2.5));
    // Both copied books share the one value row for "High Fantasy".
    let table = format!("custom_column_{}", col_num(dest_fixture.root(), "genre"));
    assert_eq!(count(dest_fixture.root(), &format!("SELECT COUNT(*) FROM {table} WHERE value='High Fantasy'")), 1);
    drop((src, dest));
    assert_integrity(dest_fixture.root());
}

#[test]
fn copy_skips_an_enumeration_the_destination_does_not_allow() {
    let (_sf, src) = open();
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    let narrower = serde_json::json!({"enum_values": ["Sad"], "enum_colors": [], "use_decorations": false});
    dest.add_custom_column_with_display("mood", "Mood", "enumeration", false, &narrower).unwrap();
    let copied = copy_one_book_detailed(&src, &dest, 1, false).unwrap().unwrap();
    assert!(copied.skipped_custom_columns.contains(&"mood".to_string()), "{:?}", copied.skipped_custom_columns);
    assert_eq!(get(&dest, copied.book_id, "mood"), None);
}

// ---- schema fidelity ------------------------------------------------

/// Whitespace-collapsed `sqlite_master` rows, by name.
fn schema(root: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let conn = raw(root);
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE (name LIKE '%custom_column%') AND name NOT LIKE 'sqlite_%'").unwrap();
    stmt.query_map([], |r| {
        let (ty, name, sql): (String, String, Option<String>) = (r.get(0)?, r.get(1)?, r.get(2)?);
        Ok((name.clone(), format!("{ty} {name}: {}", sql.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "))))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn a_column_made_by_lexmechanic_has_calibres_exact_schema_for_every_datatype() {
    let mut columns: Vec<CustomColumn> = CUSTOM_COLUMNS.iter().map(|c| CustomColumn { ..*c }).collect();
    columns.push(CustomColumn { label: "calc", name: "Calc", datatype: "composite", is_multiple: true, display: r#"{"composite_template": "{title}"}"# });
    // Both libraries get the columns in the same order, so the numbers agree.
    let calibre = tempfile::tempdir().unwrap();
    {
        let conn = Connection::open(calibre.path().join("metadata.db")).unwrap();
        conn.execute_batch(include_str!("../../../old_src/resources/metadata_sqlite.sql")).unwrap();
        // Functions the schema's views call are only needed when queried.
        for c in &columns {
            create_custom_column(&conn, c);
        }
    }
    let lexmechanic_dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(lexmechanic_dir.path()).unwrap();
    for c in &columns {
        let display: serde_json::Value = serde_json::from_str(c.display).unwrap();
        cache.add_custom_column_with_display(c.label, c.name, c.datatype, c.is_multiple, &display).unwrap();
    }
    let (theirs, ours) = (schema(calibre.path()), schema(lexmechanic_dir.path()));
    assert!(theirs.len() > 40, "{}", theirs.len());
    assert_eq!(ours, theirs);

    // And the definitions agree.
    let defs = |root: &std::path::Path| -> Vec<String> {
        let conn = raw(root);
        let mut stmt = conn.prepare("SELECT label, name, datatype, is_multiple, normalized, editable, mark_for_delete, display FROM custom_columns ORDER BY id").unwrap();
        stmt.query_map([], |r| {
            let display: serde_json::Value = serde_json::from_str(&r.get::<_, String>(7)?).unwrap();
            Ok(format!("{:?} {:?} {:?} {} {} {} {} {display}", r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    assert_eq!(defs(lexmechanic_dir.path()), defs(calibre.path()));
}

#[test]
fn removing_a_column_drops_everything_calibre_made_for_it() {
    let (f, cache) = open();
    cache.remove_custom_column("genre").unwrap();
    cache.remove_custom_column("saga").unwrap();
    let left = schema(f.root());
    assert!(!left.keys().any(|k| k.contains("custom_column_1") || k.contains("custom_column_3")), "{left:?}");
    assert!(cache.set_custom_column_value(1, "genre", "x").is_err());
    assert_integrity(f.root());
}

// ---- integrity, triggers, deletes ----------------------------------

#[test]
fn after_writes_the_file_is_intact_and_calibres_triggers_still_fire() {
    let (f, cache) = open();
    cache.set_custom_column_value(4, "genre", "Noir, Classic").unwrap();
    cache.set_custom_column_value(4, "saga", "New Saga [2]").unwrap();
    cache.set_custom_column_value(4, "mood", "Tense").unwrap();
    cache.set_custom_column_value(4, "stars", "5").unwrap();
    cache.set_custom_column_value(4, "pages", "99").unwrap();
    drop(cache);
    assert_integrity(f.root());

    let conn = raw(f.root());
    let link = format!("books_custom_column_{}_link", col_num(f.root(), "genre"));
    let table = format!("custom_column_{}", col_num(f.root(), "genre"));
    let err = conn.execute(&format!("INSERT INTO {link}(book, value) VALUES (99999, 1)"), []).unwrap_err();
    assert!(err.to_string().contains("Foreign key violation: book not in books"), "{err}");
    let err = conn.execute(&format!("INSERT INTO {link}(book, value) VALUES (1, 99999)"), []).unwrap_err();
    assert!(err.to_string().contains("value not in"), "{err}");
    let err = conn.execute(&format!("INSERT INTO custom_column_{}(book, value) VALUES (99999, 1)", col_num(f.root(), "pages")), []).unwrap_err();
    assert!(err.to_string().contains("Foreign key violation"), "{err}");
    // Deleting a value row takes its links with it (`fkc_delete_*`).
    conn.execute(&format!("DELETE FROM {table} WHERE value='Noir'"), []).unwrap();
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM {link} WHERE book=4")), 1);
    // The tag-browser view calibre defined still answers.
    let n: i64 = conn.query_row(&format!("SELECT count FROM tag_browser_{table} WHERE value='Classic'"), [], |r| r.get(0)).unwrap();
    assert_eq!(n, 3);
}

#[test]
fn deleting_a_book_clears_its_custom_values_and_orphaned_items() {
    let (f, cache) = open();
    cache.delete_book_keep_files(3).unwrap();
    let genre = col_num(f.root(), "genre");
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM books_custom_column_{genre}_link WHERE book=3")), 0);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{genre} WHERE value='Comic Fantasy'")), 0, "only book 3 had it");
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{} WHERE book=3", col_num(f.root(), "pages"))), 0);
    assert_eq!(count(f.root(), &format!("SELECT COUNT(*) FROM custom_column_{} WHERE value='Sad'", col_num(f.root(), "mood"))), 0);
    assert_integrity(f.root());
}

/// Writes to a normalized column while another connection checkpoints the
/// WAL the way the library's background thread does. A write that begins
/// with a read and then upgrades to a write cannot wait out the other
/// connection's lock, so it failed with "database is locked".
#[test]
fn normalized_writes_survive_a_concurrent_wal_checkpoint() {
    let (f, cache) = open();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checkpointer = {
        let (stop, root) = (stop.clone(), f.root().to_path_buf());
        std::thread::spawn(move || {
            let conn = raw(&root);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            }
        })
    };
    for i in 0..100 {
        let v = format!("Genre {}, Shared", i % 7);
        cache.set_custom_column_value(4, "genre", &v).unwrap_or_else(|e| panic!("write {i}: {e:#}"));
        cache.set_custom_column_value(4, "saga", &format!("Saga {} [{i}]", i % 3)).unwrap_or_else(|e| panic!("write {i}: {e:#}"));
        cache.set_custom_column_value(4, "stars", &(i % 11).to_string()).unwrap_or_else(|e| panic!("write {i}: {e:#}"));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    checkpointer.join().unwrap();
}

// ---- the change log ---------------------------------------------------

#[test]
fn a_rebuild_from_the_change_log_restores_every_custom_column_kind() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    let source = dir.path().join("Book.txt");
    std::fs::write(&source, b"hello").unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = "Book".into();
    meta.authors = vec!["Author".into()];
    let book = cache.add_book(&source, &meta).unwrap();
    for c in CUSTOM_COLUMNS {
        let display: serde_json::Value = serde_json::from_str(c.display).unwrap();
        cache.add_custom_column_with_display(c.label, c.name, c.datatype, c.is_multiple, &display).unwrap();
    }
    let b = &BOOKS[0];
    cache.set_custom_column_value(book, "genre", &b.custom.genre.join(", ")).unwrap();
    cache.set_custom_column_value(book, "saga", "Legendarium [4.5]").unwrap();
    cache.set_custom_column_value(book, "mood", "Happy").unwrap();
    cache.set_custom_column_value(book, "stars", "9").unwrap();
    cache.set_custom_column_value(book, "pages", "310").unwrap();
    cache.set_custom_column_value(book, "finished", "2022-01-15 00:00:00+00:00").unwrap();

    let rebuilt_dir = tempfile::tempdir().unwrap();
    for sub in ["changes", "snapshots"] {
        let from = dir.path().join(".lexmechanic").join(sub);
        let to = rebuilt_dir.path().join(".lexmechanic").join(sub);
        std::fs::create_dir_all(&to).unwrap();
        if let Ok(rd) = std::fs::read_dir(from) {
            for e in rd.flatten() {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    std::fs::copy(dir.path().join(".lexmechanic").join("install-id"), rebuilt_dir.path().join(".lexmechanic").join("install-id")).ok();
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();
    let rb = rebuilt.all_book_ids().unwrap()[0];
    for label in ["genre", "saga", "saga_index", "mood", "stars", "pages", "finished"] {
        assert_eq!(get(&rebuilt, rb, label), get(&cache, book, label), "#{label}");
    }
    assert_eq!(get(&rebuilt, rb, "saga_index").as_deref(), Some("4.5"));
    // The enumeration's allowed values came back with the column.
    assert!(rebuilt.set_custom_column_value(rb, "mood", "Angry").is_err());
    assert_eq!(schema(rebuilt_dir.path()), schema(dir.path()));
}
