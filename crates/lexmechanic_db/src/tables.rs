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
//! This pass ports the **read side**: real `read()` bulk-loading for
//! every standard field, faithfully matching upstream's map shapes.
//! It does **not** port the mutation side (`remove_books`/
//! `remove_items`/`rename_item`/`set_links`/`fix_link_table`/
//! `fix_case_duplicates`/format-specific mutators) -- writes still go
//! through `Cache`'s existing real, tested, per-call-SQL write
//! methods, which invalidate the read-side snapshot this file
//! provides rather than updating it incrementally in place; see
//! `cache.rs`'s module doc for that story.
//!
//! **Phase 3 (the cutover) has landed**: [`StandardTables`] (via
//! [`crate::fields::FieldStore`]) is what [`crate::cache::Cache::field_for`]
//! actually reads from now, not per-call SQL. Both `crate::view::View::sort`
//! and every field access in `search.rs` already go through
//! `Cache::field_for` themselves rather than running their own SQL, so
//! both benefit automatically -- no separate rearchitecture of either
//! was needed. See `cache.rs`'s module doc for the cutover's
//! cache-invalidation-on-write story.
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

use indexmap::IndexMap;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

/// `books.title, sort, author_sort, isbn, path, timestamp, pubdate,
/// last_modified, series_index, uuid` as ten [`OneToOneTable`]s, from one
/// scan of `books`. Exactly what ten calls of [`OneToOneTable::read`] give:
/// a book whose value is NULL has no entry in that column's table.
pub(crate) fn read_book_columns(conn: &Connection) -> Result<[OneToOneTable; 10]> {
    let mut stmt = conn.prepare("SELECT id, title, sort, author_sort, isbn, path, timestamp, pubdate, last_modified, series_index, uuid FROM books")?;
    let mut tables: [OneToOneTable; 10] = Default::default();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i32 = row.get(0)?;
        for (i, table) in tables.iter_mut().enumerate() {
            if let Some(s) = sql_value_to_string(row.get::<_, SqlValue>(i + 1)?) {
                table.book_col_map.insert(id, s);
            }
        }
    }
    Ok(tables)
}

/// Port of `OneToOneTable`: one value per book (`book_col_map`).
/// `id_column` is `"id"` for the `books` table itself, `"book"` for
/// every satellite one-to-one table (`comments`), matching upstream's
/// `idcol = 'id' if table == 'books' else 'book'`.
#[derive(Debug, Default, Clone, PartialEq)]
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
#[derive(Debug, Default, Clone, PartialEq)]
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
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UuidTable {
    pub book_col_map: HashMap<i32, String>,
    pub uuid_to_id_map: HashMap<String, i32>,
}

impl UuidTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        Ok(Self::from_one_to_one(OneToOneTable::read(conn, "books", "id", "uuid")?))
    }

    /// The table for `inner`, a book -> uuid map.
    fn from_one_to_one(inner: OneToOneTable) -> Self {
        let uuid_to_id_map = inner
            .book_col_map
            .iter()
            .map(|(&id, uuid)| (uuid.clone(), id))
            .collect();
        Self {
            book_col_map: inner.book_col_map,
            uuid_to_id_map,
        }
    }

    pub fn lookup_by_uuid(&self, uuid: &str) -> Option<i32> {
        self.uuid_to_id_map.get(uuid).copied()
    }
}

/// Port of `ManyToOneTable`: each book has at most one value, each
/// value can apply to many books (`series`/`publisher`/`rating`).
#[derive(Debug, Default, Clone, PartialEq)]
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
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ManyToManyTable {
    pub id_map: HashMap<i32, String>,
    pub link_map: HashMap<i32, String>,
    pub col_book_map: HashMap<i32, HashSet<i32>>,
    pub book_col_map: HashMap<i32, Vec<i32>>,
}

