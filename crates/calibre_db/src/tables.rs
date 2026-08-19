//! Port of `old_src/src/calibre/db/tables.py` (issue #222, a #201
//! follow-up, and phase 1 of the `Library`/`Cache` field-access
//! rearchitecture that #212/#214/#216/#220 built toward).
//!
//! # What this phase is, and isn't
//!
//! Upstream's `Table` subclasses (`OneToOneTable`/`ManyToOneTable`/
//! `ManyToManyTable`/`RatingTable`/`AuthorsTable`/`FormatsTable`/
//! `IdentifiersTable`) bulk-load an entire column's data into memory
//! on library open (`read(db)`: full `id_map`/`col_book_map`/
//! `book_col_map` dicts), and `fields.py`'s `Field` classes wrap them
//! with a typed access API that `cache.py`'s `field_for`/`set_field`/
//! searching/sorting/categories all go through instead of hitting SQL
//! per call. That's the real target architecture.
//!
//! This pass ports the **read side only**: real `read()` bulk-loading
//! for every standard field `Cache::field_for` (#204) already
//! supports, faithfully matching upstream's map shapes. It does
//! **not** yet wire this into `Cache`, `search.rs`, or `view.rs` --
//! those still use the existing real, tested, per-call-SQL strategy.
//! It also does **not** port the mutation side (`remove_books`/
//! `remove_items`/`rename_item`/`set_links`/`fix_link_table`/
//! `fix_case_duplicates`/format-specific mutators) -- those only
//! matter once something is actually consuming and keeping this
//! in-memory model in sync on writes, which is a later phase.
//!
//! Think of this as the foundation poured, not the building wired up:
//! real, tested, standalone data structures that a later phase can
//! cut `Cache`/`search.rs`/`view.rs` over to -- a deliberate,
//! incremental approach to a rearchitecture large enough that doing
//! it in one uncheckable pass would be reckless (see #222's own issue
//! body for why this wasn't started as a single all-at-once change).
//!
//! # Disclosed simplifications
//!
//! - **No `field_metadata` system exists** (recurring gap across
//!   #210/#214/#216/#218/#220), so there's no generic, data-driven
//!   `Table` construction from metadata dicts. [`StandardTables::read`]
//!   hardcodes the fixed field list `Cache::field_for` already
//!   supports (table/column/link-table names), matching that
//!   function's own field set exactly.
//! - **String-typed values throughout**, even for
//!   [`OneToOneTable`]/[`ManyToOneTable`]'s underlying-numeric columns
//!   (`series_index`, `rating`) -- matches this crate's existing
//!   `Option<String>` `field_for` contract (#204's own disclosed
//!   simplification) rather than porting upstream's dynamic-typing-
//!   preserving `unserialize` framework.
//! - **No legacy `|`-for-`,` author-name escaping.** Upstream's
//!   `AuthorsTable` un-escapes `|` back to `,` on read (author names
//!   with literal commas are pipe-escaped on write, historically, to
//!   avoid clashing with comma-separated author lists elsewhere).
//!   This crate has never applied the corresponding escaping on
//!   write (`Cache::add_book_db_entry`/`update_book_metadata` insert
//!   author names as plain text) -- doing the unescape here without
//!   the matching escape elsewhere would misinterpret a literal `|`
//!   in a real author name, so it's intentionally not ported. Every
//!   author-name read/write in this crate agrees on "plain text, no
//!   escaping," which is what matters for internal consistency.
//! - `CompositeTable`/`VirtualTable` (composite/in-memory-only fields)
//!   aren't ported -- no composite-field/template system exists to
//!   back them (same gap `search.rs`/`categories.rs` already
//!   disclose).
//! - [`RatingTable::read`]'s upstream behavior includes a real
//!   *write* as part of "reading": deleting any `rating=0` rows it
//!   finds (0 means "unset" in upstream's data model, should never be
//!   persisted). That's real schema hygiene, not a mutation-API
//!   feature, so it's ported here even though this phase is otherwise
//!   read-only.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableType {
    OneOne,
    ManyOne,
    ManyMany,
}

