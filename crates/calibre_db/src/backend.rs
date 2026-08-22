//! Port of `old_src/src/calibre/db/backend.py`'s `Connection`, `DB`, and
//! `DBPrefs` (issue #203).
//!
//! # Scope of this pass
//!
//! `backend.py`'s real `DB.__init__` also does default-preference
//! population/migration (`initialize_prefs`, ~150 lines), dynamic
//! custom-column table creation (`initialize_custom_columns`, ~100
//! lines), the in-memory field/table object model
//! (`initialize_tables`), notes/FTS bootstrapping, template-function
//! compilation, and trash-directory setup. None of that is implemented
//! here -- each is its own large, separate unit of work (custom
//! columns and the field/table model in particular belong more to
//! `cache.py`'s territory, #204). What *is* real in this pass, verified
//! against upstream:
//!
//! - Opening/creating the SQLite connection with the same pragmas
//!   Python sets.
//! - Registering the same custom SQL functions, aggregates, and
//!   collations `Connection.__init__` does (`PYNOCASE`, `icucollate`,
//!   `title_sort`, `author_to_author_sort`, `uuid4`,
//!   `books_list_filter`, `concat`, `sortconcat`/`_bar`/`_amper`,
//!   `identifiers_concat`, `aum_sortconcat`) -- real implementations,
//!   not passthrough stubs.
//! - Creating a brand-new library's schema for real: `user_version ==
//!   0` runs the *exact* bundled `metadata_sqlite.sql` DDL (copied
//!   byte-for-byte from `old_src/resources/`, not hand-transcribed),
//!   which is what upstream does too -- this crate previously assumed
//!   a `books` table already existed and would simply fail against any
//!   library it didn't itself create with ad hoc SQL.
//! - The legacy author-sort trigger fixup `DB.__init__` runs
//!   unconditionally after schema init.
//! - `library_id` (get-or-create, from the real `library_id` table --
//!   previously faked by reading a nonexistent `preferences` key).
//! - `DBPrefs`: real JSON-encoded key/value preference storage against
//!   the real `preferences` table (get/set/delete), matching
//!   `raw_to_object`/`to_raw`. Upstream's default-value population and
//!   legacy-key migration on top of this (`initialize_prefs`) is not
//!   ported.
//!
//! Upgrading an *existing* older-schema library (`schema_upgrades.py`,
//! separately still a stub) is unaffected by this file either way --
//! `initialize_database` only runs for a brand-new (`user_version ==
//! 0`) library, exactly as upstream.
//!
//! `icucollate` here is a Unicode-lowercase comparison, not upstream's
//! real ICU `sort_key`-based collation (which does locale-aware,
//! numeral-aware natural sorting). A genuine ICU binding is out of
//! scope for this pass; this is a documented approximation; true `NOCASE`-style
//! but not ICU-faithful ordering.

use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::{Connection, Error as SqlError, Result as SqlResult};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use calibre_ebooks::metadata::authors::author_to_author_sort as real_author_to_author_sort;
use calibre_ebooks::metadata::meta::title_sort as real_title_sort;

/// The bundled schema DDL. Byte-identical to upstream's
/// `resources/metadata_sqlite.sql` -- copied, not hand-transcribed, so
/// a brand-new library gets exactly the same tables/indexes/views/
/// triggers real calibre creates, including the trailing `PRAGMA
/// user_version=26` that marks it current.
const SCHEMA_SQL: &str = include_str!("../resources/metadata_sqlite.sql");