impl ManyToManyTable {
    /// `order_by` is the link table's own ordering column -- `"id"`
    /// (link-row insertion order) for most many-to-many fields, but
    /// `"item_order"` for `languages` specifically: the real schema's
    /// `books_languages_link` has its own explicit display-order
    /// column, separate from insertion order (`Cache::set_field`'s
    /// many-to-many setter, #223, writes it explicitly for exactly
    /// this reason).
    pub fn read(
        conn: &Connection,
        item_table: &str,
        item_column: &str,
        link_table: &str,
        link_column: &str,
        order_by: &str,
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
            let sql = format!("SELECT book, {link_column} FROM {link_table} ORDER BY {order_by}");
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
#[derive(Debug, Default, Clone, PartialEq)]
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
                    _ => lexmechanic_ebooks::metadata::authors::author_to_author_sort(
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
#[derive(Debug, Default, Clone, PartialEq)]
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
/// rows are the items" shape as [`FormatsTable`]. Each book's inner
/// map is an [`IndexMap`], not a plain `HashMap`, and the read query
/// is explicitly `ORDER BY id` -- both needed so a consumer joining
/// a book's identifiers into a single string (`Cache::field_for`'s
/// `"type:val,type:val"` contract) gets a deterministic, insertion-
/// order result instead of `HashMap`'s unspecified iteration order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct IdentifiersTable {
    pub book_col_map: HashMap<i32, IndexMap<String, String>>,
    pub col_book_map: HashMap<String, HashSet<i32>>,
}

impl IdentifiersTable {
    pub fn read(conn: &Connection) -> Result<Self> {
        let mut book_col_map: HashMap<i32, IndexMap<String, String>> = HashMap::new();
        let mut col_book_map: HashMap<String, HashSet<i32>> = HashMap::new();

        let mut stmt = conn.prepare("SELECT book, type, val FROM identifiers ORDER BY id")?;
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
#[derive(Clone)]
pub struct StandardTables {
    pub title: Arc<OneToOneTable>,
    pub sort: Arc<OneToOneTable>,
    pub author_sort: Arc<OneToOneTable>,
    pub isbn: Arc<OneToOneTable>,
    pub path: Arc<OneToOneTable>,
    pub timestamp: Arc<OneToOneTable>,
    pub pubdate: Arc<OneToOneTable>,
    pub last_modified: Arc<OneToOneTable>,
    pub series_index: Arc<OneToOneTable>,
    pub comments: Arc<OneToOneTable>,
    pub uuid: Arc<UuidTable>,
    pub size: Arc<SizeTable>,
    pub series: Arc<ManyToOneTable>,
    pub publisher: Arc<ManyToOneTable>,
    pub rating: Arc<ManyToOneTable>,
    pub tags: Arc<ManyToManyTable>,
    pub languages: Arc<ManyToManyTable>,
    pub authors: Arc<AuthorsTable>,
    pub formats: Arc<FormatsTable>,
    pub identifiers: Arc<IdentifiersTable>,
}

impl StandardTables {
    pub fn read(conn: &Connection) -> Result<Self> {
        // The ten columns of `books` in one scan of it (it was ten scans).
        let [title, sort, author_sort, isbn, path, timestamp, pubdate, last_modified, series_index, uuid] = read_book_columns(conn)?;
        Ok(Self {
            title: Arc::new(title),
            sort: Arc::new(sort),
            author_sort: Arc::new(author_sort),
            isbn: Arc::new(isbn),
            path: Arc::new(path),
            timestamp: Arc::new(timestamp),
            pubdate: Arc::new(pubdate),
            last_modified: Arc::new(last_modified),
            series_index: Arc::new(series_index),
            comments: Arc::new(OneToOneTable::read(conn, "comments", "book", "text")?),
            uuid: Arc::new(UuidTable::from_one_to_one(uuid)),
            size: Arc::new(SizeTable::read(conn)?),
            series: Arc::new(ManyToOneTable::read(conn, "series", "name", "books_series_link", "series")?),
            publisher: Arc::new(ManyToOneTable::read(
                conn,
                "publishers",
                "name",
                "books_publishers_link",
                "publisher",
            )?),
            rating: Arc::new(read_rating_table(conn)?),
            tags: Arc::new(ManyToManyTable::read(conn, "tags", "name", "books_tags_link", "tag", "id")?),
            languages: Arc::new(ManyToManyTable::read(
                conn,
                "languages",
                "lang_code",
                "books_languages_link",
                "lang_code",
                "item_order",
            )?),
            authors: Arc::new(AuthorsTable::read(conn)?),
            formats: Arc::new(FormatsTable::read(conn)?),
            identifiers: Arc::new(IdentifiersTable::read(conn)?),
        })
    }
}

/// One of the item tables (the ones whose rows are named values shared
/// between books), for [`StandardTables::reload_item_table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemTable {
    Series,
    Publishers,
    Ratings,
    Tags,
    Languages,
    Authors,
}

fn forget_in_col_book<K: std::hash::Hash + Eq>(col_book_map: &mut HashMap<K, HashSet<i32>>, item: &K, book: i32) {
    if let Some(set) = col_book_map.get_mut(item) {
        set.remove(&book);
    }
}

impl ManyToOneTable {
    fn forget_book(&mut self, book: i32) {
        if let Some(item) = self.book_col_map.remove(&book) {
            forget_in_col_book(&mut self.col_book_map, &item, book);
        }
    }

    fn read_book(&mut self, conn: &Connection, link_table: &str, link_column: &str, book: i32) -> Result<()> {
        let sql = format!("SELECT {link_column} FROM {link_table} WHERE book = ?1 ORDER BY id");
        let mut stmt = conn.prepare_cached(&sql)?;
        let items: Vec<i32> = stmt.query_map([book], |r| r.get::<_, i32>(0))?.collect::<Result<_>>()?;
        for item in items {
            self.col_book_map.entry(item).or_default().insert(book);
            self.book_col_map.insert(book, item);
        }
        Ok(())
    }
}

impl ManyToManyTable {
    fn forget_book(&mut self, book: i32) {
        if let Some(items) = self.book_col_map.remove(&book) {
            for item in items {
                forget_in_col_book(&mut self.col_book_map, &item, book);
            }
        }
    }

    fn read_book(&mut self, conn: &Connection, link_table: &str, link_column: &str, order_by: &str, book: i32) -> Result<()> {
        let sql = format!("SELECT {link_column} FROM {link_table} WHERE book = ?1 ORDER BY {order_by}");
        let mut stmt = conn.prepare_cached(&sql)?;
        let items: Vec<i32> = stmt.query_map([book], |r| r.get::<_, i32>(0))?.collect::<Result<_>>()?;
        for item in items {
            self.col_book_map.entry(item).or_default().insert(book);
            self.book_col_map.entry(book).or_default().push(item);
        }
        Ok(())
    }
}

/// Which of [`StandardTables`]' tables a change touched: the answer
/// [`StandardTables::reload_book`] and [`StandardTables::reload_item_table`]
/// give, for the owner to version its tables by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TableSet(u32);

macro_rules! table_bits {
    ($($name:ident = $bit:expr),* $(,)?) => {
        impl TableSet {
            $(pub const $name: TableSet = TableSet(1 << $bit);)*
            /// Every table.
            pub const ALL: TableSet = TableSet(0 $(| (1 << $bit))*);
        }
    };
}

table_bits!(
    TITLE = 0, SORT = 1, AUTHOR_SORT = 2, ISBN = 3, PATH = 4, TIMESTAMP = 5, PUBDATE = 6, LAST_MODIFIED = 7, SERIES_INDEX = 8, COMMENTS = 9, UUID = 10, SIZE = 11, SERIES = 12, PUBLISHER = 13,
    RATING = 14, TAGS = 15, LANGUAGES = 16, AUTHORS = 17, FORMATS = 18, IDENTIFIERS = 19, MEMBERS = 20,
);

impl TableSet {
    /// How many tables there are.
    pub const COUNT: usize = 21;

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: TableSet) {
        self.0 |= other.0;
    }

    pub fn union(self, other: TableSet) -> TableSet {
        TableSet(self.0 | other.0)
    }

    /// The position of each table in the set, for indexing an array of
    /// [`TableSet::COUNT`].
    pub fn indices(self) -> impl Iterator<Item = usize> {
        (0..Self::COUNT).filter(move |i| self.0 & (1 << i) != 0)
    }
}

/// What it takes to swap one book's rows in or out of a table without
/// touching any other book's: [`StandardTables::reload_book`] compares each
/// table's rows for the book with a fresh read and copies the table (it is
/// shared, see [`StandardTables`]) only if they differ.
trait BookSlice {
    /// Whether `self` and `other` hold the same for `book`.
    fn same_for(&self, other: &Self, book: i32) -> bool;
    /// Drops everything held for `book`.
    fn forget(&mut self, book: i32);
    /// Takes over what `other` holds for `book` (which `self` no longer does).
    fn adopt(&mut self, other: &Self, book: i32);
}

impl BookSlice for OneToOneTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        self.book_col_map.remove(&book);
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(v) = other.book_col_map.get(&book) {
            self.book_col_map.insert(book, v.clone());
        }
    }
}

