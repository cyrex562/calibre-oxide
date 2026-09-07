//! Port of `old_src/src/calibre/db/fts/connect.py`'s `FTS` class
//! (issue #226, a #201 follow-up): the full-text-search database --
//! its own attached `full-text-search.db` file, real dirty-tracking,
//! text indexing, and querying.
//!
//! # Scope of this pass
//!
//! Real, matching `fts_sqlite.sql`/`fts_triggers.sql`/`connect.py`:
//! schema creation (`dirtied_formats`, `books_text`, two FTS5 virtual
//! tables), the triggers that keep the FTS index in sync with
//! `books_text` and that dirty a book's formats when `main.data`
//! changes (or clear them when a book/format is deleted), dirty-set
//! management (`dirty_book`/`remove_dirty`/`number_dirtied`/
//! `all_currently_dirty`/`clear_all_dirty`/`dirty_existing`),
//! `unindex`, `add_text` (real signature: an error message, empty
//! text, or real text each take a different real path, matching
//! upstream), `vacuum`, and `search` (real dynamic SQL: single-id
//! vs. multi-id `restrict_to_book_ids` via a temp table, `snippet()`/
//! `highlight()` when requested, real `FtsQueryError` on a malformed
//! MATCH query).
//!
//! # Disclosed simplifications
//!
//! - **Real `calibre`/`porter` FTS5 tokenizers, as of issue #566.**
//!   `books_fts`/`books_fts_stemmed` now use the real
//!   `tokenize = 'calibre remove_diacritics 2'` /
//!   `'porter calibre remove_diacritics 2'` clauses from real
//!   upstream's bundled `fts_sqlite.sql`, registered via
//!   `crate::sqlite_extension::register_fts5_tokenizers`. **Correction
//!   to a stale claim this doc previously made**: it used to say
//!   "nothing here needs to change" once tokenizer registration
//!   landed, on the theory that the DDL already matched upstream --
//!   that was wrong. Neither virtual table had a `tokenize=` clause AT
//!   ALL before #566 (both silently used FTS5's default `unicode61`),
//!   so `use_stemming` in [`FtsConnection::search`] previously
//!   selected between two byte-for-byte IDENTICAL tables, not just two
//!   tables sharing the same (wrong) tokenizer. Confirmed by reading
//!   real upstream's actual bundled `resources/fts_sqlite.sql`
//!   directly, not by re-trusting this comment's own prior claim.
//! - **No background worker pool.** `fts/pool.py`'s `Pool`/`Worker`
//!   spawn a subprocess per book/format to extract text
//!   out-of-process (via `calibre.db.fts.text.main`) and feed results
//!   back through a supervisor thread -- this crate has no
//!   ebook-text-extraction pipeline to spawn in the first place, so
//!   `queue_job`/`get_next_fts_job`/`commit_result`'s job-queueing
//!   halves are not ported. [`FtsConnection::add_text`] (the actual
//!   database write once text is available) *is* real and is exactly
//!   what a future extraction pipeline would call.
//! - **`unicode_normalize`** (NFC normalization of the query string
//!   before matching) is not applied -- no equivalent helper exists
//!   in this crate outside `sqlite_extension`'s tokenizer, which isn't
//!   wired into the query path here either.

use crate::constants::FTS_DB_NAME;
use crate::errors::FtsQueryError;
use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One [`FtsConnection::search`] hit -- upstream's per-result dict
/// (`id`/`book_id`/`format`/`text`).
#[derive(Debug, Clone, PartialEq)]
pub struct FtsSearchResult {
    pub id: i32,
    pub book_id: i32,
    pub format: String,
    /// `None` when `return_text` was false.
    pub text: Option<String>,
}

pub struct FtsConnection {
    conn: Arc<Mutex<Connection>>,
    fts_db_path: PathBuf,
}

impl FtsConnection {
    pub fn new(conn: Arc<Mutex<Connection>>, main_db_path: &Path) -> Self {
        let fts_db_path = main_db_path
            .parent()
            .unwrap_or(main_db_path)
            .join(FTS_DB_NAME);
        FtsConnection { conn, fts_db_path }
    }