/// `Clone` is cheap and shares the same live connection (`conn` is an
/// `Arc<Mutex<Connection>>`) -- used by [`crate::library::Library`] to
/// hand its own connection to [`crate::cache::Cache`]/`search.rs`
/// without opening a second connection to the same file.
#[derive(Clone)]
pub struct Backend {
    pub library_path: PathBuf,
    pub db_path: PathBuf,
    pub conn: Arc<Mutex<Connection>>,
    /// Deprecated: superseded by [`Backend::get_pref`]/[`Backend::set_pref`],
    /// which store real JSON-typed values against the real `preferences`
    /// table. Kept only because it's `pub` API; never populated -- always empty.
    #[deprecated(note = "use get_pref/set_pref instead; this was never a faithful port")]
    pub prefs: HashMap<String, String>,
    /// Lazily-opened, shared across every clone of this `Backend`
    /// (issue #93's crate-wide write-path retrofit). `Backend::new`
    /// deliberately does *not* open this up front -- unlike
    /// `LibraryHandle::open` itself, opening a `Backend`/`Cache` must
    /// stay safe to do many times over the same library (read-only CLI
    /// commands, tests that construct more than one `Backend`/`Cache`
    /// over the same directory), so the real exclusive writer lock is
    /// only ever acquired the first time something actually needs to
    /// write. See [`Backend::write_handle`].
    write_handle: Arc<Mutex<Option<Arc<crate::library_handle::LibraryHandle>>>>,
}