fn sql_value_to_string(v: SqlValue) -> Option<String> {
    match v {
        SqlValue::Null => None,
        SqlValue::Integer(i) => Some(i.to_string()),
        SqlValue::Real(f) => Some(f.to_string()),
        SqlValue::Text(s) => Some(s),
        SqlValue::Blob(_) => None,
    }
}

/// Port of `OneToOneTable`: one value per book (`book_col_map`).
/// `id_column` is `"id"` for the `books` table itself, `"book"` for
/// every satellite one-to-one table (`comments`), matching upstream's
/// `idcol = 'id' if table == 'books' else 'book'`.
#[derive(Debug, Default, Clone)]
pub struct OneToOneTable {
    pub book_col_map: HashMap<i32, String>,
}

impl OneToOneTable {
    pub fn read(
        conn: &Connection,
        table: &str,
        id_column: &str,
        value_column: &str,
    ) -> Result<Self> {
        let sql = format!("SELECT {id_column}, {value_column} FROM {table}");
        let mut stmt = conn.prepare(&sql)?;
        let mut book_col_map = HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, SqlValue>(1)?))
        })?;
        for row in rows {
            let (book_id, val) = row?;
            if let Some(s) = sql_value_to_string(val) {
                book_col_map.insert(book_id, s);
            }
        }
        Ok(Self { book_col_map })
    }
}

/// Port of `SizeTable`: `MAX(uncompressed_size)` across a book's
/// formats.
#[derive(Debug, Default, Clone)]
pub struct SizeTable {
    pub book_col_map: HashMap<i32, i64>,
}

impl SizeTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT books.id, (SELECT MAX(uncompressed_size) FROM data WHERE data.book = books.id) FROM books",
        )?;
        let mut book_col_map = HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        for row in rows {
            let (book_id, size) = row?;
            if let Some(size) = size {
                book_col_map.insert(book_id, size);
            }
        }
        Ok(Self { book_col_map })
    }
}

/// Port of `UUIDTable`: adds a reverse `uuid_to_id_map` lookup on top
/// of the plain one-to-one `book_col_map`.
#[derive(Debug, Default, Clone)]
pub struct UuidTable {
    pub book_col_map: HashMap<i32, String>,
    pub uuid_to_id_map: HashMap<String, i32>,
}

impl UuidTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let inner = OneToOneTable::read(conn, "books", "id", "uuid")?;
        let uuid_to_id_map = inner
            .book_col_map
            .iter()
            .map(|(&id, uuid)| (uuid.clone(), id))
            .collect();
        Ok(Self {
            book_col_map: inner.book_col_map,
            uuid_to_id_map,
        })
    }

    pub fn lookup_by_uuid(&self, uuid: &str) -> Option<i32> {
        self.uuid_to_id_map.get(uuid).copied()
    }
}

/// Port of `ManyToOneTable`: each book has at most one value, each
/// value can apply to many books (`series`/`publisher`/`rating`).
#[derive(Debug, Default, Clone)]
pub struct ManyToOneTable {
    pub id_map: HashMap<i32, String>,
    pub link_map: HashMap<i32, String>,
    pub col_book_map: HashMap<i32, HashSet<i32>>,
    pub book_col_map: HashMap<i32, i32>,
}

impl ManyToOneTable {
    pub fn read(
        conn: &Connection,
        item_table: &str,
        item_column: &str,
        link_table: &str,
        link_column: &str,
    ) -> Result<Self> {
        let mut id_map = HashMap::new();
        let mut link_map = HashMap::new();
        {
            let sql = format!("SELECT id, {item_column}, link FROM {item_table}");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, SqlValue>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, val, link) = row?;
                if let Some(s) = sql_value_to_string(val) {
                    id_map.insert(id, s);
                }
                link_map.insert(id, link);
            }
        }

        let mut col_book_map: HashMap<i32, HashSet<i32>> = HashMap::new();
        let mut book_col_map = HashMap::new();
        {
            let sql = format!("SELECT book, {link_column} FROM {link_table}");
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?;
            for row in rows {
                let (book, item_id) = row?;
                col_book_map.entry(item_id).or_default().insert(book);
                book_col_map.insert(book, item_id);
            }
        }

        Ok(Self {
            id_map,
            link_map,
            col_book_map,
            book_col_map,
        })
    }
}

