//! Tests for [`crate::field_feed`]: the field snapshot stays equal to a
//! fresh load while only the touched books are re-read.

use crate::backend::Backend;
use crate::cache::Cache;

const FIELDS: [&str; 20] = [
    "title", "sort", "author_sort", "isbn", "path", "timestamp", "pubdate", "last_modified", "series_index", "comments", "uuid", "size", "series", "publisher", "rating", "tags", "languages",
    "authors", "formats", "identifiers",
];

fn new_book(cache: &Cache, title: &str) -> i32 {
    let conn = cache.backend.conn.lock().unwrap();
    conn.execute("INSERT INTO books (title) VALUES (?1)", [title]).unwrap();
    conn.last_insert_rowid() as i32
}

/// Every field of every book, read through `cache`.
fn dump(cache: &Cache, ids: &[i32]) -> Vec<(i32, &'static str, Option<String>)> {
    let mut out = Vec::new();
    for &id in ids {
        for f in FIELDS {
            out.push((id, f, cache.field_for(id, f).unwrap()));
        }
    }
    out
}

#[test]
fn a_patched_snapshot_reads_exactly_what_a_fresh_load_reads() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let cache = Cache::from_backend(backend.clone());
    let ids: Vec<i32> = (0..8).map(|i| new_book(&cache, &format!("Book {i}"))).collect();
    // Prime: from here on every change must arrive as a patch.
    dump(&cache, &ids);
    assert_eq!(cache.field_cache_full_loads(), 1);

    for (n, &id) in ids.iter().enumerate() {
        cache.set_field(id, "title", &format!("Renamed {n}")).unwrap();
        cache.set_field(id, "tags", if n % 2 == 0 { "red, blue" } else { "blue" }).unwrap();
        cache.set_field(id, "authors", if n % 3 == 0 { "Ann Author & Bob Writer" } else { "Bob Writer" }).unwrap();
        cache.set_field(id, "series", if n % 2 == 0 { "Saga" } else { "" }).unwrap();
        cache.set_field(id, "publisher", "Pub House").unwrap();
        cache.set_field(id, "comments", &format!("about {n}")).unwrap();
        // Reads between writes, as a bulk loop does.
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some(format!("Renamed {n}").as_str()));
        assert_eq!(cache.field_cache_full_loads(), 1, "after the edits to book {n}");
    }
    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'PDF', 500, 'a')", [ids[0]]).unwrap();
        conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 900, 'a')", [ids[0]]).unwrap();
        conn.execute("INSERT INTO identifiers (book, type, val) VALUES (?1, 'isbn', '123')", [ids[1]]).unwrap();
    }
    assert_eq!(cache.field_for(ids[0], "size").unwrap().as_deref(), Some("900"));
    assert_eq!(cache.field_cache_full_loads(), 1, "after raw inserts");
    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("DELETE FROM data WHERE book = ?1 AND format = 'EPUB'", [ids[0]]).unwrap();
    }
    assert_eq!(cache.field_for(ids[0], "size").unwrap().as_deref(), Some("500"));
    assert_eq!(cache.field_for(ids[0], "formats").unwrap().as_deref(), Some("PDF"));
    assert_eq!(cache.field_cache_full_loads(), 1, "after a raw delete");
    // An item table edit changes every book that has the item.
    {
        let conn = backend.conn.lock().unwrap();
        conn.execute("UPDATE tags SET name = 'crimson' WHERE name = 'red'", []).unwrap();
    }
    assert_eq!(cache.field_for(ids[0], "tags").unwrap().as_deref(), Some("crimson, blue"));
    assert_eq!(cache.field_cache_full_loads(), 1, "after an item rename");
    // A deleted book vanishes.
    cache.delete_book_keep_files(ids[2]).unwrap();
    assert_eq!(cache.field_for(ids[2], "title").unwrap(), None);

    let patched = dump(&cache, &ids);
    let fresh = dump(&Cache::from_backend(backend.clone()), &ids);
    assert_eq!(patched, fresh);
    assert_eq!(cache.field_cache_full_loads(), 1, "every change should have been a patch");
}

#[test]
fn a_write_by_another_connection_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    let id = new_book(&cache, "Before");
    assert_eq!(cache.field_for(id, "comments").unwrap(), None);
    let other = rusqlite::Connection::open(dir.path().join("metadata.db")).unwrap();
    other.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    other.execute("INSERT INTO comments (book, text) VALUES (?1, 'After')", [id]).unwrap();
    drop(other);
    assert_eq!(cache.field_for(id, "comments").unwrap().as_deref(), Some("After"));
    assert_eq!(cache.field_cache_full_loads(), 2);
}

#[test]
fn an_idle_snapshot_is_not_reloaded() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    let id = new_book(&cache, "Steady");
    for _ in 0..20 {
        cache.field_for(id, "title").unwrap();
    }
    // The background checkpoint thread opens its own connection; that must
    // not look like somebody else's write.
    assert_eq!(cache.field_cache_full_loads(), 1);
}