impl Backend {
    pub fn new<P: AsRef<Path>>(library_path: P) -> SqlResult<Self> {
        let library_path = library_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&library_path).map_err(|e| {
            SqlError::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("failed to create library dir: {e}")),
            )
        })?;
        let db_path = library_path.join("metadata.db");

        let mut conn = Connection::open(&db_path)?;

        // Port of `Connection.__init__`'s pragmas.
        conn.execute_batch(
            "PRAGMA cache_size=-5000; PRAGMA temp_store=2; PRAGMA foreign_keys=ON;",
        )?;

        register_functions(&conn)?;

        // Port of `DB.__init__`: `if self.user_version == 0: self.initialize_database()`.
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version == 0 {
            conn.execute_batch(SCHEMA_SQL)?;
        }

        // Port of `schema_upgrades::SchemaUpgrade::upgrade_to_latest`, for an
        // *existing* library at an older schema version (a no-op loop for a
        // library that was just created above, since it's already at the
        // latest version). Reuses the same `conn` `register_functions` just
        // set up -- many migration steps call `title_sort`/`uuid4`/etc., so a
        // second, functions-less connection here would fail on them.
        crate::schema_upgrades::SchemaUpgrade::upgrade_to_latest(&mut conn, &library_path)?;

        // Port of `DB.__init__`'s legacy author-sort trigger fixup, run
        // unconditionally on every open (not just for new libraries).
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS author_insert_trg;
             CREATE TEMP TRIGGER author_insert_trg
                 AFTER INSERT ON authors
                 BEGIN
                 UPDATE authors SET sort=author_to_author_sort(NEW.name) WHERE id=NEW.id;
             END;
             DROP TRIGGER IF EXISTS author_update_trg;
             CREATE TEMP TRIGGER author_update_trg
                 BEFORE UPDATE ON authors
                 BEGIN
                 UPDATE authors SET sort=author_to_author_sort(NEW.name)
                 WHERE id=NEW.id AND name <> NEW.name;
             END;
             UPDATE authors SET sort=author_to_author_sort(name) WHERE sort IS NULL;",
        )?;

        let backend = Backend {
            library_path,
            db_path,
            conn: Arc::new(Mutex::new(conn)),
            #[allow(deprecated)]
            prefs: HashMap::new(),
            write_handle: Arc::new(Mutex::new(None)),
        };

        // Port of `DB.__init__`'s `self.library_id` access: "Guarantee
        // that the library_id is set."
        backend.library_id()?;

        Ok(backend)
    }

    /// The real [`crate::library_handle::LibraryHandle`] for this
    /// library (issue #93's crate-wide write-path retrofit) --
    /// opened, and its exclusive writer lock acquired, on first call;
    /// every subsequent call (including from any clone of this
    /// `Backend`, since `write_handle` is `Arc`-shared) returns the
    /// same handle rather than attempting a second, doomed-to-fail
    /// `open()`. Call this only from a path that is actually about to
    /// write to the library -- it's the real exclusive lock from §7,
    /// not a formality.
    pub fn write_handle(
        &self,
    ) -> Result<Arc<crate::library_handle::LibraryHandle>, crate::library_handle::LibraryHandleError>
    {
        let mut guard = self.write_handle.lock().unwrap();
        if let Some(handle) = guard.as_ref() {
            return Ok(Arc::clone(handle));
        }
        let handle = Arc::new(crate::library_handle::LibraryHandle::open(
            &self.library_path,
        )?);
        *guard = Some(Arc::clone(&handle));
        Ok(handle)
    }

    /// Port of `DB.library_id` (get-or-create): the UUID for this
    /// library, stored in the dedicated `library_id` table.
    pub fn library_id(&self) -> SqlResult<String> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<String> = conn
            .query_row("SELECT uuid FROM library_id", [], |row| row.get(0))
            .ok();
        if let Some(id) = existing {
            return Ok(id);
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        conn.execute_batch(&format!(
            "DELETE FROM library_id; INSERT INTO library_id (uuid) VALUES ('{new_id}');"
        ))?;
        Ok(new_id)
    }

    /// Port of `DBPrefs.__getitem__`/`raw_to_object`: the JSON-decoded
    /// value for `key`, or `None` if unset. Real preference values are
    /// arbitrary JSON (strings, numbers, lists, objects), not always
    /// strings -- callers that need a `String` should match on the
    /// returned [`JsonValue`].
    pub fn get_pref(&self, key: &str) -> Option<JsonValue> {
        let conn = self.conn.lock().unwrap();
        let raw: String = conn
            .query_row("SELECT val FROM preferences WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Port of `DBPrefs.__setitem__`/`to_raw`: JSON-encodes `val` and
    /// inserts or updates the row for `key`.
    pub fn set_pref(&self, key: &str, val: &JsonValue) -> SqlResult<()> {
        let raw = serde_json::to_string(val).map_err(|e| {
            SqlError::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
                Some(format!("failed to encode pref {key}: {e}")),
            )
        })?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preferences (key, val) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET val = excluded.val",
            (key, &raw),
        )?;
        Ok(())
    }

    /// Port of `DBPrefs.__delitem__`.
    pub fn delete_pref(&self, key: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM preferences WHERE key = ?", [key])?;
        Ok(())
    }

    /// Port of `DBPrefs.load_from_db`: every stored preference,
    /// JSON-decoded. Entries that fail to decode are skipped, matching
    /// upstream's `except Exception: continue`.
    pub fn all_prefs(&self) -> HashMap<String, JsonValue> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT key, val FROM preferences") {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };
        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let raw: String = row.get(1)?;
            Ok((key, raw))
        });
        let mut out = HashMap::new();
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (key, raw) = row;
                if let Ok(val) = serde_json::from_str(&raw) {
                    out.insert(key, val);
                }
            }
        }
        out
    }

    pub fn field_for(&self, book_id: i32, field_name: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();

        // Allowed fields whitelist to prevent injection
        let sql = match field_name {
            "title" | "sort" | "author_sort" | "isbn" | "path" | "series_index" | "uuid" => {
                format!("SELECT {} FROM books WHERE id = ?", field_name)
            }
            _ => return Ok(None),
        };

        let mut stmt = conn.prepare(&sql)?;
        let result: SqlResult<String> = stmt.query_row([book_id], |row| {
            // Some fields might be NULL or different types, but for now assuming String for simplicity
            // In reality, series_index is REAL.
            if field_name == "series_index" {
                let val: f64 = row.get(0)?;
                Ok(val.to_string())
            } else {
                row.get(0)
            }
        });

        match result {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn update(&self, book_id: i32, field: &str, value: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();

        // Whitelist again
        let sql = match field {
            "title" | "sort" | "author_sort" | "uuid" => {
                format!("UPDATE books SET {} = ? WHERE id = ?", field)
            }
            "series_index" => "UPDATE books SET series_index = ? WHERE id = ?".to_string(),
            _ => return Ok(()), // Unknown field, ignore or error. For now ignore to avoid crashing.
        };

        if field == "series_index" {
            let val = value.parse::<f64>().unwrap_or(1.0); // Default to 1.0 if parse fails? Or error?
                                                           // rusqlite execute with params
            conn.execute(&sql, (val, book_id))?;
        } else {
            conn.execute(&sql, (value, book_id))?;
        }

        Ok(())
    }

    pub fn insert_book(
        &self,
        title: &str,
        sort: &str,
        author_sort: &str,
        uuid: &str,
    ) -> SqlResult<i32> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (title, sort, author_sort, uuid, series_index) VALUES (?, ?, ?, ?, 1.0)",
            (title, sort, author_sort, uuid),
        )?;
        Ok(conn.last_insert_rowid() as i32)
    }

    pub fn get_all_authors(&self) -> SqlResult<HashMap<i32, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM authors")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;

        let mut authors = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            authors.insert(id, name);
        }
        Ok(authors)
    }

    pub fn get_all_series(&self) -> SqlResult<HashMap<i32, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM series")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;

        let mut series = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            series.insert(id, name);
        }
        Ok(series)
    }

    /// Deprecated no-op, kept only for API compatibility with existing
    /// callers. `prefs` is never populated -- use [`Backend::get_pref`]/
    /// [`Backend::all_prefs`] instead, which read the real table.
    #[deprecated(note = "prefs is never populated; use get_pref/all_prefs instead")]
    pub fn load_prefs(&mut self) -> SqlResult<()> {
        Ok(())
    }
}

