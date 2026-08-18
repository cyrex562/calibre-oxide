//! Port of `old_src/src/calibre/db/cache.py`'s `Cache` (issue #204).
//!
//! # Scope of this pass
//!
//! `cache.py`'s real `Cache` is calibre's single largest class (~3,700
//! lines, 200+ methods): field access, writes, custom columns, notes,
//! FTS, virtual libraries, saved searches, categories, trash, format/
//! cover storage, dump/restore, and more. This pass ports the one
//! piece everything else in the class (and this crate's `search.rs`/
//! `view.rs`/`write.rs`) actually depends on: real field access for
//! the *standard* (non-custom-column) fields, plus real book-id
//! enumeration and real preference access. Real, verified against
//! upstream schema/semantics:
//!
//! - [`Cache::all_book_ids`]: `SELECT id FROM books` -- previously
//!   nonexistent; `search.rs` had a comment reading "let's assume
//!   valid IDs are 1..100" in its place.
//! - [`Cache::field_for`]: resolves every *standard* field
//!   (`id`, `title`, `sort`, `author_sort`, `isbn`, `path`, `uuid`,
//!   `series_index`, `timestamp`, `pubdate`, `last_modified`,
//!   `comments`, `series`, `publisher`, `rating`, `authors`, `tags`,
//!   `languages`, `formats`, `identifiers`, `size`) via the real
//!   schema -- not the narrow 7-column passthrough whitelist
//!   `Backend::field_for` has (which this leaves untouched; it's used
//!   directly by a few other modules for exactly those 7 columns and
//!   isn't part of `cache.py` upstream at all -- `DB`/`Backend` has no
//!   `field_for` in real calibre, that concept only exists on `Cache`).
//! - [`Cache::pref`]/[`Cache::set_pref`]: real, including namespaced
//!   keys, delegating to `Backend`'s JSON preference storage from
//!   #203.
//!
//! # A real, disclosed simplification: string-joined multi-value fields
//!
//! Upstream's `field_for` returns real typed Python values: a tuple of
//! item names (in book-link order) for `is_multiple` fields like
//! `authors`/`tags`/`languages`/`formats`, and a `dict[type, val]` for
//! `identifiers`. This crate's pre-existing `field_for` contract
//! (`Cache::field_for`/`Backend::field_for`, used throughout
//! `backup.rs`/`covers.rs`/`lazy.rs`/etc.) returns `Option<String>` --
//! changing that to a typed enum is a real, separate refactor with a
//! wide blast radius across the crate, not part of this pass. Instead,
//! multi-value fields here return a joined string (`" & "` for
//! authors, matching `author_sort_from_authors`'s separator; `", "`
//! for tags/languages/formats; `"type:val,type:val"` for identifiers).
//! This is a correct, useful value for display/search purposes but is
//! *not* upstream's tuple/dict shape -- callers that need the real
//! per-item structure (e.g. to add/remove one tag) need a typed API
//! this pass doesn't add.
//!
//! # Not ported
//!
//! Everything else: `set_field`/writes beyond what `write.rs` already
//! has, custom columns, notes, FTS, composite fields, virtual
//! libraries, saved searches, categories, trash, cover/format storage
//! beyond what `covers.rs`/`add_format` already do, dump/restore,
//! `move_library_to`. Each is its own follow-up.

use crate::backend::Backend;
use rusqlite::{OptionalExtension, Result};
use std::path::Path;

pub struct Cache {
    pub backend: Backend,
}

impl Cache {
    pub fn new<P: AsRef<Path>>(library_path: P) -> Result<Self> {
        let backend = Backend::new(library_path)?;
        Ok(Cache { backend })
    }

    pub fn library_id(&self) -> String {
        self.backend.library_id().unwrap_or_default()
    }