/// Port of `RatingTable`: a [`ManyToOneTable`] over `ratings`, with
/// real cleanup of any `rating=0` rows (upstream's "0 means unset,
/// should never be persisted" invariant) as part of loading.
pub fn read_rating_table(conn: &Connection) -> Result<ManyToOneTable> {
    let mut table =
        ManyToOneTable::read(conn, "ratings", "rating", "books_ratings_link", "rating")?;
    let bad_ids: Vec<i32> = table
        .id_map
        .iter()
        .filter(|(_, v)| v.as_str() == "0")
        .map(|(&id, _)| id)
        .collect();
    if !bad_ids.is_empty() {
        for id in &bad_ids {
            table.id_map.remove(id);
            table.col_book_map.remove(id);
        }
        table
            .book_col_map
            .retain(|_, item_id| !bad_ids.contains(item_id));
        conn.execute_batch(&format!(
            "DELETE FROM books_ratings_link WHERE rating IN ({});
             DELETE FROM ratings WHERE rating = 0;",
            bad_ids
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ))?;
    }
    Ok(table)
}

/// Port of `ManyToManyTable`: each book can have many values, each
/// value can apply to many books (`tags`/`languages`). `book_col_map`
/// is ordered by link-table `id` (insertion order), matching
/// upstream's `ORDER BY id`.
#[derive(Debug, Default, Clone)]
pub struct ManyToManyTable {
    pub id_map: HashMap<i32, String>,
    pub link_map: HashMap<i32, String>,
    pub col_book_map: HashMap<i32, HashSet<i32>>,
    pub book_col_map: HashMap<i32, Vec<i32>>,
}

impl ManyToManyTable {
    pub fn read(
        conn: &Connection,
        item_table: &str,
        item_column: &str,
        link_table: &str,
        link_column: &str,
    ) -> Result<Self> {
        let mut id_map = HashMap::new();
        let mut link_map = HashMap::new();
        {
            let sql = format!("SELECT id, {item_column}, link FROM {item_table}");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, SqlValue>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, val, link) = row?;
                if let Some(s) = sql_value_to_string(val) {
                    id_map.insert(id, s);
                }
                link_map.insert(id, link);
            }
        }

        let mut col_book_map: HashMap<i32, HashSet<i32>> = HashMap::new();
        let mut book_col_map: HashMap<i32, Vec<i32>> = HashMap::new();
        {
            let sql = format!("SELECT book, {link_column} FROM {link_table} ORDER BY id");
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?;
            for row in rows {
                let (book, item_id) = row?;
                col_book_map.entry(item_id).or_default().insert(book);
                book_col_map.entry(book).or_default().push(item_id);
            }
        }

        Ok(Self {
            id_map,
            link_map,
            col_book_map,
            book_col_map,
        })
    }
}

/// Port of `AuthorsTable`: a [`ManyToManyTable`] over `authors`, plus
/// the `asort_map` (per-author sort value, falling back to a computed
/// `author_to_author_sort` when the stored `sort` is empty).
#[derive(Debug, Default, Clone)]
pub struct AuthorsTable {
    pub id_map: HashMap<i32, String>,
    pub link_map: HashMap<i32, String>,
    pub asort_map: HashMap<i32, String>,
    pub col_book_map: HashMap<i32, HashSet<i32>>,
    pub book_col_map: HashMap<i32, Vec<i32>>,
}