impl BookSlice for UuidTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        if let Some(uuid) = self.book_col_map.remove(&book) {
            if self.uuid_to_id_map.get(&uuid) == Some(&book) {
                self.uuid_to_id_map.remove(&uuid);
            }
        }
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(uuid) = other.book_col_map.get(&book) {
            self.uuid_to_id_map.insert(uuid.clone(), book);
            self.book_col_map.insert(book, uuid.clone());
        }
    }
}

impl BookSlice for SizeTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        self.book_col_map.remove(&book);
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(v) = other.book_col_map.get(&book) {
            self.book_col_map.insert(book, *v);
        }
    }
}

impl BookSlice for ManyToOneTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        self.forget_book(book);
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(&item) = other.book_col_map.get(&book) {
            self.col_book_map.entry(item).or_default().insert(book);
            self.book_col_map.insert(book, item);
        }
    }
}

impl BookSlice for ManyToManyTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        self.forget_book(book);
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(items) = other.book_col_map.get(&book) {
            for &item in items {
                self.col_book_map.entry(item).or_default().insert(book);
            }
            self.book_col_map.insert(book, items.clone());
        }
    }
}

impl BookSlice for AuthorsTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        if let Some(items) = self.book_col_map.remove(&book) {
            for item in items {
                forget_in_col_book(&mut self.col_book_map, &item, book);
            }
        }
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(items) = other.book_col_map.get(&book) {
            for &item in items {
                self.col_book_map.entry(item).or_default().insert(book);
            }
            self.book_col_map.insert(book, items.clone());
        }
    }
}