/// Port of `Connection.__init__`'s custom function/collation/aggregate
/// registration.
pub(crate) fn register_functions(conn: &Connection) -> SqlResult<()> {
    conn.create_collation("PYNOCASE", |a: &str, b: &str| -> Ordering {
        a.to_lowercase().cmp(&b.to_lowercase())
    })?;

    // Approximation of upstream's ICU `sort_key`-based collation -- see
    // the module docs for what's missing (locale-aware, numeral-aware
    // natural sorting).
    conn.create_collation("icucollate", |a: &str, b: &str| -> Ordering {
        a.to_lowercase().cmp(&b.to_lowercase())
    })?;

    conn.create_scalar_function("title_sort", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        let title: String = ctx.get(0)?;
        Ok(real_title_sort(&title))
    })?;

    conn.create_scalar_function(
        "author_to_author_sort",
        1,
        FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let author: String = ctx.get(0)?;
            let author = author.replace('|', ",");
            Ok(real_author_to_author_sort(
                &author, None, None, None, None, None, None,
            ))
        },
    )?;

    conn.create_scalar_function("uuid4", 0, FunctionFlags::SQLITE_UTF8, |_ctx| {
        Ok(uuid::Uuid::new_v4().to_string())
    })?;

    // Dummy function for dynamically created filters (matches upstream:
    // the real filter behavior comes from `create_dynamic_filter`
    // replacing this registration per-name at query time, not from
    // this base implementation).
    conn.create_scalar_function("books_list_filter", 1, FunctionFlags::SQLITE_UTF8, |_ctx| {
        Ok(1i64)
    })?;

    conn.create_aggregate_function(
        "concat",
        1,
        FunctionFlags::SQLITE_UTF8,
        ConcatAgg { sep: "," },
    )?;
    conn.create_aggregate_function(
        "sortconcat",
        2,
        FunctionFlags::SQLITE_UTF8,
        SortConcatAgg { sep: "," },
    )?;
    conn.create_aggregate_function(
        "sortconcat_bar",
        2,
        FunctionFlags::SQLITE_UTF8,
        SortConcatAgg { sep: "|" },
    )?;
    conn.create_aggregate_function(
        "sortconcat_amper",
        2,
        FunctionFlags::SQLITE_UTF8,
        SortConcatAgg { sep: "&" },
    )?;
    conn.create_aggregate_function(
        "identifiers_concat",
        2,
        FunctionFlags::SQLITE_UTF8,
        IdentifiersConcatAgg,
    )?;
    conn.create_aggregate_function(
        "aum_sortconcat",
        4,
        FunctionFlags::SQLITE_UTF8,
        AumSortConcatAgg,
    )?;

    Ok(())
}

/// Port of `Concatenate`: joins non-null values with `sep`.
struct ConcatAgg {
    sep: &'static str,
}