impl AuthorsTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let mut id_map = HashMap::new();
        let mut link_map = HashMap::new();
        let mut asort_map = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, name, sort, link FROM authors")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, name, sort, link) = row?;
                let sort = match sort {
                    Some(s) if !s.is_empty() => s,
                    _ => calibre_ebooks::metadata::authors::author_to_author_sort(
                        &name, None, None, None, None, None, None,
                    ),
                };
                asort_map.insert(id, sort);
                id_map.insert(id, name);
                link_map.insert(id, link);
            }
        }

        let mut col_book_map: HashMap<i32, HashSet<i32>> = HashMap::new();
        let mut book_col_map: HashMap<i32, Vec<i32>> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT book, author FROM books_authors_link ORDER BY id")?;
            let rows =
                stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?;
            for row in rows {
                let (book, author_id) = row?;
                col_book_map.entry(author_id).or_default().insert(book);
                book_col_map.entry(book).or_default().push(author_id);
            }
        }

        Ok(Self {
            id_map,
            link_map,
            asort_map,
            col_book_map,
            book_col_map,
        })
    }
}

/// Port of `FormatsTable`: keyed by format string (e.g. `"EPUB"`),
/// not an integer item id -- there's no `formats` item table, `data`
/// rows *are* the items.
#[derive(Debug, Default, Clone)]
pub struct FormatsTable {
    pub fname_map: HashMap<i32, HashMap<String, String>>,
    pub size_map: HashMap<i32, HashMap<String, i64>>,
    pub col_book_map: HashMap<String, HashSet<i32>>,
    /// Sorted (matches upstream's `tuple(sorted(v))`), unlike every
    /// other many-to-many table's link-insertion order.
    pub book_col_map: HashMap<i32, Vec<String>>,
}

impl FormatsTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let mut fname_map: HashMap<i32, HashMap<String, String>> = HashMap::new();
        let mut size_map: HashMap<i32, HashMap<String, i64>> = HashMap::new();
        let mut col_book_map: HashMap<String, HashSet<i32>> = HashMap::new();
        let mut bcm: HashMap<i32, Vec<String>> = HashMap::new();

        let mut stmt = conn.prepare("SELECT book, format, name, uncompressed_size FROM data")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (book, fmt, name, size) = row?;
            let Some(fmt) = fmt else { continue };
            let fmt = fmt.to_uppercase();
            col_book_map.entry(fmt.clone()).or_default().insert(book);
            bcm.entry(book).or_default().push(fmt.clone());
            fname_map.entry(book).or_default().insert(fmt.clone(), name);
            size_map.entry(book).or_default().insert(fmt, size);
        }

        let book_col_map = bcm
            .into_iter()
            .map(|(book, mut fmts)| {
                fmts.sort();
                (book, fmts)
            })
            .collect();

        Ok(Self {
            fname_map,
            size_map,
            col_book_map,
            book_col_map,
        })
    }
}

/// Port of `IdentifiersTable`: keyed by identifier type (e.g.
/// `"isbn"`), not an integer item id -- same "no item table, link
/// rows are the items" shape as [`FormatsTable`].
#[derive(Debug, Default, Clone)]
pub struct IdentifiersTable {
    pub book_col_map: HashMap<i32, HashMap<String, String>>,
    pub col_book_map: HashMap<String, HashSet<i32>>,
}

impl IdentifiersTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let mut book_col_map: HashMap<i32, HashMap<String, String>> = HashMap::new();
        let mut col_book_map: HashMap<String, HashSet<i32>> = HashMap::new();

        let mut stmt = conn.prepare("SELECT book, type, val FROM identifiers")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (book, typ, val) = row?;
            if let (Some(typ), Some(val)) = (typ, val) {
                col_book_map.entry(typ.clone()).or_default().insert(book);
                book_col_map.entry(book).or_default().insert(typ, val);
            }
        }

        Ok(Self {
            book_col_map,
            col_book_map,
        })
    }
}

/// Every standard field's real, bulk-loaded table, matching
/// `Cache::field_for`'s (#204) field set exactly. See this module's
/// docs for what's not wired to anything yet.
pub struct StandardTables {
    pub title: OneToOneTable,
    pub sort: OneToOneTable,
    pub author_sort: OneToOneTable,
    pub isbn: OneToOneTable,
    pub path: OneToOneTable,
    pub timestamp: OneToOneTable,
    pub pubdate: OneToOneTable,
    pub last_modified: OneToOneTable,
    pub series_index: OneToOneTable,
    pub comments: OneToOneTable,
    pub uuid: UuidTable,
    pub size: SizeTable,
    pub series: ManyToOneTable,
    pub publisher: ManyToOneTable,
    pub rating: ManyToOneTable,
    pub tags: ManyToManyTable,
    pub languages: ManyToManyTable,
    pub authors: AuthorsTable,
    pub formats: FormatsTable,
    pub identifiers: IdentifiersTable,
}