    /// Port of `Cache.all_book_ids`. Previously nonexistent -- callers
    /// (namely `search.rs`) worked around its absence by assuming book
    /// ids fell in some hardcoded range.
    pub fn all_book_ids(&self) -> Result<Vec<i32>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM books ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Port of `Cache.pref` (without the `default` param collapsing --
    /// callers get `None` for an unset key and decide their own
    /// default, which is equivalent).
    pub fn pref(&self, name: &str, namespace: Option<&str>) -> Option<serde_json::Value> {
        match namespace {
            Some(ns) => self.backend.get_pref(&format!("namespaced:{ns}:{name}")),
            None => self.backend.get_pref(name),
        }
    }

    /// Port of `Cache.set_pref`. The search-cache/dynamic-category
    /// invalidation upstream does for specific preference names
    /// (`grouped_search_terms`, `virtual_libraries`, ...) is not
    /// ported -- this crate doesn't have those caches yet.
    pub fn set_pref(
        &self,
        name: &str,
        val: &serde_json::Value,
        namespace: Option<&str>,
    ) -> Result<()> {
        match namespace {
            Some(ns) => self
                .backend
                .set_pref(&format!("namespaced:{ns}:{name}"), val),
            None => self.backend.set_pref(name, val),
        }
    }

    /// Port of `Cache.field_for` for the standard (non-custom-column)
    /// fields. See the module docs for the multi-value-field string-
    /// join simplification and what's not covered.
    pub fn field_for(&self, book_id: i32, field_name: &str) -> Result<Option<String>> {
        let conn = self.backend.conn.lock().unwrap();

        match field_name {
            // `id` is INTEGER, not TEXT like the rest of this arm --
            // `row.get::<_, String>(0)` would fail to convert it
            // (rusqlite's `FromSql` for `String` only accepts SQLite
            // TEXT), so it needs its own arm that fetches an `i64`
            // first and formats it.
            "id" => conn
                .query_row("SELECT id FROM books WHERE id = ?", [book_id], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .map(|v| v.map(|n| n.to_string())),
            "title" | "sort" | "author_sort" | "isbn" | "path" | "uuid" | "timestamp"
            | "pubdate" | "last_modified" => {
                let sql = format!("SELECT {field_name} FROM books WHERE id = ?");
                conn.query_row(&sql, [book_id], |row| row.get(0)).optional()
            }
            "series_index" => conn
                .query_row(
                    "SELECT series_index FROM books WHERE id = ?",
                    [book_id],
                    |row| row.get::<_, f64>(0),
                )
                .optional()
                .map(|v| v.map(|n| n.to_string())),
            "comments" => conn
                .query_row(
                    "SELECT text FROM comments WHERE book = ?",
                    [book_id],
                    |row| row.get(0),
                )
                .optional(),
            "size" => conn
                .query_row(
                    "SELECT MAX(uncompressed_size) FROM data WHERE book = ?",
                    [book_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()
                .map(|v| v.flatten().map(|n| n.to_string())),
            "series" => conn
                .query_row(
                    "SELECT series.name FROM books_series_link \
                     JOIN series ON series.id = books_series_link.series \
                     WHERE books_series_link.book = ?",
                    [book_id],
                    |row| row.get(0),
                )
                .optional(),
            "publisher" => conn
                .query_row(
                    "SELECT publishers.name FROM books_publishers_link \
                     JOIN publishers ON publishers.id = books_publishers_link.publisher \
                     WHERE books_publishers_link.book = ?",
                    [book_id],
                    |row| row.get(0),
                )
                .optional(),
            "rating" => conn
                .query_row(
                    "SELECT ratings.rating FROM books_ratings_link \
                     JOIN ratings ON ratings.id = books_ratings_link.rating \
                     WHERE books_ratings_link.book = ?",
                    [book_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map(|v| v.map(|n| n.to_string())),
            "authors" => join_many_to_many(
                &conn,
                "SELECT authors.name FROM books_authors_link \
                 JOIN authors ON authors.id = books_authors_link.author \
                 WHERE books_authors_link.book = ? ORDER BY books_authors_link.id",
                book_id,
                " & ",
            ),
            "tags" => join_many_to_many(
                &conn,
                "SELECT tags.name FROM books_tags_link \
                 JOIN tags ON tags.id = books_tags_link.tag \
                 WHERE books_tags_link.book = ? ORDER BY books_tags_link.id",
                book_id,
                ", ",
            ),
            "languages" => join_many_to_many(
                &conn,
                "SELECT languages.lang_code FROM books_languages_link \
                 JOIN languages ON languages.id = books_languages_link.lang_code \
                 WHERE books_languages_link.book = ? ORDER BY books_languages_link.item_order",
                book_id,
                ", ",
            ),
            "formats" => join_many_to_many(
                &conn,
                "SELECT format FROM data WHERE book = ? ORDER BY id",
                book_id,
                ", ",
            ),
            "identifiers" => {
                let mut stmt =
                    conn.prepare("SELECT type, val FROM identifiers WHERE book = ? ORDER BY id")?;
                let pairs: Vec<(String, String)> = stmt
                    .query_map([book_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<_>>()?;
                if pairs.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(
                        pairs
                            .into_iter()
                            .map(|(t, v)| format!("{t}:{v}"))
                            .collect::<Vec<_>>()
                            .join(","),
                    ))
                }
            }
            // Every field name `Backend::field_for` recognizes is
            // already handled above (its whitelist -- title, sort,
            // author_sort, isbn, path, series_index, uuid -- is a
            // strict subset of this match), so there is nothing left
            // to delegate to it: an unrecognized name here is
            // genuinely unrecognized. (Also: `self.backend.conn` is
            // already locked in this scope; calling
            // `self.backend.field_for` would try to lock it again and
            // deadlock, since `std::sync::Mutex` isn't reentrant.)
            _ => Ok(None),
        }
    }

    pub fn update_memory(&mut self, _book_id: i32, _field: &str, _value: &str) {
        // Placeholder for future in-memory cache invalidation.
        // Currently, field_for hits the DB directly so no cache to clear.
    }
}

/// Runs a many-to-many field's SELECT (already scoped to one book,
/// already ordered) and joins the results with `sep`, matching the
/// "is_multiple fields always return a value, `default_value` is
/// ignored" rule from `field_for`'s docstring -- an empty result is
/// `None` here (this crate's `Option<String>` contract), not upstream's
/// empty tuple, but the "no default substitution" behavior is the same.
fn join_many_to_many(
    conn: &rusqlite::Connection,
    sql: &str,
    book_id: i32,
    sep: &str,
) -> Result<Option<String>> {
    let mut stmt = conn.prepare(sql)?;
    let values: Vec<String> = stmt
        .query_map([book_id], |row| row.get(0))?
        .collect::<Result<_>>()?;
    if values.is_empty() {
        Ok(None)
    } else {
        Ok(Some(values.join(sep)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    fn insert_book(cache: &Cache, title: &str) -> i32 {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES (?1)", [title])
            .unwrap();
        conn.last_insert_rowid() as i32
    }

    #[test]
    fn all_book_ids_returns_every_book_in_id_order() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.all_book_ids().unwrap(), Vec::<i32>::new());
        let id1 = insert_book(&cache, "One");
        let id2 = insert_book(&cache, "Two");
        assert_eq!(cache.all_book_ids().unwrap(), vec![id1, id2]);
    }

    #[test]
    fn field_for_reads_scalar_book_columns() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "My Title");
        assert_eq!(
            cache.field_for(id, "title").unwrap(),
            Some("My Title".to_string())
        );
    }

    #[test]
    fn field_for_reads_the_integer_id_column_without_a_type_mismatch() {
        // Regression test: `id` is INTEGER, not TEXT like most of the
        // `books` columns `field_for` handles in the same match arm --
        // fetching it via `row.get::<_, String>(0)` (rusqlite's
        // `FromSql` for `String` only accepts SQLite TEXT) used to
        // return an `InvalidColumnType` error that `view.rs::sort`'s
        // `.ok().flatten()` silently swallowed into "no value",
        // breaking `sort("id", ...)` without ever surfacing an error.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "id").unwrap(), Some(id.to_string()));
    }

    #[test]
    fn field_for_reads_comments_from_the_comments_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "comments").unwrap(), None);
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO comments (book, text) VALUES (?1, ?2)",
                (id, "Some comment text"),
            )
            .unwrap();
        }
        assert_eq!(
            cache.field_for(id, "comments").unwrap(),
            Some("Some comment text".to_string())
        );
    }