    /// Attaches `full-text-search.db` (if not already) and creates the
    /// real schema/triggers -- port of `FTS.initialize`/
    /// `SchemaUpgrade.__init__` (this crate has only ever had schema
    /// version 1, so there's no upgrade ladder to walk, just the
    /// initial creation).
    pub fn initialize(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let attached: i32 = conn
            .query_row(
                "SELECT count(*) FROM pragma_database_list WHERE name='fts_db'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if attached == 0 {
            conn.execute(
                "ATTACH DATABASE ? AS fts_db",
                [self.fts_db_path.to_str().unwrap()],
            )?;
            // docs/FAULT_TOLERANCE.md §3 (issue #260): each attached
            // sidecar database needs its own journal_mode/synchronous
            // pragma -- unqualified PRAGMA journal_mode only applies
            // to `main`, not to an ATTACHed schema.
            conn.execute_batch("PRAGMA fts_db.journal_mode=WAL; PRAGMA fts_db.synchronous=FULL;")?;

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS fts_db.dirtied_formats (
                    id INTEGER PRIMARY KEY,
                    book INTEGER NOT NULL,
                    format TEXT NOT NULL COLLATE NOCASE,
                    in_progress INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(book, format)
                );
                CREATE TABLE IF NOT EXISTS fts_db.books_text (
                    id INTEGER PRIMARY KEY,
                    book INTEGER NOT NULL,
                    timestamp REAL NOT NULL,
                    format TEXT NOT NULL COLLATE NOCASE,
                    format_hash TEXT NOT NULL COLLATE NOCASE,
                    format_size INTEGER NOT NULL DEFAULT 0,
                    searchable_text TEXT NOT NULL DEFAULT '',
                    text_size INTEGER NOT NULL DEFAULT 0,
                    text_hash TEXT NOT NULL COLLATE NOCASE DEFAULT '',
                    err_msg TEXT DEFAULT '',
                    UNIQUE(book, format)
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS fts_db.books_fts USING fts5(
                    searchable_text, content = 'books_text', content_rowid = 'id',
                    tokenize = 'calibre remove_diacritics 2'
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS fts_db.books_fts_stemmed USING fts5(
                    searchable_text, content = 'books_text', content_rowid = 'id',
                    tokenize = 'porter calibre remove_diacritics 2'
                );
                CREATE TRIGGER IF NOT EXISTS fts_db.books_fts_insert_trg AFTER INSERT ON books_text BEGIN
                    INSERT INTO books_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    INSERT INTO books_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    DELETE FROM dirtied_formats WHERE book=NEW.book AND format=NEW.format;
                END;
                CREATE TRIGGER IF NOT EXISTS fts_db.books_fts_delete_trg AFTER DELETE ON books_text BEGIN
                    INSERT INTO books_fts(books_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO books_fts_stemmed(books_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                END;
                CREATE TRIGGER IF NOT EXISTS fts_db.books_fts_update_trg AFTER UPDATE ON books_text BEGIN
                    INSERT INTO books_fts(books_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO books_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    INSERT INTO books_fts_stemmed(books_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO books_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    DELETE FROM dirtied_formats WHERE book=NEW.book AND format=NEW.format;
                END;",
            )?;
        }

        // Upstream's `fts_triggers.sql` triggers are TEMP -- scoped to
        // the connection, so they're (re-)created every `initialize()`
        // call, same as here. Trigger bodies must reference
        // `books_text`/`dirtied_formats` unqualified (SQLite rejects a
        // `database.table`-qualified name inside a trigger's INSERT/
        // UPDATE/DELETE statements, even though upstream's real
        // `fts_triggers.sql` writes them qualified -- apparently
        // accepted by whatever SQLite build apsw links against, but
        // not by this crate's bundled one). Unqualified still resolves
        // correctly to the `fts_db`-attached tables, since no
        // same-named table exists in `main`/`temp`.
        conn.execute_batch(
            "CREATE TEMP TRIGGER IF NOT EXISTS fts_db_book_deleted_trg AFTER DELETE ON main.books BEGIN
                DELETE FROM books_text WHERE book=OLD.id;
                DELETE FROM dirtied_formats WHERE book=OLD.id;
            END;
            CREATE TEMP TRIGGER IF NOT EXISTS fts_db_format_deleted_trg AFTER DELETE ON main.data BEGIN
                DELETE FROM books_text WHERE book=OLD.book AND format=OLD.format;
                DELETE FROM dirtied_formats WHERE book=OLD.book AND format=OLD.format;
            END;
            CREATE TEMP TRIGGER IF NOT EXISTS fts_db_format_added_trg AFTER INSERT ON main.data BEGIN
                INSERT OR IGNORE INTO dirtied_formats(book, format) VALUES (NEW.book, NEW.format);
            END;
            CREATE TEMP TRIGGER IF NOT EXISTS fts_db_format_updated_trg AFTER UPDATE ON main.data BEGIN
                INSERT OR IGNORE INTO dirtied_formats(book, format) VALUES (NEW.book, NEW.format);
            END;",
        )?;
        Ok(())
    }

    /// Marks every format currently in `main.data` as dirty -- used
    /// when FTS indexing is freshly enabled on a library that already
    /// has books in it.
    pub fn dirty_existing(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO fts_db.dirtied_formats(book, format) SELECT book, format FROM main.data",
            [],
        )?;
        Ok(())
    }

    pub fn number_dirtied(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM fts_db.dirtied_formats", [], |r| {
            r.get(0)
        })
    }

    /// Number of formats already indexed (`books_text` row count) --
    /// combined with [`FtsConnection::number_dirtied`], this is the
    /// `left`/`total` pair upstream's `fts_indexing_progress` reports
    /// (`total = left + indexed`).
    pub fn number_indexed(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM fts_db.books_text", [], |r| r.get(0))
    }

    pub fn all_currently_dirty(&self) -> SqlResult<Vec<(i32, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT book, format FROM fts_db.dirtied_formats ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }

    pub fn clear_all_dirty(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM fts_db.dirtied_formats", [])?;
        Ok(())
    }

    pub fn vacuum(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("VACUUM fts_db")
    }

    pub fn remove_dirty(&self, book_id: i32, fmt: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM fts_db.dirtied_formats WHERE book=?1 AND format=?2",
            (book_id, fmt.to_uppercase()),
        )?;
        Ok(())
    }

    pub fn dirty_book(&self, book_id: i32, fmts: &[&str]) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        for fmt in fmts {
            conn.execute(
                "INSERT OR IGNORE INTO fts_db.dirtied_formats (book, format) VALUES (?1, ?2)",
                (book_id, fmt.to_uppercase()),
            )?;
        }
        Ok(())
    }