impl Aggregate<Vec<String>, Option<String>> for ConcatAgg {
    fn init(&self, _ctx: &mut Context<'_>) -> SqlResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Vec<String>) -> SqlResult<()> {
        if let Ok(v) = ctx.get::<String>(0) {
            acc.push(v);
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<Vec<String>>,
    ) -> SqlResult<Option<String>> {
        match acc {
            Some(v) if !v.is_empty() => Ok(Some(v.join(self.sep))),
            _ => Ok(None),
        }
    }
}

/// Port of `SortedConcatenate`: `(ndx, value)` pairs, joined by `sep`
/// in ascending `ndx` order.
struct SortConcatAgg {
    sep: &'static str,
}

impl Aggregate<Vec<(i64, String)>, Option<String>> for SortConcatAgg {
    fn init(&self, _ctx: &mut Context<'_>) -> SqlResult<Vec<(i64, String)>> {
        Ok(Vec::new())
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Vec<(i64, String)>) -> SqlResult<()> {
        let ndx: i64 = ctx.get(0)?;
        if let Ok(value) = ctx.get::<String>(1) {
            acc.push((ndx, value));
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<Vec<(i64, String)>>,
    ) -> SqlResult<Option<String>> {
        match acc {
            Some(mut v) if !v.is_empty() => {
                v.sort_by_key(|(ndx, _)| *ndx);
                Ok(Some(
                    v.into_iter()
                        .map(|(_, value)| value)
                        .collect::<Vec<_>>()
                        .join(self.sep),
                ))
            }
            _ => Ok(None),
        }
    }
}

/// Port of `IdentifiersConcat`: `(key, val)` pairs joined as
/// `"key:val,key:val,..."`, in encounter order.
struct IdentifiersConcatAgg;

impl Aggregate<Vec<String>, Option<String>> for IdentifiersConcatAgg {
    fn init(&self, _ctx: &mut Context<'_>) -> SqlResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Vec<String>) -> SqlResult<()> {
        let key: String = ctx.get(0)?;
        let val: String = ctx.get(1)?;
        acc.push(format!("{key}:{val}"));
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<Vec<String>>,
    ) -> SqlResult<Option<String>> {
        Ok(acc.map(|v| v.join(",")))
    }
}

/// Port of `AumSortedConcatenate`: `(ndx, author, sort, link)` tuples.
/// Each tuple becomes `"author:::sort:::link"`; tuples are joined by
/// `:#:` in ascending `ndx` order.
struct AumSortConcatAgg;

impl Aggregate<Vec<(i64, String)>, Option<String>> for AumSortConcatAgg {
    fn init(&self, _ctx: &mut Context<'_>) -> SqlResult<Vec<(i64, String)>> {
        Ok(Vec::new())
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Vec<(i64, String)>) -> SqlResult<()> {
        let ndx: i64 = ctx.get(0)?;
        let author: Option<String> = ctx.get(1)?;
        let sort: String = ctx.get(2).unwrap_or_default();
        let link: String = ctx.get(3).unwrap_or_default();
        if let Some(author) = author {
            acc.push((ndx, format!("{author}:::{sort}:::{link}")));
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<Vec<(i64, String)>>,
    ) -> SqlResult<Option<String>> {
        match acc {
            Some(mut v) if !v.is_empty() => {
                v.sort_by_key(|(ndx, _)| *ndx);
                Ok(Some(
                    v.into_iter()
                        .map(|(_, entry)| entry)
                        .collect::<Vec<_>>()
                        .join(":#:"),
                ))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_library() -> (tempfile::TempDir, Backend) {
        let dir = tempdir().unwrap();
        let backend = Backend::new(dir.path()).expect("Backend::new should succeed");
        (dir, backend)
    }

    #[test]
    fn multiple_backends_over_the_same_directory_never_touch_the_writer_lock() {
        // The whole point of `write_handle` being lazy: opening a
        // `Backend` (or several, over the same library) must stay
        // safe and lock-free as long as nothing actually writes --
        // read-only CLI commands, and this crate's own tests that
        // construct more than one `Backend`/`Cache` over one
        // directory, both depend on this.
        let dir = tempdir().unwrap();
        let _first = Backend::new(dir.path()).unwrap();
        let _second = Backend::new(dir.path()).unwrap();
        assert!(!dir.path().join(".calibre-oxide").exists());
    }

    #[test]
    fn write_handle_is_cached_across_calls_on_the_same_backend() {
        let (_dir, backend) = open_test_library();
        let first = backend.write_handle().unwrap();
        let second = backend.write_handle().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn write_handle_is_shared_across_clones_of_the_same_backend() {
        let (_dir, backend) = open_test_library();
        let first = backend.write_handle().unwrap();
        let second = backend.clone().write_handle().unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "a clone must reuse the already-open handle, not attempt a second (doomed) open()"
        );
    }

    #[test]
    fn a_second_independent_backend_cannot_get_a_write_handle_while_the_first_holds_one() {
        let dir = tempdir().unwrap();
        let first = Backend::new(dir.path()).unwrap();
        let _handle = first.write_handle().unwrap();

        let second = Backend::new(dir.path()).unwrap();
        let result = second.write_handle();
        assert!(matches!(
            result,
            Err(crate::library_handle::LibraryHandleError::AlreadyLocked)
        ));
    }

    #[test]
    fn new_library_creates_the_real_calibre_schema() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 26, "bundled schema sets user_version=26");

        for table in [
            "books",
            "authors",
            "series",
            "publishers",
            "tags",
            "ratings",
            "identifiers",
            "comments",
            "preferences",
            "library_id",
            "custom_columns",
            "annotations",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |row| row.get::<_, i64>(0).map(|_| true),
                )
                .unwrap_or(false);
            assert!(exists, "expected table {table} to exist");
        }
    }

    #[test]
    fn reopening_an_existing_library_does_not_error_or_redo_schema_init() {
        let dir = tempdir().unwrap();
        let backend1 = Backend::new(dir.path()).unwrap();
        let id1 = backend1.library_id().unwrap();
        drop(backend1);

        // Second open against the same on-disk library must not try to
        // CREATE TABLE again (user_version is already 26).
        let backend2 = Backend::new(dir.path()).unwrap();
        let id2 = backend2.library_id().unwrap();
        assert_eq!(id1, id2, "library_id must be stable across reopens");
    }

    #[test]
    fn library_id_is_a_real_uuid_stored_in_the_library_id_table() {
        let (_dir, backend) = open_test_library();
        let id = backend.library_id().unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "{id}");

        let conn = backend.conn.lock().unwrap();
        let stored: String = conn
            .query_row("SELECT uuid FROM library_id", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, id);
    }