    #[test]
    fn field_for_reads_authors_joined_with_ampersand_in_link_order() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO authors (name) VALUES ('Bob')", [])
            .unwrap();
        let bob_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO authors (name) VALUES ('Alice')", [])
            .unwrap();
        let alice_id = conn.last_insert_rowid();
        // Link Bob first, then Alice -- field order must follow link
        // (insertion) order, not alphabetical.
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (id, bob_id),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (id, alice_id),
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "authors").unwrap(),
            Some("Bob & Alice".to_string())
        );
    }

    #[test]
    fn field_for_reads_tags_joined_with_comma() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "tags").unwrap(), None);
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO tags (name) VALUES ('fiction')", [])
            .unwrap();
        let t1 = conn.last_insert_rowid();
        conn.execute("INSERT INTO tags (name) VALUES ('drama')", [])
            .unwrap();
        let t2 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
            (id, t1),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
            (id, t2),
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "tags").unwrap(),
            Some("fiction, drama".to_string())
        );
    }

    #[test]
    fn field_for_reads_series_and_series_index() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO series (name) VALUES ('The Trilogy')", [])
            .unwrap();
        let series_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO books_series_link (book, series) VALUES (?1, ?2)",
            (id, series_id),
        )
        .unwrap();
        conn.execute("UPDATE books SET series_index = 2.0 WHERE id = ?1", [id])
            .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "series").unwrap(),
            Some("The Trilogy".to_string())
        );
        assert_eq!(
            cache.field_for(id, "series_index").unwrap(),
            Some("2".to_string())
        );
    }

    #[test]
    fn field_for_reads_identifiers_as_type_val_pairs() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO identifiers (book, type, val) VALUES (?1, 'isbn', '12345')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO identifiers (book, type, val) VALUES (?1, 'doi', 'abc')",
            [id],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "identifiers").unwrap(),
            Some("isbn:12345,doi:abc".to_string())
        );
    }

    #[test]
    fn field_for_reads_formats_from_the_data_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 100, 'book')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'MOBI', 200, 'book')",
            [id],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "formats").unwrap(),
            Some("EPUB, MOBI".to_string())
        );
        assert_eq!(
            cache.field_for(id, "size").unwrap(),
            Some("200".to_string())
        );
    }

    #[test]
    fn pref_and_set_pref_round_trip_including_namespaced_keys() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.pref("k", None), None);
        cache.set_pref("k", &serde_json::json!("v"), None).unwrap();
        assert_eq!(cache.pref("k", None), Some(serde_json::json!("v")));

        assert_eq!(cache.pref("k", Some("ns")), None);
        cache
            .set_pref("k", &serde_json::json!(42), Some("ns"))
            .unwrap();
        assert_eq!(cache.pref("k", Some("ns")), Some(serde_json::json!(42)));
        // Namespaced and un-namespaced keys of the same name are distinct.
        assert_eq!(cache.pref("k", None), Some(serde_json::json!("v")));
    }

    #[test]
    fn field_for_returns_none_for_an_unrecognized_field_without_deadlocking() {
        // Regression test: the fallback arm used to call
        // `self.backend.field_for(...)`, which tries to lock
        // `self.backend.conn` a second time -- but this function
        // already holds that lock for its whole body, and
        // `std::sync::Mutex` isn't reentrant, so that was a guaranteed
        // self-deadlock on any name not handled by an earlier arm
        // (which is every name, since `Backend::field_for`'s whitelist
        // is a strict subset of this function's). If this test hangs
        // instead of returning, the deadlock is back.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "not_a_real_field").unwrap(), None);
    }
}
