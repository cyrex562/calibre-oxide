//! Custom columns, stored the way calibre stores them.
//!
//! calibre keeps a custom column in one of two shapes, chosen when the
//! column is created and recorded in `custom_columns.normalized`
//! (`Backend.create_custom_column`, `db/backend.py`):
//!
//! * **Not normalized** -- `datetime`, `comments`, `int`, `bool`,
//!   `float` and `composite`: one table, `custom_column_N(id, book,
//!   value)`, at most one row per book (`UNIQUE(book)`).
//! * **Normalized** -- everything else: `text` (single *or* multiple),
//!   `series`, `enumeration` and `rating`. A deduplicated value table
//!   `custom_column_N(id, value, link)` plus a link table
//!   `books_custom_column_N_link(id, book, value[, extra])`. A series
//!   column's link rows carry the series index in `extra`.
//!
//! The write rules here are the ones in `db/write.py` (`many_one`,
//! `many_many`, `one_one_in_other`, `custom_series_index`) and the
//! adapters in `db/write.py::get_adapter` / `db/cache.py::_set_field`:
//!
//! * a value row is reused when an existing one matches (case-insensitively
//!   for `text`/`series`, exactly for `enumeration`/`rating`), created
//!   otherwise, and values no book uses any more are removed;
//! * a multiple-value column keeps the order it was written in (link rows
//!   are read back `ORDER BY id`), with case-insensitive duplicates dropped;
//! * a rating is clamped to 0-10 and 0 clears it;
//! * an enumeration only accepts a value in the column's `enum_values`;
//! * a series value may carry its index as `Name [3.5]`; with none given a
//!   book keeps the index it had, else gets 1.0.
//!
//! One deliberate difference: calibre's `Cache.set_field` defaults to
//! `allow_case_change=True`, which *renames* a shared value when a book is
//! given the same text in another case (writing "fantasy" renames "Fantasy"
//! for every book that has it). This port never does that -- the existing
//! value is reused as written -- the behaviour of calibre's own
//! `LibraryDatabase.set_custom` (`allow_case_change=False`). It keeps a
//! replayed change log deterministic and means one book's edit cannot
//! rewrite another's metadata.

use anyhow::{anyhow, bail, Result};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// A row of `custom_columns`, as the read/write code needs it.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub num: i32,
    pub label: String,
    pub datatype: String,
    pub is_multiple: bool,
    pub normalized: bool,
    pub display: Value,
}

impl ColumnDef {
    pub fn table(&self) -> String {
        format!("custom_column_{}", self.num)
    }

    pub fn link_table(&self) -> String {
        format!("books_custom_column_{}_link", self.num)
    }

    /// `(ui_to_list, list_to_ui)` for a multiple-value column
    /// (`initialize_custom_columns`' `multiple_seps`). `is_names` columns
    /// are separated like authors.
    pub fn separators(&self) -> (&'static str, &'static str) {
        if self.display.get("is_names").and_then(Value::as_bool).unwrap_or(false) {
            ("&", " & ")
        } else {
            (",", ", ")
        }
    }