impl BookSlice for FormatsTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        self.fname_map.get(&book) == other.fname_map.get(&book) && self.size_map.get(&book) == other.size_map.get(&book) && self.book_col_map.get(&book) == other.book_col_map.get(&book)
    }
    fn forget(&mut self, book: i32) {
        self.fname_map.remove(&book);
        self.size_map.remove(&book);
        if let Some(fmts) = self.book_col_map.remove(&book) {
            for f in fmts {
                forget_in_col_book(&mut self.col_book_map, &f, book);
            }
        }
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(names) = other.fname_map.get(&book) {
            for fmt in names.keys() {
                self.col_book_map.entry(fmt.clone()).or_default().insert(book);
            }
            self.fname_map.insert(book, names.clone());
        }
        if let Some(sizes) = other.size_map.get(&book) {
            self.size_map.insert(book, sizes.clone());
        }
        if let Some(fmts) = other.book_col_map.get(&book) {
            self.book_col_map.insert(book, fmts.clone());
        }
    }
}

impl BookSlice for IdentifiersTable {
    fn same_for(&self, other: &Self, book: i32) -> bool {
        // In stored order: an `IndexMap` compares equal whatever the order of its keys, and the order is what the field shows.
        match (self.book_col_map.get(&book), other.book_col_map.get(&book)) {
            (None, None) => true,
            (Some(a), Some(b)) => a.iter().eq(b.iter()),
            _ => false,
        }
    }
    fn forget(&mut self, book: i32) {
        if let Some(idents) = self.book_col_map.remove(&book) {
            for (k, _) in idents {
                forget_in_col_book(&mut self.col_book_map, &k, book);
            }
        }
    }
    fn adopt(&mut self, other: &Self, book: i32) {
        if let Some(idents) = other.book_col_map.get(&book) {
            for k in idents.keys() {
                self.col_book_map.entry(k.clone()).or_default().insert(book);
            }
            self.book_col_map.insert(book, idents.clone());
        }
    }
}

/// Copies `fresh`'s rows for `book` into `table` if they differ from what
/// `table` has, copying `table` itself first only if something else shares it.
fn sync_book<T: BookSlice + Clone>(table: &mut Arc<T>, fresh: &Arc<T>, book: i32, bit: TableSet, changed: &mut TableSet) {
    if table.same_for(fresh, book) {
        return;
    }
    let owned = Arc::make_mut(table);
    owned.forget(book);
    owned.adopt(fresh, book);
    changed.insert(bit);
}