    /// Removes `book_id`'s indexed text -- every format if `fmt` is
    /// `None`, otherwise just that one.
    pub fn unindex(&self, book_id: i32, fmt: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        match fmt {
            None => conn.execute("DELETE FROM fts_db.books_text WHERE book=?1", (book_id,))?,
            Some(fmt) => conn.execute(
                "DELETE FROM fts_db.books_text WHERE book=?1 AND format=?2",
                (book_id, fmt.to_uppercase()),
            )?,
        };
        Ok(())
    }

    /// Port of `FTS.add_text`: records extracted text (or an
    /// extraction failure) for a book's format, real timestamping via
    /// the caller-supplied Unix-epoch-seconds `timestamp`. Exactly one
    /// of `text`/`err_msg` should be set; an empty `text` with no
    /// `err_msg` just clears the dirty row without indexing anything
    /// (upstream's "nothing to index" branch).
    #[allow(clippy::too_many_arguments)]
    pub fn add_text(
        &self,
        book_id: i32,
        fmt: &str,
        timestamp: f64,
        text: Option<&str>,
        text_hash: &str,
        fmt_size: i64,
        fmt_hash: &str,
        err_msg: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let fmt = fmt.to_uppercase();
        if let Some(err_msg) = err_msg {
            conn.execute(
                "INSERT OR REPLACE INTO fts_db.books_text \
                 (book, timestamp, format, format_size, format_hash, err_msg) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (book_id, timestamp, &fmt, fmt_size, fmt_hash, err_msg),
            )?;
        } else if let Some(text) = text.filter(|t| !t.is_empty()) {
            conn.execute(
                "INSERT OR REPLACE INTO fts_db.books_text \
                 (book, timestamp, format, format_size, format_hash, searchable_text, text_size, text_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (book_id, timestamp, &fmt, fmt_size, fmt_hash, text, text.len() as i64, text_hash),
            )?;
        } else {
            conn.execute(
                "DELETE FROM fts_db.dirtied_formats WHERE book=?1 AND format=?2",
                (book_id, &fmt),
            )?;
        }
        Ok(())
    }