    fn enum_values(&self) -> Vec<String> {
        self.display
            .get("enum_values")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
}

/// What a label names: a column's value, or a series column's index
/// (`#label_index` in calibre's field naming).
#[derive(Debug, Clone)]
pub enum Target {
    Value(ColumnDef),
    SeriesIndex(ColumnDef),
}

const COLUMN_SQL: &str = "SELECT id, label, datatype, is_multiple, normalized, display FROM custom_columns";

fn row_to_def(r: &rusqlite::Row) -> rusqlite::Result<ColumnDef> {
    let display: String = r.get(5)?;
    Ok(ColumnDef {
        num: r.get(0)?,
        label: r.get(1)?,
        datatype: r.get(2)?,
        is_multiple: r.get(3)?,
        normalized: r.get(4)?,
        display: serde_json::from_str(&display).unwrap_or_else(|_| serde_json::json!({})),
    })
}

/// The column with this label (a leading `#` is accepted).
pub fn column(conn: &Connection, label: &str) -> rusqlite::Result<Option<ColumnDef>> {
    let label = label.strip_prefix('#').unwrap_or(label);
    conn.query_row(&format!("{COLUMN_SQL} WHERE label = ?1"), [label], row_to_def).optional()
}

/// Every column.
pub fn all_columns(conn: &Connection) -> rusqlite::Result<Vec<ColumnDef>> {
    let mut stmt = conn.prepare(&format!("{COLUMN_SQL} ORDER BY id"))?;
    let rows = stmt.query_map([], row_to_def)?;
    rows.collect()
}

/// Resolves a label to a column, or to a series column's `_index`. A real
/// column named `x_index` wins over the index of series column `x`.
pub fn resolve(conn: &Connection, label: &str) -> rusqlite::Result<Option<Target>> {
    if let Some(def) = column(conn, label)? {
        return Ok(Some(Target::Value(def)));
    }
    let bare = label.strip_prefix('#').unwrap_or(label);
    if let Some(base) = bare.strip_suffix("_index") {
        if let Some(def) = column(conn, base)? {
            if def.datatype == "series" {
                return Ok(Some(Target::SeriesIndex(def)));
            }
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------

/// `Backend.create_custom_column`'s table, index, trigger and view script,
/// executed for column `num`. The text is calibre's own so a library made
/// here is indistinguishable from one calibre made.
pub fn create_schema(conn: &Connection, num: i64, datatype: &str, normalized: bool) -> Result<()> {
    let dt = match datatype {
        "rating" | "int" => "INT",
        "text" | "comments" | "series" | "composite" | "enumeration" => "TEXT",
        "float" => "REAL",
        "datetime" => "timestamp",
        "bool" => "BOOL",
        other => bail!("'{other}' is not a supported data type"),
    };
    let collate = if dt == "TEXT" { "COLLATE NOCASE" } else { "" };
    let table = format!("custom_column_{num}");
    let lt = format!("books_custom_column_{num}_link");

    let script = if normalized {
        let s_index = if datatype == "series" { "extra REAL," } else { "" };
        format!(
            r#"
CREATE TABLE {table}(
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    value {dt} NOT NULL {collate},
    link TEXT NOT NULL DEFAULT "",
    UNIQUE(value));
CREATE INDEX {table}_idx ON {table} (value {collate});
CREATE TABLE {lt}(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    book INTEGER NOT NULL,
    value INTEGER NOT NULL,
    {s_index}
    UNIQUE(book, value)
    );
CREATE INDEX {lt}_aidx ON {lt} (value);
CREATE INDEX {lt}_bidx ON {lt} (book);
CREATE TRIGGER fkc_update_{lt}_a
        BEFORE UPDATE OF book ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
CREATE TRIGGER fkc_update_{lt}_b
        BEFORE UPDATE OF author ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from {table} WHERE id=NEW.value) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: value not in {table}')
            END;
        END;
CREATE TRIGGER fkc_insert_{lt}
        BEFORE INSERT ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                WHEN (SELECT id from {table} WHERE id=NEW.value) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: value not in {table}')
            END;
        END;
CREATE TRIGGER fkc_delete_{lt}
        AFTER DELETE ON {table}
        BEGIN
            DELETE FROM {lt} WHERE value=OLD.id;
        END;
CREATE VIEW tag_browser_{table} AS SELECT
    id,
    value,
    (SELECT COUNT(id) FROM {lt} WHERE value={table}.id) count,
    (SELECT AVG(r.rating)
     FROM {lt},
          books_ratings_link as bl,
          ratings as r
     WHERE {lt}.value={table}.id and bl.book={lt}.book and
           r.id = bl.rating and r.rating <> 0) avg_rating,
    value AS sort
FROM {table};
CREATE VIEW tag_browser_filtered_{table} AS SELECT
    id,
    value,
    (SELECT COUNT({lt}.id) FROM {lt} WHERE value={table}.id AND
    books_list_filter(book)) count,
    (SELECT AVG(r.rating)
     FROM {lt},
          books_ratings_link as bl,
          ratings as r
     WHERE {lt}.value={table}.id AND bl.book={lt}.book AND
           r.id = bl.rating AND r.rating <> 0 AND
           books_list_filter(bl.book)) avg_rating,
    value AS sort
FROM {table};
"#
        )
    } else {
        format!(
            r#"
CREATE TABLE {table}(
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    book  INTEGER,
    value {dt} NOT NULL {collate},
    UNIQUE(book));
CREATE INDEX {table}_idx ON {table} (book);
CREATE TRIGGER fkc_insert_{table}
        BEFORE INSERT ON {table}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
CREATE TRIGGER fkc_update_{table}
        BEFORE UPDATE OF book ON {table}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
"#
        )
    };
    conn.execute_batch(&script)?;
    Ok(())
}

/// Everything `initialize_custom_columns` drops for a deleted column.
pub fn drop_schema(conn: &Connection, num: i32) -> Result<()> {
    let (table, lt) = (format!("custom_column_{num}"), format!("books_custom_column_{num}_link"));
    conn.execute_batch(&format!(
        "DROP INDEX   IF EXISTS {table}_idx;
         DROP INDEX   IF EXISTS {lt}_aidx;
         DROP INDEX   IF EXISTS {lt}_bidx;
         DROP TRIGGER IF EXISTS fkc_update_{lt}_a;
         DROP TRIGGER IF EXISTS fkc_update_{lt}_b;
         DROP TRIGGER IF EXISTS fkc_insert_{lt};
         DROP TRIGGER IF EXISTS fkc_delete_{lt};
         DROP TRIGGER IF EXISTS fkc_insert_{table};
         DROP TRIGGER IF EXISTS fkc_update_{table};
         DROP TRIGGER IF EXISTS fkc_delete_{table};
         DROP VIEW    IF EXISTS tag_browser_{table};
         DROP VIEW    IF EXISTS tag_browser_filtered_{table};
         DROP TABLE   IF EXISTS {table};
         DROP TABLE   IF EXISTS {lt};"
    ))?;
    Ok(())
}

/// `create_custom_column`'s own argument checks.
pub fn validate_new_column(label: &str, datatype: &str) -> Result<()> {
    if label.is_empty() {
        bail!("No label was provided");
    }
    let ok = label.chars().all(|c| c.is_alphanumeric() || c == '_')
        && label.chars().next().is_some_and(char::is_alphabetic)
        && label.to_lowercase() == label;
    if !ok {
        bail!("The label must contain only lower case letters, digits and underscores, and start with a letter");
    }
    if !matches!(datatype, "rating" | "text" | "comments" | "datetime" | "int" | "float" | "bool" | "series" | "composite" | "enumeration") {
        bail!("'{datatype}' is not a supported data type");
    }
    Ok(())
}

/// Whether a column of this datatype gets a link table.
pub fn is_normalized(datatype: &str) -> bool {
    !matches!(datatype, "datetime" | "comments" | "int" | "bool" | "float" | "composite")
}

// ---------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------

/// A column's value for a book, as text. A multiple-value column is joined
/// with its `list_to_ui` separator, in the order it was written. `None` when
/// the book has no value (and always for a composite, which is computed).
pub fn read_value(conn: &Connection, def: &ColumnDef, book: i32) -> rusqlite::Result<Option<String>> {
    if def.datatype == "composite" {
        return Ok(None);
    }
    let table = def.table();
    if !def.normalized {
        return conn
            .query_row(&format!("SELECT value FROM {table} WHERE book = ?1"), [book], |row| {
                Ok(match row.get::<_, Sql>(0)? {
                    Sql::Null => None,
                    Sql::Integer(n) => Some(n.to_string()),
                    Sql::Real(f) => Some(if def.datatype == "int" || def.datatype == "bool" { (f as i64).to_string() } else { f.to_string() }),
                    Sql::Text(s) => Some(s),
                    Sql::Blob(b) => Some(String::from_utf8_lossy(&b).into_owned()),
                })
            })
            .optional()
            .map(Option::flatten);
    }
    let lt = def.link_table();
    let mut stmt = conn.prepare(&format!(
        "SELECT v.value FROM {lt} l JOIN {table} v ON v.id = l.value WHERE l.book = ?1 ORDER BY l.id"
    ))?;
    let vals: Vec<String> = stmt
        .query_map([book], |row| {
            Ok(match row.get::<_, Sql>(0)? {
                Sql::Integer(n) => n.to_string(),
                Sql::Real(f) => f.to_string(),
                Sql::Text(s) => s,
                _ => String::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if vals.is_empty() {
        return Ok(None);
    }
    Ok(Some(if def.is_multiple { vals.join(def.separators().1) } else { vals.into_iter().next().unwrap() }))
}

/// The book's index in a series column, if it has a value there.
pub fn read_series_index(conn: &Connection, def: &ColumnDef, book: i32) -> rusqlite::Result<Option<f64>> {
    if def.datatype != "series" || !def.normalized {
        return Ok(None);
    }
    conn.query_row(&format!("SELECT extra FROM {} WHERE book = ?1", def.link_table()), [book], |r| r.get::<_, Option<f64>>(0))
        .optional()
        .map(Option::flatten)
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

/// `series_index_pat` / `get_series_values`: `"Name [3.5]"` -> `("Name", Some(3.5))`.
pub fn split_series_value(val: &str) -> (String, Option<f64>) {
    let t = val.trim();
    if let Some(inner) = t.strip_suffix(']') {
        if let Some(open) = inner.rfind('[') {
            let (head, idx) = (&inner[..open], &inner[open + 1..]);
            if !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit() || c == '.') && head.ends_with(char::is_whitespace) {
                if let Ok(n) = idx.parse::<f64>() {
                    return (head.trim().to_string(), Some(n));
                }
            }
        }
    }
    (val.to_string(), None)
}

fn single_text(v: &str) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `multiple_text`: split, trim, collapse inner whitespace, drop empties and
/// (case-insensitive) duplicates, keeping the first and the order.
fn multiple_text(def: &ColumnDef, v: &str) -> Vec<String> {
    let (sep, _) = def.separators();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in v.split(sep) {
        let item = part.split_whitespace().collect::<Vec<_>>().join(" ");
        if !item.is_empty() && seen.insert(item.to_lowercase()) {
            out.push(item);
        }
    }
    out
}

fn parse_number(v: &str, what: &str) -> Result<Option<f64>> {
    let t = v.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    t.parse::<f64>().map(Some).map_err(|_| anyhow!("'{t}' is not a valid {what}"))
}

fn parse_bool(v: &str) -> Result<Option<bool>> {
    let t = v.trim().to_lowercase();
    Ok(match t.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        "none" | "" => None,
        other => Some(other.parse::<i64>().map_err(|_| anyhow!("'{v}' is not a valid yes/no value"))? != 0),
    })
}

fn parse_datetime(v: &str) -> Result<Option<String>> {
    let t = v.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let dt = calibre_utils::date::parse_date(t, true).ok_or_else(|| anyhow!("'{t}' is not a valid date"))?;
    // `isoformat(x, sep=' ')`: microseconds only when there are some.
    let fmt = if dt.timestamp_subsec_micros() == 0 { "%Y-%m-%d %H:%M:%S+00:00" } else { "%Y-%m-%d %H:%M:%S%.6f+00:00" };
    Ok(Some(dt.format(fmt).to_string()))
}

/// Writes `value` for `book`. An empty (or `none`) value clears. Returns
/// the series index now stored when the column is a series (so the caller
/// can log it), and errors on a value the column cannot hold.
pub fn write_value(conn: &Connection, def: &ColumnDef, book: i32, value: &str) -> Result<Option<f64>> {
    // IMMEDIATE, not the default deferred: this transaction starts by
    // *reading* (looking up the value row, the book's current links) and
    // only then writes. A deferred transaction upgrades read -> write at
    // that point, and when another connection holds the write lock (the
    // library's background WAL-checkpoint thread opens its own connection
    // and runs `wal_checkpoint(TRUNCATE)`) SQLite cannot honour the busy
    // timeout for an upgrade -- it fails at once with "database is locked".
    // Taking the write lock at BEGIN lets the busy timeout do its job.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let result = write_value_inner(&tx, def, book, value);
    if result.is_ok() {
        tx.commit()?;
    }
    result
}

fn write_value_inner(conn: &Connection, def: &ColumnDef, book: i32, value: &str) -> Result<Option<f64>> {
    let dt = def.datatype.as_str();
    if dt == "composite" {
        return Ok(None); // computed from a template, never stored
    }
    if !def.normalized {
        let stored: Option<Sql> = match dt {
            "int" => parse_number(value, "integer")?
                .map(|n| if n.fract() == 0.0 { Ok(Sql::Integer(n as i64)) } else { Err(anyhow!("'{value}' is not a whole number")) })
                .transpose()?,
            "float" => parse_number(value, "number")?.map(Sql::Real),
            "bool" => parse_bool(value)?.map(|b| Sql::Integer(b as i64)),
            "datetime" => parse_datetime(value)?.map(Sql::Text),
            _ => single_text(value).map(Sql::Text), // comments
        };
        let table = def.table();
        match stored {
            None => conn.execute(&format!("DELETE FROM {table} WHERE book = ?1"), [book])?,
            Some(v) => conn.execute(&format!("INSERT OR REPLACE INTO {table}(book, value) VALUES (?1, ?2)"), rusqlite::params![book, v])?,
        };
        return Ok(None);
    }

    let table = def.table();
    let lt = def.link_table();

    // The new value(s) as the value table holds them.
    let (items, index): (Vec<Sql>, Option<f64>) = match dt {
        "rating" => {
            let n = parse_number(value, "rating")?.map(|n| n.round().clamp(0.0, 10.0) as i64).filter(|n| *n != 0);
            (n.map(Sql::Integer).into_iter().collect(), None)
        }
        "enumeration" => match single_text(value) {
            None => (vec![], None),
            Some(v) => {
                let allowed = def.enum_values();
                if !allowed.contains(&v) {
                    bail!("'{v}' is not one of the values allowed in column '{}': {}", def.label, allowed.join(", "));
                }
                (vec![Sql::Text(v)], None)
            }
        },
        "series" => {
            let (name, idx) = split_series_value(value);
            (single_text(&name).map(Sql::Text).into_iter().collect(), idx)
        }
        _ if def.is_multiple => (multiple_text(def, value).into_iter().map(Sql::Text).collect(), None),
        _ => (single_text(value).map(Sql::Text).into_iter().collect(), None),
    };

    let case_insensitive = matches!(dt, "text" | "series");
    let mut ids = Vec::with_capacity(items.len());
    for item in &items {
        ids.push(item_id(conn, &table, item, case_insensitive)?);
    }

    // What the book has now, in link order.
    let current: Vec<(i64, Option<f64>)> = {
        let extra = if dt == "series" { "extra" } else { "NULL" };
        let mut stmt = conn.prepare(&format!("SELECT value, {extra} FROM {lt} WHERE book = ?1 ORDER BY id"))?;
        let rows = stmt.query_map([book], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let current_ids: Vec<i64> = current.iter().map(|c| c.0).collect();

    let mut final_index = None;
    if dt == "series" {
        // An explicit index wins; otherwise the book keeps what it had.
        let old = current.first().and_then(|c| c.1);
        if !ids.is_empty() {
            final_index = Some(index.or(old).unwrap_or(1.0));
        }
    }

    if ids != current_ids {
        conn.execute(&format!("DELETE FROM {lt} WHERE book = ?1"), [book])?;
        for id in &ids {
            if dt == "series" {
                conn.execute(&format!("INSERT INTO {lt}(book, value, extra) VALUES (?1, ?2, ?3)"), rusqlite::params![book, id, final_index.unwrap_or(1.0)])?;
            } else {
                conn.execute(&format!("INSERT INTO {lt}(book, value) VALUES (?1, ?2)"), rusqlite::params![book, id])?;
            }
        }
    } else if let (Some(idx), true) = (final_index, dt == "series") {
        if current.first().and_then(|c| c.1) != Some(idx) {
            conn.execute(&format!("UPDATE {lt} SET extra = ?1 WHERE book = ?2"), rusqlite::params![idx, book])?;
        }
    }

    remove_unused_items(conn, def)?;
    Ok(final_index)
}

/// `custom_series_index`: sets the index of a book's series entry. Without
/// a series there is nothing to index and nothing is written.
pub fn write_series_index(conn: &Connection, def: &ColumnDef, book: i32, value: &str) -> Result<Option<f64>> {
    // `adapt_series_index`: empty means 1.0.
    let idx = parse_number(value, "series index")?.unwrap_or(1.0);
    let lt = def.link_table();
    let changed = conn.execute(&format!("UPDATE {lt} SET extra = ?1 WHERE book = ?2"), rusqlite::params![idx, book])?;
    Ok((changed > 0).then_some(idx))
}

/// The id of the value row for `item`, creating it if there is none.
fn item_id(conn: &Connection, table: &str, item: &Sql, case_insensitive: bool) -> Result<i64> {
    // The value column is `COLLATE NOCASE`, so `=` already matches ASCII
    // case-insensitively; non-ASCII text needs the Unicode fold calibre's
    // `icu_lower` gives.
    if let Some(id) = conn.query_row(&format!("SELECT id FROM {table} WHERE value = ?1"), [item], |r| r.get::<_, i64>(0)).optional()? {
        return Ok(id);
    }
    if let (Sql::Text(text), true) = (item, case_insensitive) {
        if !text.is_ascii() {
            let lower = text.to_lowercase();
            let mut stmt = conn.prepare(&format!("SELECT id, value FROM {table}"))?;
            let hit = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .filter_map(Result::ok)
                .find(|(_, v)| v.to_lowercase() == lower);
            if let Some((id, _)) = hit {
                return Ok(id);
            }
        }
    }
    conn.execute(&format!("INSERT INTO {table}(value) VALUES (?1)"), [item])?;
    Ok(conn.last_insert_rowid())
}

/// "Remove no longer used items": value rows no link refers to.
fn remove_unused_items(conn: &Connection, def: &ColumnDef) -> Result<()> {
    conn.execute(&format!("DELETE FROM {} WHERE id NOT IN (SELECT value FROM {})", def.table(), def.link_table()), [])?;
    Ok(())
}

/// What calibre's temp `custom_books_delete_trg` does when a book is
/// deleted: every custom column's rows for the book go, and values only that
/// book used go with them.
pub fn purge_book(conn: &Connection, book: i32) -> Result<()> {
    for def in all_columns(conn)? {
        if def.normalized {
            conn.execute(&format!("DELETE FROM {} WHERE book = ?1", def.link_table()), [book])?;
            remove_unused_items(conn, &def)?;
        } else {
            conn.execute(&format!("DELETE FROM {} WHERE book = ?1", def.table()), [book])?;
        }
    }
    Ok(())
}