/// Replaces `table` with `fresh` unless they are equal.
fn swap_table<T: PartialEq>(table: &mut Arc<T>, fresh: T, bit: TableSet, changed: &mut TableSet) {
    if **table != fresh {
        *table = Arc::new(fresh);
        changed.insert(bit);
    }
}

impl StandardTables {
    /// Every table empty.
    pub fn empty() -> Self {
        Self {
            title: Default::default(),
            sort: Default::default(),
            author_sort: Default::default(),
            isbn: Default::default(),
            path: Default::default(),
            timestamp: Default::default(),
            pubdate: Default::default(),
            last_modified: Default::default(),
            series_index: Default::default(),
            comments: Default::default(),
            uuid: Default::default(),
            size: Default::default(),
            series: Default::default(),
            publisher: Default::default(),
            rating: Default::default(),
            tags: Default::default(),
            languages: Default::default(),
            authors: Default::default(),
            formats: Default::default(),
            identifiers: Default::default(),
        }
    }

    /// Reloads one whole item table and the links that point at it
    /// (a rename of an author or tag, say, changes what every book
    /// with it reads back). Says which table changed: none if what was
    /// read is what was already held.
    pub fn reload_item_table(&mut self, conn: &Connection, which: ItemTable) -> Result<TableSet> {
        let mut changed = TableSet::default();
        match which {
            ItemTable::Series => swap_table(&mut self.series, ManyToOneTable::read(conn, "series", "name", "books_series_link", "series")?, TableSet::SERIES, &mut changed),
            ItemTable::Publishers => swap_table(&mut self.publisher, ManyToOneTable::read(conn, "publishers", "name", "books_publishers_link", "publisher")?, TableSet::PUBLISHER, &mut changed),
            ItemTable::Ratings => swap_table(&mut self.rating, read_rating_table(conn)?, TableSet::RATING, &mut changed),
            ItemTable::Tags => swap_table(&mut self.tags, ManyToManyTable::read(conn, "tags", "name", "books_tags_link", "tag", "id")?, TableSet::TAGS, &mut changed),
            ItemTable::Languages => swap_table(&mut self.languages, ManyToManyTable::read(conn, "languages", "lang_code", "books_languages_link", "lang_code", "item_order")?, TableSet::LANGUAGES, &mut changed),
            ItemTable::Authors => swap_table(&mut self.authors, AuthorsTable::read(conn)?, TableSet::AUTHORS, &mut changed),
        }
        Ok(changed)
    }

    /// Reads one whole item table into `self` (and nothing else): the
    /// connection half of [`StandardTables::reload_item_table`], for an owner
    /// that applies the result later, away from the connection.
    pub fn read_item_table(&mut self, conn: &Connection, which: ItemTable) -> Result<()> {
        match which {
            ItemTable::Series => self.series = Arc::new(ManyToOneTable::read(conn, "series", "name", "books_series_link", "series")?),
            ItemTable::Publishers => self.publisher = Arc::new(ManyToOneTable::read(conn, "publishers", "name", "books_publishers_link", "publisher")?),
            ItemTable::Ratings => self.rating = Arc::new(read_rating_table(conn)?),
            ItemTable::Tags => self.tags = Arc::new(ManyToManyTable::read(conn, "tags", "name", "books_tags_link", "tag", "id")?),
            ItemTable::Languages => self.languages = Arc::new(ManyToManyTable::read(conn, "languages", "lang_code", "books_languages_link", "lang_code", "item_order")?),
            ItemTable::Authors => self.authors = Arc::new(AuthorsTable::read(conn)?),
        }
        Ok(())
    }

    /// Drops everything this snapshot holds about one book.
    pub fn forget_book(&mut self, book: i32) {
        fn forget<T: BookSlice + Clone>(table: &mut Arc<T>, book: i32) {
            Arc::make_mut(table).forget(book);
        }
        for t in [&mut self.title, &mut self.sort, &mut self.author_sort, &mut self.isbn, &mut self.path, &mut self.timestamp, &mut self.pubdate, &mut self.last_modified, &mut self.series_index, &mut self.comments] {
            forget(t, book);
        }
        forget(&mut self.uuid, book);
        forget(&mut self.size, book);
        forget(&mut self.series, book);
        forget(&mut self.publisher, book);
        forget(&mut self.rating, book);
        forget(&mut self.tags, book);
        forget(&mut self.languages, book);
        forget(&mut self.authors, book);
        forget(&mut self.formats, book);
        forget(&mut self.identifiers, book);
    }