    #[test]
    fn title_sort_function_matches_the_real_port() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        let result: String = conn
            .query_row("SELECT title_sort('The Great Gatsby')", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(result, real_title_sort("The Great Gatsby"));
        assert_eq!(result, "Great Gatsby, The");
    }

    #[test]
    fn author_to_author_sort_function_swaps_pipe_for_comma_first() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        let result: String = conn
            .query_row(
                "SELECT author_to_author_sort('Jane Doe|Extra')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // Matches `_author_to_author_sort`: `|` becomes `,` before the
        // real conversion runs.
        let expected =
            real_author_to_author_sort("Jane Doe,Extra", None, None, None, None, None, None);
        assert_eq!(result, expected);
    }

    #[test]
    fn uuid4_function_returns_a_fresh_valid_uuid_each_call() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        let a: String = conn
            .query_row("SELECT uuid4()", [], |row| row.get(0))
            .unwrap();
        let b: String = conn
            .query_row("SELECT uuid4()", [], |row| row.get(0))
            .unwrap();
        assert!(uuid::Uuid::parse_str(&a).is_ok());
        assert!(uuid::Uuid::parse_str(&b).is_ok());
        assert_ne!(a, b);
    }

    #[test]
    fn pynocase_collation_is_unicode_case_insensitive() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE t (v TEXT);
             INSERT INTO t VALUES ('Banana'), ('apple'), ('Cherry'), ('ÉCLAIR');",
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT v FROM t ORDER BY v COLLATE PYNOCASE")
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec!["apple", "Banana", "Cherry", "ÉCLAIR"]);
    }

    #[test]
    fn concat_and_sortconcat_aggregates_match_upstream_semantics() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE t (ndx INTEGER, v TEXT);
             INSERT INTO t VALUES (2, 'b'), (0, 'a'), (1, NULL);",
        )
        .unwrap();

        let concat: String = conn
            .query_row("SELECT concat(v) FROM t", [], |row| row.get(0))
            .unwrap();
        // NULLs are skipped by `step`, so only 'b' and 'a' are joined,
        // in row-encounter order (not sorted -- that's `sortconcat`).
        assert_eq!(concat, "b,a");

        let sortconcat: String = conn
            .query_row("SELECT sortconcat(ndx, v) FROM t", [], |row| row.get(0))
            .unwrap();
        // NULL value at ndx=1 is skipped; remaining pairs sorted by ndx.
        assert_eq!(sortconcat, "a,b");
    }

    #[test]
    fn identifiers_concat_aggregate_joins_key_val_pairs() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE t (k TEXT, v TEXT);
             INSERT INTO t VALUES ('isbn', '123'), ('doi', '456');",
        )
        .unwrap();
        let result: String = conn
            .query_row("SELECT identifiers_concat(k, v) FROM t", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(result, "isbn:123,doi:456");
    }

    #[test]
    fn aum_sortconcat_aggregate_matches_upstream_semantics() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE t (ndx INTEGER, author TEXT, sort TEXT, link TEXT);
             INSERT INTO t VALUES (1, 'Bob', 'Bob', ''), (0, 'Alice', 'Alice', 'http://x');",
        )
        .unwrap();
        let result: String = conn
            .query_row(
                "SELECT aum_sortconcat(ndx, author, sort, link) FROM t",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(result, "Alice:::Alice:::http://x:#:Bob:::Bob:::");
    }

    #[test]
    fn pref_round_trips_real_json_typed_values_not_just_strings() {
        let (_dir, backend) = open_test_library();

        backend
            .set_pref("a_string", &JsonValue::String("hello".to_string()))
            .unwrap();
        backend.set_pref("a_number", &JsonValue::from(42)).unwrap();
        backend
            .set_pref("a_list", &JsonValue::from(vec![1, 2, 3]))
            .unwrap();

        assert_eq!(
            backend.get_pref("a_string"),
            Some(JsonValue::String("hello".to_string()))
        );
        assert_eq!(backend.get_pref("a_number"), Some(JsonValue::from(42)));
        assert_eq!(
            backend.get_pref("a_list"),
            Some(JsonValue::from(vec![1, 2, 3]))
        );
        assert_eq!(backend.get_pref("nonexistent"), None);

        backend.delete_pref("a_string").unwrap();
        assert_eq!(backend.get_pref("a_string"), None);
    }

    #[test]
    fn pref_update_overwrites_the_existing_value() {
        let (_dir, backend) = open_test_library();
        backend.set_pref("k", &JsonValue::from(1)).unwrap();
        backend.set_pref("k", &JsonValue::from(2)).unwrap();
        assert_eq!(backend.get_pref("k"), Some(JsonValue::from(2)));

        let conn = backend.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM preferences WHERE key='k'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "update must not insert a duplicate row");
    }

    #[test]
    fn all_prefs_returns_every_stored_json_value() {
        let (_dir, backend) = open_test_library();
        backend.set_pref("x", &JsonValue::from(1)).unwrap();
        backend.set_pref("y", &JsonValue::from("z")).unwrap();
        let all = backend.all_prefs();
        assert_eq!(all.get("x"), Some(&JsonValue::from(1)));
        assert_eq!(all.get("y"), Some(&JsonValue::String("z".to_string())));
    }

    #[test]
    fn field_for_and_insert_book_work_against_the_real_schema() {
        let (_dir, backend) = open_test_library();
        let id = backend
            .insert_book("Some Title", "Title, Some", "Doe, Jane", "uuid-1")
            .unwrap();
        assert_eq!(
            backend.field_for(id, "title").unwrap(),
            Some("Some Title".to_string())
        );
        backend.update(id, "title", "New Title").unwrap();
        assert_eq!(
            backend.field_for(id, "title").unwrap(),
            Some("New Title".to_string())
        );
    }

    #[test]
    fn author_insert_trigger_computes_sort_automatically() {
        let (_dir, backend) = open_test_library();
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO authors (name) VALUES ('Doe, Jane')", [])
            .unwrap();
        let sort: String = conn
            .query_row(
                "SELECT sort FROM authors WHERE name='Doe, Jane'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sort,
            real_author_to_author_sort("Doe, Jane", None, None, None, None, None, None,)
        );
    }
}