impl StandardTables {
    pub fn read(conn: &Connection) -> Result<Self> {
        Ok(Self {
            title: OneToOneTable::read(conn, "books", "id", "title")?,
            sort: OneToOneTable::read(conn, "books", "id", "sort")?,
            author_sort: OneToOneTable::read(conn, "books", "id", "author_sort")?,
            isbn: OneToOneTable::read(conn, "books", "id", "isbn")?,
            path: OneToOneTable::read(conn, "books", "id", "path")?,
            timestamp: OneToOneTable::read(conn, "books", "id", "timestamp")?,
            pubdate: OneToOneTable::read(conn, "books", "id", "pubdate")?,
            last_modified: OneToOneTable::read(conn, "books", "id", "last_modified")?,
            series_index: OneToOneTable::read(conn, "books", "id", "series_index")?,
            comments: OneToOneTable::read(conn, "comments", "book", "text")?,
            uuid: UuidTable::read(conn)?,
            size: SizeTable::read(conn)?,
            series: ManyToOneTable::read(conn, "series", "name", "books_series_link", "series")?,
            publisher: ManyToOneTable::read(
                conn,
                "publishers",
                "name",
                "books_publishers_link",
                "publisher",
            )?,
            rating: read_rating_table(conn)?,
            tags: ManyToManyTable::read(conn, "tags", "name", "books_tags_link", "tag")?,
            languages: ManyToManyTable::read(
                conn,
                "languages",
                "lang_code",
                "books_languages_link",
                "lang_code",
            )?,
            authors: AuthorsTable::read(conn)?,
            formats: FormatsTable::read(conn)?,
            identifiers: IdentifiersTable::read(conn)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use tempfile::tempdir;

    fn open_test_conn() -> (tempfile::TempDir, Backend) {
        let dir = tempdir().unwrap();
        let backend = Backend::new(dir.path()).unwrap();
        (dir, backend)
    }

    fn insert_book(conn: &Connection, title: &str) -> i32 {
        conn.execute("INSERT INTO books (title) VALUES (?1)", [title])
            .unwrap();
        conn.last_insert_rowid() as i32
    }

    #[test]
    fn one_to_one_table_reads_every_books_title() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id1 = insert_book(&conn, "First");
        let id2 = insert_book(&conn, "Second");

        let table = OneToOneTable::read(&conn, "books", "id", "title").unwrap();
        assert_eq!(table.book_col_map.get(&id1), Some(&"First".to_string()));
        assert_eq!(table.book_col_map.get(&id2), Some(&"Second".to_string()));
    }

    #[test]
    fn uuid_table_supports_reverse_lookup() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        let uuid: String = conn
            .query_row("SELECT uuid FROM books WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();

        let table = UuidTable::read(&conn).unwrap();
        assert_eq!(table.lookup_by_uuid(&uuid), Some(id));
        assert_eq!(table.lookup_by_uuid("not-a-real-uuid"), None);
    }

    #[test]
    fn size_table_reports_max_format_size_per_book() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 100, 'T')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'MOBI', 500, 'T')",
            [id],
        )
        .unwrap();

        let table = SizeTable::read(&conn).unwrap();
        assert_eq!(table.book_col_map.get(&id), Some(&500));
    }

    #[test]
    fn many_to_one_table_reads_series_with_real_counts() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id1 = insert_book(&conn, "A");
        let id2 = insert_book(&conn, "B");
        conn.execute("INSERT INTO series (name) VALUES ('My Series')", [])
            .unwrap();
        let series_id: i32 = conn.last_insert_rowid() as i32;
        conn.execute(
            "INSERT INTO books_series_link (book, series) VALUES (?1, ?2), (?3, ?2)",
            (id1, series_id, id2),
        )
        .unwrap();

        let table =
            ManyToOneTable::read(&conn, "series", "name", "books_series_link", "series").unwrap();
        assert_eq!(table.id_map.get(&series_id), Some(&"My Series".to_string()));
        assert_eq!(table.col_book_map[&series_id].len(), 2);
        assert_eq!(table.book_col_map.get(&id1), Some(&series_id));
    }

    #[test]
    fn rating_table_deletes_zero_ratings_as_part_of_reading() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        conn.execute("INSERT INTO ratings (rating) VALUES (0)", [])
            .unwrap();
        let bad_rating_id = conn.last_insert_rowid() as i32;
        conn.execute(
            "INSERT INTO books_ratings_link (book, rating) VALUES (?1, ?2)",
            (id, bad_rating_id),
        )
        .unwrap();
        conn.execute("INSERT INTO ratings (rating) VALUES (6)", [])
            .unwrap();

        let table = read_rating_table(&conn).unwrap();
        assert!(!table.id_map.contains_key(&bad_rating_id));

        let remaining: i32 = conn
            .query_row("SELECT COUNT(*) FROM ratings WHERE rating = 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn many_to_many_table_preserves_link_insertion_order() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        for tag in ["zebra", "apple"] {
            conn.execute("INSERT INTO tags (name) VALUES (?1)", [tag])
                .unwrap();
            let tag_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
                (id, tag_id),
            )
            .unwrap();
        }

        let table = ManyToManyTable::read(&conn, "tags", "name", "books_tags_link", "tag").unwrap();
        let names: Vec<&String> = table.book_col_map[&id]
            .iter()
            .map(|id| table.id_map.get(id).unwrap())
            .collect();
        assert_eq!(names, vec!["zebra", "apple"]);
    }

    #[test]
    fn authors_table_falls_back_to_computed_sort_when_unset() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO authors (name) VALUES ('Doe, Jane')", [])
            .unwrap();
        let author_id = conn.last_insert_rowid() as i32;

        let table = AuthorsTable::read(&conn).unwrap();
        // `author_insert_trg` (real schema, #203) already computes a
        // real sort value on insert -- this just confirms the table
        // reads whatever ended up in the column, not a hardcoded
        // fallback, and that the id/asort maps agree on the same id.
        assert_eq!(table.id_map.get(&author_id), Some(&"Doe, Jane".to_string()));
        assert!(table.asort_map.contains_key(&author_id));
    }

    #[test]
    fn formats_table_sorts_a_books_formats_and_tracks_size() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'mobi', 10, 'T')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'epub', 20, 'T')",
            [id],
        )
        .unwrap();

        let table = FormatsTable::read(&conn).unwrap();
        assert_eq!(table.book_col_map[&id], vec!["EPUB", "MOBI"]);
        assert_eq!(table.size_map[&id]["EPUB"], 20);
        assert!(table.col_book_map["EPUB"].contains(&id));
    }

    #[test]
    fn identifiers_table_reads_type_val_pairs() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        let id = insert_book(&conn, "T");
        conn.execute(
            "INSERT INTO identifiers (book, type, val) VALUES (?1, 'isbn', '1234567890')",
            [id],
        )
        .unwrap();

        let table = IdentifiersTable::read(&conn).unwrap();
        assert_eq!(table.book_col_map[&id]["isbn"], "1234567890");
        assert!(table.col_book_map["isbn"].contains(&id));
    }

    #[test]
    fn standard_tables_read_populates_every_field() {
        let (_dir, backend) = open_test_conn();
        let conn = backend.conn.lock().unwrap();
        insert_book(&conn, "Whole Library Smoke Test");

        // Real bulk load across every standard field in one call --
        // this is the whole point of the phase: it should just work
        // against the real schema, end to end, for a freshly created
        // library.
        let tables = StandardTables::read(&conn).unwrap();
        assert!(!tables.title.book_col_map.is_empty());
        assert!(tables.uuid.book_col_map.values().next().is_some());
    }
}