    /// Re-reads one book from the database and brings this snapshot to what
    /// [`StandardTables::read`] would hold for that book, at the price of a
    /// dozen indexed lookups instead of a scan of every table. A book that no
    /// longer exists is simply forgotten.
    ///
    /// Only the tables whose rows for the book actually changed are touched
    /// (and so copied, when another snapshot shares them): editing a rating
    /// copies the rating table, not the fifty thousand titles beside it. The
    /// tables that changed are returned.
    pub fn reload_book(&mut self, conn: &Connection, book: i32) -> Result<TableSet> {
        let mut fresh = StandardTables::empty();
        fresh.read_book_rows(conn, book)?;
        Ok(self.adopt_book(&fresh, book))
    }

    /// What [`StandardTables::reload_book`] does once the book is read: makes
    /// this snapshot hold, for `book`, what `fresh` (read from the database
    /// for that one book) holds.
    pub fn adopt_book(&mut self, fresh: &StandardTables, book: i32) -> TableSet {
        let mut changed = TableSet::default();
        // Whether the book is one the library lists (has a title) is what every
        // sort order depends on beyond the field it sorts by: `MEMBERS` moves
        // when a book appears or goes, not when its title changes.
        let listed = self.title.book_col_map.contains_key(&book);
        sync_book(&mut self.title, &fresh.title, book, TableSet::TITLE, &mut changed);
        if listed != self.title.book_col_map.contains_key(&book) {
            changed.insert(TableSet::MEMBERS);
        }
        sync_book(&mut self.sort, &fresh.sort, book, TableSet::SORT, &mut changed);
        sync_book(&mut self.author_sort, &fresh.author_sort, book, TableSet::AUTHOR_SORT, &mut changed);
        sync_book(&mut self.isbn, &fresh.isbn, book, TableSet::ISBN, &mut changed);
        sync_book(&mut self.path, &fresh.path, book, TableSet::PATH, &mut changed);
        sync_book(&mut self.timestamp, &fresh.timestamp, book, TableSet::TIMESTAMP, &mut changed);
        sync_book(&mut self.pubdate, &fresh.pubdate, book, TableSet::PUBDATE, &mut changed);
        sync_book(&mut self.last_modified, &fresh.last_modified, book, TableSet::LAST_MODIFIED, &mut changed);
        sync_book(&mut self.series_index, &fresh.series_index, book, TableSet::SERIES_INDEX, &mut changed);
        sync_book(&mut self.comments, &fresh.comments, book, TableSet::COMMENTS, &mut changed);
        sync_book(&mut self.uuid, &fresh.uuid, book, TableSet::UUID, &mut changed);
        sync_book(&mut self.size, &fresh.size, book, TableSet::SIZE, &mut changed);
        sync_book(&mut self.series, &fresh.series, book, TableSet::SERIES, &mut changed);
        sync_book(&mut self.publisher, &fresh.publisher, book, TableSet::PUBLISHER, &mut changed);
        sync_book(&mut self.rating, &fresh.rating, book, TableSet::RATING, &mut changed);
        sync_book(&mut self.tags, &fresh.tags, book, TableSet::TAGS, &mut changed);
        sync_book(&mut self.languages, &fresh.languages, book, TableSet::LANGUAGES, &mut changed);
        sync_book(&mut self.authors, &fresh.authors, book, TableSet::AUTHORS, &mut changed);
        sync_book(&mut self.formats, &fresh.formats, book, TableSet::FORMATS, &mut changed);
        sync_book(&mut self.identifiers, &fresh.identifiers, book, TableSet::IDENTIFIERS, &mut changed);
        changed
    }