    /// Port of `FTS.search`. `restrict_to_book_ids` mirrors upstream's
    /// single-id-inline-vs-temp-table optimization; `Some(&empty set)`
    /// returns no results immediately (matching upstream's `if
    /// restrict_to_book_ids is not None and not restrict_to_book_ids:
    /// return`), while `None` means unrestricted.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        fts_engine_query: &str,
        use_stemming: bool,
        highlight: Option<(&str, &str)>,
        snippet_size: Option<usize>,
        restrict_to_book_ids: Option<&HashSet<i32>>,
        return_text: bool,
    ) -> Result<Vec<FtsSearchResult>, FtsQueryError> {
        if let Some(ids) = restrict_to_book_ids {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
        }
        let fts_table = if use_stemming {
            "books_fts_stemmed"
        } else {
            "books_fts"
        };

        let mut text_col = String::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();
        if return_text {
            text_col = if let Some((start, end)) = highlight {
                params.push(start.to_string().into());
                params.push(end.to_string().into());
                match snippet_size {
                    Some(n) => format!(
                        ", snippet(\"{fts_table}\", 0, ?, ?, '…', {})",
                        n.clamp(1, 64)
                    ),
                    None => format!(", highlight(\"{fts_table}\", 0, ?, ?)"),
                }
            } else {
                ", books_text.searchable_text".to_string()
            };
        }

        let mut query = format!(
            "SELECT books_text.id, books_text.book, books_text.format{text_col} FROM books_text"
        );
        query.push_str(&format!(
            " JOIN {fts_table} ON fts_db.books_text.id = {fts_table}.rowid WHERE"
        ));

        let conn = self.conn.lock().unwrap();
        let mut temp_table_name = String::new();
        if let Some(ids) = restrict_to_book_ids {
            if ids.len() == 1 {
                let only_book = *ids.iter().next().unwrap();
                query.push_str(&format!(" fts_db.books_text.book == {only_book} AND"));
            } else {
                static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                temp_table_name = format!("fts_restrict_search_{n}");
                conn.execute(
                    &format!("CREATE TABLE temp.{temp_table_name}(x INTEGER)"),
                    [],
                )
                .map_err(|e| sql_err(fts_engine_query, "CREATE TABLE temp", &e))?;
                for &id in ids {
                    conn.execute(
                        &format!("INSERT INTO temp.{temp_table_name} VALUES (?1)"),
                        [id],
                    )
                    .map_err(|e| sql_err(fts_engine_query, "INSERT INTO temp", &e))?;
                }
                query.push_str(&format!(
                    " fts_db.books_text.book IN temp.{temp_table_name} AND"
                ));
            }
        }
        query.push_str(&format!(" \"{fts_table}\" MATCH ?"));
        params.push(fts_engine_query.to_string().into());
        query.push_str(&format!(" ORDER BY {fts_table}.rank"));

        let result = (|| -> SqlResult<Vec<FtsSearchResult>> {
            let mut stmt = conn.prepare(&query)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(FtsSearchResult {
                    id: row.get(0)?,
                    book_id: row.get(1)?,
                    format: row.get(2)?,
                    text: if return_text {
                        row.get::<_, Option<String>>(3)?
                    } else {
                        None
                    },
                })
            })?;
            rows.collect()
        })();

        if !temp_table_name.is_empty() {
            let _ = conn.execute(&format!("DROP TABLE temp.{temp_table_name}"), []);
        }

        result.map_err(|e| sql_err(fts_engine_query, &query, &e))
    }

    /// Whether a row with this exact `(book, format, format_size,
    /// format_hash)` is already indexed -- used to skip re-extracting
    /// text for a format that hasn't actually changed.
    pub fn already_indexed(
        &self,
        book_id: i32,
        fmt: &str,
        fmt_size: i64,
        fmt_hash: &str,
    ) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i32> = conn
            .query_row(
                "SELECT id FROM fts_db.books_text WHERE book=?1 AND format=?2 AND format_size=?3 AND format_hash=?4",
                (book_id, fmt.to_uppercase(), fmt_size, fmt_hash),
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }
}

fn sql_err(query: &str, sql_statement: &str, e: &rusqlite::Error) -> FtsQueryError {
    FtsQueryError {
        query: query.to_string(),
        sql_statement: sql_statement.to_string(),
        apsw_error: e.to_string(),
    }
}
