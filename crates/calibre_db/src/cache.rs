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
use std::collections::HashMap;
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

    /// Real custom-column support (issue #212 follow-up), moved here
    /// from `library.rs`'s previously Cache-side-only duplicate of the
    /// same logic -- `Library::add_custom_column`/etc. now delegate to
    /// these instead of hand-rolling their own SQL, now that both
    /// share the real schema (#212). NOT a port of upstream's real
    /// `tables.py`/`fields.py` custom-column architecture (per-column
    /// `Table` subclasses bulk-loaded into memory on library open,
    /// `CustomColumns`/`initialize_custom_columns` in `backend.py`) --
    /// that's a much larger, separate rearchitecture of how this
    /// crate accesses fields at all (`Cache::field_for` already hits
    /// the DB directly per call rather than an in-memory table model,
    /// its own disclosed simplification -- see this module's docs).
    /// This is a narrower, same-shape extension of that existing
    /// per-call SQL strategy to custom columns: a `custom_columns`
    /// metadata row plus a dynamic `custom_column_N` value table per
    /// column, same as `Library`'s original implementation, just
    /// hosted on `Cache`'s connection instead of a second one.
    pub fn custom_column_label_map(&self) -> Result<HashMap<String, serde_json::Value>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized FROM custom_columns",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: i32 = row.get(0)?;
            let label: String = row.get(1)?;
            let name: String = row.get(2)?;
            let datatype: String = row.get(3)?;
            let mark_for_delete: bool = row.get(4)?;
            let editable: bool = row.get(5)?;
            let display: String = row.get(6)?;
            let is_multiple: bool = row.get(7)?;
            let normalized: bool = row.get(8)?;

            let mut map = serde_json::Map::new();
            map.insert("num".to_string(), serde_json::json!(id));
            map.insert("label".to_string(), serde_json::json!(label.clone()));
            map.insert("name".to_string(), serde_json::json!(name));
            map.insert("datatype".to_string(), serde_json::json!(datatype));
            map.insert(
                "mark_for_delete".to_string(),
                serde_json::json!(mark_for_delete),
            );
            map.insert("editable".to_string(), serde_json::json!(editable));
            map.insert(
                "display".to_string(),
                serde_json::from_str(&display).unwrap_or(serde_json::json!({})),
            );
            map.insert("is_multiple".to_string(), serde_json::json!(is_multiple));
            map.insert("normalized".to_string(), serde_json::json!(normalized));

            Ok((label, serde_json::Value::Object(map)))
        })?;

        let mut out = HashMap::new();
        for row in rows {
            let (label, data) = row?;
            out.insert(label, data);
        }
        Ok(out)
    }

    /// `datatype` is one of `bool`/`int`/`float`/`rating` (one-to-one
    /// numeric value table) or `text`/`comments`/`series` (one-to-one
    /// text value table, non-`is_multiple` only -- see the error
    /// below); anything else falls back to a generic text value table,
    /// same as `Library`'s original behavior.
    pub fn add_custom_column(
        &self,
        label: &str,
        name: &str,
        datatype: &str,
        is_multiple: bool,
    ) -> anyhow::Result<i32> {
        let mut conn = self.backend.conn.lock().unwrap();

        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = ?1",
            [label],
            |row| row.get(0),
        )?;
        if count > 0 {
            anyhow::bail!("Column with label '{}' already exists", label);
        }

        let tx = conn.transaction()?;

        tx.execute(
            "INSERT INTO custom_columns
            (label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized)
            VALUES (?1, ?2, ?3, 0, 1, '{}', ?4, 0)",
            (label, name, datatype, is_multiple),
        )?;
        let col_id = tx.last_insert_rowid() as i32;
        let table_name = format!("custom_column_{}", col_id);

        match datatype {
            "bool" | "int" | "float" | "rating" => {
                let value_type = if datatype == "float" || datatype == "rating" {
                    "REAL"
                } else {
                    "INTEGER"
                };
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value {value_type})"
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{col_id}_book ON {table_name} (book)"),
                    [],
                )?;
            }
            "text" | "comments" | "series" => {
                if is_multiple {
                    anyhow::bail!("Multiple-value text columns not yet supported in this port");
                }
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)"
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{col_id}_book ON {table_name} (book)"),
                    [],
                )?;
            }
            _ => {
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)"
                    ),
                    [],
                )?;
            }
        }

        tx.commit()?;
        Ok(col_id)
    }

    fn custom_column_lookup(&self, label: &str) -> Result<Option<(i32, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, datatype FROM custom_columns WHERE label = ?1",
            [label],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    }

    pub fn get_custom_column_value(&self, book_id: i32, label: &str) -> Result<Option<String>> {
        let Some((col_id, datatype)) = self.custom_column_lookup(label)? else {
            return Ok(None);
        };
        let table_name = format!("custom_column_{col_id}");
        let conn = self.backend.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT value FROM {table_name} WHERE book = ?1"),
            [book_id],
            |row| match datatype.as_str() {
                "int" | "bool" => row.get::<_, i32>(0).map(|v| v.to_string()),
                "float" | "rating" => row.get::<_, f64>(0).map(|v| v.to_string()),
                _ => row.get(0),
            },
        )
        .optional()
    }

    pub fn set_custom_column_value(
        &self,
        book_id: i32,
        label: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let Some((col_id, datatype)) = self.custom_column_lookup(label)? else {
            anyhow::bail!("Custom column with label '{}' not found", label);
        };
        let table_name = format!("custom_column_{col_id}");
        let conn = self.backend.conn.lock().unwrap();
        let sql = format!("INSERT OR REPLACE INTO {table_name} (book, value) VALUES (?1, ?2)");
        match datatype.as_str() {
            "bool" => conn.execute(
                &sql,
                (book_id, value.parse::<bool>().unwrap_or(false) as i32),
            ),
            "int" => conn.execute(&sql, (book_id, value.parse::<i32>().unwrap_or(0))),
            "float" | "rating" => {
                conn.execute(&sql, (book_id, value.parse::<f64>().unwrap_or(0.0)))
            }
            _ => conn.execute(&sql, (book_id, value)),
        }?;
        Ok(())
    }

    pub fn remove_custom_column(&self, label: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let col_id: Option<i32> = conn
            .query_row(
                "SELECT id FROM custom_columns WHERE label = ?1",
                [label],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = col_id else {
            anyhow::bail!("Column '{}' not found", label);
        };

        let tx = conn.transaction()?;
        tx.execute("DELETE FROM custom_columns WHERE id = ?1", [id])?;
        // `id` is an integer we control (from the `custom_columns` row
        // just looked up), not user input -- no injection risk from
        // building the table name via `format!`.
        tx.execute(&format!("DROP TABLE IF EXISTS custom_column_{id}"), [])?;
        tx.commit()?;
        Ok(())
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

    #[test]
    fn custom_column_round_trips_a_text_value() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache
            .add_custom_column("mycol", "My Column", "text", false)
            .unwrap();
        assert_eq!(cache.get_custom_column_value(id, "mycol").unwrap(), None);
        cache.set_custom_column_value(id, "mycol", "hello").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "mycol").unwrap(),
            Some("hello".to_string())
        );
    }

    #[test]
    fn custom_column_round_trips_numeric_datatypes() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.add_custom_column("n", "N", "int", false).unwrap();
        cache.set_custom_column_value(id, "n", "42").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "n").unwrap(),
            Some("42".to_string())
        );

        cache.add_custom_column("f", "F", "float", false).unwrap();
        cache.set_custom_column_value(id, "f", "3.5").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "f").unwrap(),
            Some("3.5".to_string())
        );
    }

    #[test]
    fn add_custom_column_rejects_a_duplicate_label() {
        let (_dir, cache) = open_test_cache();
        cache
            .add_custom_column("dup", "Dup", "text", false)
            .unwrap();
        assert!(cache
            .add_custom_column("dup", "Dup 2", "text", false)
            .is_err());
    }

    #[test]
    fn set_custom_column_value_errors_for_an_unknown_label() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert!(cache.set_custom_column_value(id, "nope", "x").is_err());
    }

    #[test]
    fn remove_custom_column_drops_the_column_and_its_value_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache
            .add_custom_column("temp", "Temp", "text", false)
            .unwrap();
        cache.set_custom_column_value(id, "temp", "x").unwrap();

        cache.remove_custom_column("temp").unwrap();

        assert!(cache.get_custom_column_value(id, "temp").unwrap().is_none());
        assert!(!cache
            .custom_column_label_map()
            .unwrap()
            .contains_key("temp"));
        // Re-adding the same label should work again now that it's gone.
        assert!(cache
            .add_custom_column("temp", "Temp", "text", false)
            .is_ok());
    }

    #[test]
    fn custom_column_label_map_reports_metadata_for_every_column() {
        let (_dir, cache) = open_test_cache();
        cache
            .add_custom_column("mycol", "My Column", "int", true)
            .unwrap();
        let map = cache.custom_column_label_map().unwrap();
        let entry = map.get("mycol").unwrap();
        assert_eq!(entry["name"], "My Column");
        assert_eq!(entry["datatype"], "int");
        assert_eq!(entry["is_multiple"], true);
    }
}