    /// Reads what the database holds for one book into `self`, which must
    /// hold nothing for it (a fresh [`StandardTables::empty`]).
    pub fn read_book_rows(&mut self, conn: &Connection, book: i32) -> Result<()> {
        use rusqlite::OptionalExtension;
        fn own<T: Clone>(table: &mut Arc<T>) -> &mut T {
            Arc::make_mut(table)
        }

        let row: Option<Vec<SqlValue>> = conn
            .prepare_cached("SELECT title, sort, author_sort, isbn, path, timestamp, pubdate, last_modified, series_index, uuid FROM books WHERE id = ?1")?
            .query_row([book], |r| (0..10).map(|i| r.get::<_, SqlValue>(i)).collect::<Result<Vec<_>>>())
            .optional()?;
        let Some(row) = row else { return Ok(()) };
        let mut it = row.into_iter();
        let mut next = || sql_value_to_string(it.next().unwrap_or(SqlValue::Null));
        for t in [&mut self.title, &mut self.sort, &mut self.author_sort, &mut self.isbn, &mut self.path, &mut self.timestamp, &mut self.pubdate, &mut self.last_modified, &mut self.series_index] {
            if let Some(v) = next() {
                own(t).book_col_map.insert(book, v);
            }
        }
        if let Some(uuid) = next() {
            let t = own(&mut self.uuid);
            t.uuid_to_id_map.insert(uuid.clone(), book);
            t.book_col_map.insert(book, uuid);
        }

        let text: Option<SqlValue> = conn.prepare_cached("SELECT text FROM comments WHERE book = ?1")?.query_row([book], |r| r.get(0)).optional()?;
        if let Some(s) = text.and_then(sql_value_to_string) {
            own(&mut self.comments).book_col_map.insert(book, s);
        }

        let size: Option<i64> = conn.prepare_cached("SELECT MAX(uncompressed_size) FROM data WHERE book = ?1")?.query_row([book], |r| r.get(0))?;
        if let Some(size) = size {
            own(&mut self.size).book_col_map.insert(book, size);
        }

        own(&mut self.series).read_book(conn, "books_series_link", "series", book)?;
        own(&mut self.publisher).read_book(conn, "books_publishers_link", "publisher", book)?;
        own(&mut self.rating).read_book(conn, "books_ratings_link", "rating", book)?;
        own(&mut self.tags).read_book(conn, "books_tags_link", "tag", "id", book)?;
        own(&mut self.languages).read_book(conn, "books_languages_link", "lang_code", "item_order", book)?;
        {
            let mut stmt = conn.prepare_cached("SELECT author FROM books_authors_link WHERE book = ?1 ORDER BY id")?;
            let items: Vec<i32> = stmt.query_map([book], |r| r.get::<_, i32>(0))?.collect::<Result<_>>()?;
            let authors = own(&mut self.authors);
            for item in items {
                authors.col_book_map.entry(item).or_default().insert(book);
                authors.book_col_map.entry(book).or_default().push(item);
            }
        }
        {
            let mut stmt = conn.prepare_cached("SELECT format, name, uncompressed_size FROM data WHERE book = ?1")?;
            let rows: Vec<(Option<String>, String, i64)> = stmt.query_map([book], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_>>()?;
            let formats = own(&mut self.formats);
            let mut fmts = Vec::new();
            for (fmt, name, size) in rows {
                let Some(fmt) = fmt else { continue };
                let fmt = fmt.to_uppercase();
                formats.col_book_map.entry(fmt.clone()).or_default().insert(book);
                formats.fname_map.entry(book).or_default().insert(fmt.clone(), name);
                formats.size_map.entry(book).or_default().insert(fmt.clone(), size);
                fmts.push(fmt);
            }
            if !fmts.is_empty() {
                fmts.sort();
                formats.book_col_map.insert(book, fmts);
            }
        }
        {
            let mut stmt = conn.prepare_cached("SELECT type, val FROM identifiers WHERE book = ?1 ORDER BY id")?;
            let rows: Vec<(Option<String>, Option<String>)> = stmt.query_map([book], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_>>()?;
            let identifiers = own(&mut self.identifiers);
            for (typ, val) in rows {
                if let (Some(typ), Some(val)) = (typ, val) {
                    identifiers.col_book_map.entry(typ.clone()).or_default().insert(book);
                    identifiers.book_col_map.entry(book).or_default().insert(typ, val);
                }
            }
        }
        Ok(())
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

        let table =
            ManyToManyTable::read(&conn, "tags", "name", "books_tags_link", "tag", "id").unwrap();
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
