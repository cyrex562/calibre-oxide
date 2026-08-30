//! `Library` -- the CLI's data-access layer (`src/cli/cmd_*.rs`
//! operates on this, not `Cache`/`Backend` directly).
//!
//! # Schema unification (issue #201 follow-up)
//!
//! Until this pass, `Library` created its own ad hoc, minimal 6-table
//! schema (`init_schema`, since removed) and registered no-op stub SQL
//! functions (`title_sort`/`author_to_author_sort` that just echoed
//! their input back). That schema was missing every table
//! `crate::cache::Cache`/`crate::search::search` (issues #204/#210)
//! depend on -- `comments`, `series`, `tags`, `identifiers`,
//! `books_series_link`, `books_tags_link`, `library_id`, and more --
//! which is why the CLI's real query-syntax search engine was
//! unreachable from any CLI command: `Library` and `Backend`/`Cache`
//! were two entirely disconnected data-access layers over
//! incompatible schemas. See the #201 audit for the full writeup.
//!
//! `Library::open`/`create`/`open_test` now all delegate schema
//! creation and SQL function/collation registration to
//! [`crate::backend::Backend::new`] -- the same real, bundled
//! `metadata_sqlite.sql` DDL and real function ports `Cache` uses.
//! [`Library::search`] shares that same live connection with a
//! [`crate::cache::Cache`]/[`crate::search::search`] call (via
//! `Backend`'s cheap `Clone`, not a second connection), so the CLI's
//! `search` subcommand now gets the real query-syntax engine
//! (`author:`, `tag:`, date ranges, boolean operators, `AND`/`OR`/
//! `NOT`) instead of the old `title LIKE '%q%'` stub.
//!
//! One real behavioral consequence of the real schema: it includes
//! `books_insert_trg`, which unconditionally overwrites `sort` and
//! `uuid` on every `INSERT INTO books` (`sort=title_sort(NEW.title),
//! uuid=uuid4()`) -- exactly what real calibre does. Callers that want
//! a *specific* `uuid` preserved (`add_book_db_entry`, used by
//! `restore.rs`'s OPF-driven restore) can't set it via the `INSERT`
//! itself anymore; it now does an explicit `UPDATE` afterward, the
//! same pattern real calibre's own restore path uses.
//!
//! [`Library::open_test`] now backs its database with a real temp
//! directory (auto-cleaned via [`TestDirGuard`]'s `Drop`) instead of
//! an in-memory `:memory:` connection -- `Backend::new` always creates
//! a real file at `<library_path>/metadata.db`, and this crate has no
//! separate in-memory code path for it. A side effect: every method
//! that used to special-case `self.path == PathBuf::from(":memory:")`
//! to skip real file operations during tests now always does them for
//! real, which is strictly more faithful test coverage, not less.
//!
//! # Not unified in this pass
//!
//! `Library` still owns real functionality `Cache`/`Backend` has not
//! grown yet -- all filesystem book/format/cover management
//! (folder-per-book layout, rename-on-metadata-change, format file
//! add/remove, cover copy, clone-library, delete-with-cleanup) and all
//! custom-column support (dynamic `custom_column_N` tables). None of
//! that has been ported to `Cache` yet (`cache.rs`'s own module docs
//! list custom columns as not-yet-ported), so `Library` keeps doing it
//! itself, just against the real schema now instead of its own ad hoc
//! one. Fully collapsing `Library` into `Cache` -- so the CLI operates
//! on one data-access layer instead of two -- would require porting
//! that functionality first; out of scope here.

use crate::backend::Backend;
use crate::book::Book;
use calibre_ebooks::metadata::MetaInformation;
use rusqlite::{Connection, OptionalExtension, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LibraryError {
    #[error("Database connection error: {0}")]
    Connection(#[from] rusqlite::Error),
    #[error("Library path does not exist")]
    InvalidPath,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Transaction error: {0}")]
    Transaction(String),
}

/// Auto-cleans up [`Library::open_test`]'s backing temp directory when
/// the `Library` (and this guard along with it) drops.
struct TestDirGuard(PathBuf);

impl Drop for TestDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct Library {
    backend: Backend,
    path: PathBuf,
    _test_dir: Option<TestDirGuard>,
}

impl Library {
    /// Opens an existing library. Errors if `metadata.db` doesn't
    /// exist yet -- use [`Library::create`] for a brand-new one.
    pub fn open(path: PathBuf) -> Result<Self, LibraryError> {
        let db_path = path.join("metadata.db");
        if !db_path.exists() {
            return Err(LibraryError::InvalidPath);
        }
        let backend = Backend::new(&path)?;
        Ok(Library {
            backend,
            path,
            _test_dir: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Locks and returns the underlying connection. Callers that need
    /// to run raw SQL (mostly test setup) can chain straight off this,
    /// e.g. `lib.conn().execute(...)`, same as before -- the returned
    /// guard derefs to `&Connection`/`&mut Connection`.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.backend.conn.lock().unwrap()
    }

    /// Opens a fresh library backed by a real (auto-cleaned-up) temp
    /// directory, for tests. See this module's docs for why it's a
    /// real directory now rather than an in-memory connection.
    pub fn open_test() -> Result<Self, LibraryError> {
        let path =
            std::env::temp_dir().join(format!("calibre_oxide_libtest_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        let backend = Backend::new(&path)?;
        Ok(Library {
            backend,
            path: path.clone(),
            _test_dir: Some(TestDirGuard(path)),
        })
    }

    /// Create a new library database at the specified path.
    /// Fails if database already exists.
    pub fn create(path: PathBuf) -> Result<Self, LibraryError> {
        let db_path = path.join("metadata.db");
        if db_path.exists() {
            return Err(LibraryError::Transaction(
                "Database already exists".to_string(),
            ));
        }
        let backend = Backend::new(&path)?;
        Ok(Library {
            backend,
            path,
            _test_dir: None,
        })
    }

    pub fn insert_test_book(&self, title: &str) -> Result<(), LibraryError> {
        self.conn().execute(
            "INSERT INTO books (title, author_sort, has_cover, series_index, path)
             VALUES (?1, 'Author', 0, 1.0, '')",
            (title,),
        )?;
        Ok(())
    }

    pub fn book_count(&self) -> Result<i32, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM books")?;
        let count: i32 = stmt.query_row([], |row| row.get(0))?;
        Ok(count)
    }

    pub fn list_books(&self) -> Result<Vec<Book>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, sort, timestamp, pubdate, series_index, author_sort, isbn, lccn, path, has_cover, uuid
             FROM books"
        )?;

        let book_iter = stmt.query_map([], |row| {
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                sort: row.get(2)?,
                timestamp: row.get(3)?,
                pubdate: row.get(4)?,
                series_index: row.get(5)?,
                author_sort: row.get(6)?,
                isbn: row.get(7)?,
                lccn: row.get(8)?,
                path: row.get(9)?,
                has_cover: row.get::<_, i32>(10)? != 0,
                uuid: row.get(11)?,
            })
        })?;

        let mut books = Vec::new();
        for book in book_iter {
            books.push(book?);
        }
        Ok(books)
    }

    pub fn get_cover_path(&self, book: &Book) -> Option<PathBuf> {
        if book.has_cover {
            Some(self.path.join(&book.path).join("cover.jpg"))
        } else {
            None
        }
    }

    pub fn get_default_book_file(&self, book: &Book) -> Option<PathBuf> {
        let dir_path = self.path.join(&book.path);
        if !dir_path.exists() {
            return None;
        }

        // Look for EPUB first, then others
        let preferred_exts = ["epub", "mobi", "azw3", "pdf", "txt"];

        if let Ok(entries) = fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if preferred_exts.contains(&ext_lower.as_str()) {
                            return Some(path);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn add_book(
        &mut self,
        source_path: &Path,
        metadata: &MetaInformation,
    ) -> Result<i32, LibraryError> {
        self.as_cache()
            .add_book(source_path, metadata)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    /// Adds a book entry to the database without copying files (used
    /// by `restore_database`).
    pub fn add_book_db_entry(
        &mut self,
        metadata: &MetaInformation,
        rel_path: &str,
    ) -> Result<i32, LibraryError> {
        self.as_cache()
            .add_book_db_entry(metadata, rel_path)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn update_book_metadata(
        &mut self,
        book_id: i32,
        title: &str,
        author: &str,
    ) -> Result<(), LibraryError> {
        self.as_cache()
            .update_book_metadata(book_id, title, author)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn add_format(
        &mut self,
        book_id: i32,
        source_path: &Path,
        format: &str,
        replace: bool,
    ) -> Result<bool, LibraryError> {
        self.as_cache()
            .add_format(book_id, source_path, format, replace)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn update_book_cover(
        &mut self,
        book_id: i32,
        new_cover_path: &Path,
    ) -> Result<(), LibraryError> {
        let data = fs::read(new_cover_path)?;
        crate::covers::set_cover(&self.as_cache(), book_id, &data)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn delete_book(&mut self, book_id: i32) -> Result<(), LibraryError> {
        self.as_cache()
            .delete_book(book_id)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    /// Shares this `Library`'s own live connection with a fresh
    /// [`crate::cache::Cache`] (via `Backend`'s cheap `Clone` -- no
    /// second connection opened), for delegating to `Cache`-side
    /// functionality (real search, real custom columns) instead of
    /// `Library` hand-rolling its own duplicate SQL.
    fn as_cache(&self) -> crate::cache::Cache {
        crate::cache::Cache::from_backend(self.backend.clone())
    }

    pub fn get_custom_column_label_map(
        &self,
    ) -> Result<std::collections::HashMap<String, serde_json::Value>, LibraryError> {
        self.as_cache()
            .custom_column_label_map()
            .map_err(LibraryError::Connection)
    }

    /// Real query-syntax search (issue #210), via [`Library::as_cache`].
    /// See this module's docs for why that engine was previously
    /// unreachable from the CLI.
    pub fn search(&self, query: &str) -> Result<Vec<i32>, LibraryError> {
        crate::search::search(&self.as_cache(), query).map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn get_book(&self, id: i32) -> Result<Option<Book>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, sort, timestamp, pubdate, series_index, author_sort, isbn, lccn, path, has_cover, uuid
             FROM books WHERE id = ?1"
        )?;

        let mut rows = stmt.query([id])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                sort: row.get(2)?,
                timestamp: row.get(3)?,
                pubdate: row.get(4)?,
                series_index: row.get(5)?,
                author_sort: row.get(6)?,
                isbn: row.get(7)?,
                lccn: row.get(8)?,
                path: row.get(9)?,
                has_cover: row.get::<_, i32>(10)? != 0,
                uuid: row.get(11)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn all_book_ids(&self) -> Result<Vec<i32>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM books")?;
        let rows = stmt.query_map([], |row| row.get(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn backup_metadata_to_opf(&self, book_id: i32) -> Result<(), LibraryError> {
        let book_opt = self.get_book(book_id)?;
        let Some(book) = book_opt else {
            return Err(LibraryError::InvalidPath); // Or BookNotFound
        };
        if book.path.is_empty() {
            // Python skips invisible/pathless books rather than erroring.
            return Ok(());
        }
        let cache = Arc::new(Mutex::new(self.as_cache()));
        crate::backup::backup_metadata(&cache, book_id)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn vacuum(&self, vacuum_fts: bool) -> Result<(), LibraryError> {
        self.conn().execute("VACUUM", [])?;
        if vacuum_fts {
            // Placeholder: functionality for FTS vacuum if we have FTS db
        }
        Ok(())
    }

    /// Real tag-browser categories (issue #220): `authors`/`tags`/
    /// `series`/`publisher`/`languages`, each with a real per-item
    /// book count and average rating -- see `categories.rs`'s module
    /// docs for what's not covered (composite/user categories,
    /// hierarchical categories, the `ratings` category itself).
    pub fn get_categories(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<crate::categories::Tag>>, LibraryError> {
        let cats = crate::categories::get_categories(&self.as_cache(), "name", None)
            .map_err(|e| LibraryError::Transaction(e.to_string()))?;
        Ok(cats.into_iter().collect())
    }

    pub fn remove_format(&mut self, book_id: i32, fmt: &str) -> Result<(), LibraryError> {
        if self.get_book(book_id)?.is_none() {
            return Err(LibraryError::Transaction(format!(
                "Book {} not found",
                book_id
            )));
        }
        self.as_cache()
            .remove_format(book_id, fmt)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn all_authors(&self) -> Result<Vec<(i32, String)>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, name FROM authors")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;

        let mut authors = Vec::new();
        for row in rows {
            authors.push(row?);
        }
        Ok(authors)
    }

    pub fn format_files(&self, book_id: i32) -> Result<Vec<(String, String)>, LibraryError> {
        // Query 'data' table for formats
        let conn = self.conn();
        let stmt = conn.prepare("SELECT name, format FROM data WHERE book = ?1");

        match stmt {
            Ok(mut s) => {
                let rows = s.query_map([book_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
                let mut formats = Vec::new();
                for row in rows {
                    formats.push(row?);
                }
                Ok(formats)
            }
            Err(_) => {
                // Return empty if data table doesn't exist or error (or handle properly)
                Ok(Vec::new())
            }
        }
    }

    pub fn clone_to(&self, dest: &Path) -> Result<(), LibraryError> {
        self.as_cache()
            .clone_to(dest)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn has_cover(&self, book_id: i32) -> Result<bool, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT has_cover FROM books WHERE id = ?1")?;
        let has_cover: Option<i32> = stmt.query_row([book_id], |row| row.get(0)).ok();
        Ok(has_cover.unwrap_or(0) != 0)
    }

    pub fn is_case_sensitive(&self) -> bool {
        false
    }

    pub fn add_custom_column(
        &mut self,
        label: &str,
        name: &str,
        datatype: &str,
        is_multiple: bool,
    ) -> Result<i32, LibraryError> {
        self.as_cache()
            .add_custom_column(label, name, datatype, is_multiple)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    pub fn set_custom_column_value(
        &mut self,
        book_id: i32,
        label: &str,
        value: &str,
    ) -> Result<(), LibraryError> {
        self.as_cache()
            .set_custom_column_value(book_id, label, value)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }

    /// A real [`crate::fts::connection::FtsConnection`] over this
    /// library's `full-text-search.db`, sharing this library's live
    /// connection (via `Backend`'s cheap `Clone`, same pattern
    /// [`Library::as_cache`] uses) -- issue #226.
    pub fn fts(&self) -> crate::fts::connection::FtsConnection {
        crate::fts::connection::FtsConnection::new(self.backend.conn.clone(), &self.backend.db_path)
    }

    /// A real [`crate::notes::connection::NotesConnection`] over this
    /// library's `.calnotes/notes.db`, sharing this library's live
    /// connection (same pattern as [`Library::fts`]) -- issue #227.
    pub fn notes(&self) -> crate::notes::connection::NotesConnection {
        crate::notes::connection::NotesConnection::new(self.backend.clone(), &self.path)
    }

    /// A real [`crate::checksums::ChecksumStore`] over this library's
    /// `.calibre-oxide/checksums.db`, sharing this library's live
    /// connection (same pattern as [`Library::fts`]/[`Library::notes`])
    /// -- issue #93 §8.
    pub fn checksums(&self) -> crate::checksums::ChecksumStore {
        crate::checksums::ChecksumStore::new(self.backend.conn.clone(), &self.path)
    }

    /// Whether FTS indexing has been turned on for this library --
    /// port of `is_fts_enabled`, backed by the same `preferences`
    /// table [`Library::get_preference`]/[`Library::set_preference`]
    /// use (a plain string flag, not `Cache`/`Backend`'s separate
    /// JSON-preference storage).
    pub fn is_fts_enabled(&self) -> Result<bool, LibraryError> {
        Ok(self.get_preference("fts.enabled")?.as_deref() == Some("true"))
    }

    /// Port of `enable_fts`: turns FTS indexing on (marking every
    /// existing format dirty so a real indexing pipeline -- not part
    /// of this crate, see `fts/connection.rs`'s module doc -- would
    /// pick them all up) or off.
    pub fn set_fts_enabled(&mut self, enabled: bool) -> Result<(), LibraryError> {
        self.set_preference("fts.enabled", if enabled { "true" } else { "false" })?;
        if enabled {
            self.fts()
                .initialize()
                .map_err(|e| LibraryError::Transaction(e.to_string()))?;
            self.fts()
                .dirty_existing()
                .map_err(|e| LibraryError::Transaction(e.to_string()))?;
        }
        Ok(())
    }

    /// Port of `fts_indexing_progress`'s `(left, total)` -- the real
    /// `rate` (indexing throughput) has no meaning here since this
    /// crate has no background indexing pipeline to measure (see
    /// `fts/connection.rs`'s module doc), so callers that want
    /// upstream's 3-tuple just treat rate as always unavailable.
    pub fn fts_indexing_progress(&self) -> Result<(i64, i64), LibraryError> {
        let fts = self.fts();
        fts.initialize()
            .map_err(|e| LibraryError::Transaction(e.to_string()))?;
        let left = fts
            .number_dirtied()
            .map_err(|e| LibraryError::Transaction(e.to_string()))?;
        let indexed = fts
            .number_indexed()
            .map_err(|e| LibraryError::Transaction(e.to_string()))?;
        Ok((left, left + indexed))
    }

    pub fn get_preference(&self, key: &str) -> Result<Option<String>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT val FROM preferences WHERE key = ?1")?;
        let val: Option<String> = stmt.query_row([key], |row| row.get(0)).optional()?;
        Ok(val)
    }

    pub fn set_preference(&mut self, key: &str, val: &str) -> Result<(), LibraryError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO preferences (key, val) VALUES (?1, ?2)",
            (key, val),
        )?;
        Ok(())
    }

    pub fn get_custom_column_value(
        &self,
        book_id: i32,
        label: &str,
    ) -> Result<Option<String>, LibraryError> {
        self.as_cache()
            .get_custom_column_value(book_id, label)
            .map_err(LibraryError::Connection)
    }

    pub fn get_authors(&self, book_id: i32) -> Result<Vec<String>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.name FROM authors a
             JOIN books_authors_link bal ON a.id = bal.author
             WHERE bal.book = ?1",
        )?;
        let rows = stmt.query_map([book_id], |row| row.get(0))?;

        let mut authors = Vec::new();
        for row in rows {
            authors.push(row?);
        }
        Ok(authors)
    }

    pub fn remove_books(&mut self, ids: &[i32], permanent: bool) -> Result<(), LibraryError> {
        if !permanent {
            // TODO: Implement recycle bin / trash support. Real calibre
            // moves to trash; this crate doesn't have that wired up yet
            // for `Library`, so we do a permanent delete and warn.
            eprintln!("Warning: Trash not supported yet, deleting permanently.");
        }

        for &id in ids {
            self.delete_book(id)?;
        }
        Ok(())
    }

    pub fn set_metadata(
        &mut self,
        book_id: i32,
        field: &str,
        value: &str,
    ) -> Result<(), LibraryError> {
        match field {
            "title" => {
                let authors = self.get_authors(book_id)?;
                let author = authors.first().map(|s| s.as_str()).unwrap_or("Unknown");
                self.update_book_metadata(book_id, value, author)?;
            }
            "author" => {
                let book_opt = self.get_book(book_id)?;
                if let Some(book) = book_opt {
                    self.update_book_metadata(book_id, &book.title, value)?;
                } else {
                    return Err(LibraryError::Transaction("Book not found".to_string()));
                }
            }
            "sort" | "author_sort" | "isbn" | "lccn" | "uuid" => {
                let sql = format!("UPDATE books SET {} = ?1 WHERE id = ?2", field);
                self.conn().execute(&sql, (value, book_id))?;
            }
            "pubdate" | "timestamp" => {
                let sql = format!("UPDATE books SET {} = ?1 WHERE id = ?2", field);
                self.conn().execute(&sql, (value, book_id))?;
            }
            "series_index" => {
                let val = value.parse::<f64>().unwrap_or(1.0);
                self.conn().execute(
                    "UPDATE books SET series_index = ?1 WHERE id = ?2",
                    (val, book_id),
                )?;
            }
            _ => {
                return Err(LibraryError::Transaction(format!(
                    "Unknown or unsupported field: {}",
                    field
                )));
            }
        }
        Ok(())
    }

    pub fn remove_custom_column(&mut self, label: &str) -> Result<(), LibraryError> {
        self.as_cache()
            .remove_custom_column(label)
            .map_err(|e| LibraryError::Transaction(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_open_memory() {
        let lib = Library::open_test();
        assert!(lib.is_ok());
    }

    #[test]
    fn test_update_book_metadata() {
        let mut lib = Library::open_test().unwrap();
        // Insert manually for test since add_book requires MetaInformation
        lib.conn().execute(
            "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index) VALUES ('Old Title', 'Old Author', '', 0, '', '', 1.0)",
            [],
        ).unwrap();
        let book_id = lib.conn().last_insert_rowid() as i32;

        // Link author
        lib.conn()
            .execute("INSERT INTO authors (name) VALUES ('Old Author')", [])
            .unwrap();
        let auth_id = lib.conn().last_insert_rowid();
        lib.conn()
            .execute(
                "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
                (book_id, auth_id),
            )
            .unwrap();

        // Update
        lib.update_book_metadata(book_id, "New Title", "New Author")
            .unwrap();

        // Verify Book
        let book = lib.get_book(book_id).unwrap().unwrap();

        assert_eq!(book.title, "New Title");
        assert_eq!(book.author_sort, Some("New Author".to_string()));

        // Verify Author Link
        let auth_name: String = lib.conn().query_row(
            "SELECT name FROM authors JOIN books_authors_link ON authors.id = books_authors_link.author WHERE books_authors_link.book = ?1",
            [book_id],
            |row| row.get(0)
        ).unwrap();
        assert_eq!(auth_name, "New Author");
    }

    #[test]
    fn test_delete_book() {
        let mut lib = Library::open_test().unwrap();
        lib.conn()
            .execute("INSERT INTO books (title) VALUES ('To Delete')", [])
            .unwrap();
        let book_id = lib.conn().last_insert_rowid() as i32;

        lib.delete_book(book_id).unwrap();

        let count: i32 = lib
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM books WHERE id = ?1",
                [book_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_rename_book() {
        let mut lib = Library::open_test().unwrap();
        let temp_dir = lib.path().to_path_buf();

        let old_author = "Old Author";
        let old_title = "Old Title";
        let old_rel_path = "Old_Author/Old_Title"; // Sanitized

        // Create files
        let full_book_dir = temp_dir.join(old_rel_path);
        std::fs::create_dir_all(&full_book_dir).unwrap();
        std::fs::write(full_book_dir.join("Old Title.mock"), "content").unwrap();

        lib.conn().execute(
                "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index)
                 VALUES (?1, ?2, ?3, 0, '', '', 1.0)",
                (old_title, old_author, old_rel_path),
            ).unwrap();
        let book_id = lib.conn().last_insert_rowid() as i32;

        // Link Author
        lib.conn()
            .execute("INSERT INTO authors (name) VALUES (?1)", [old_author])
            .unwrap();
        let auth_id = lib.conn().last_insert_rowid();
        lib.conn()
            .execute(
                "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
                (book_id, auth_id),
            )
            .unwrap();

        // 2. Rename
        lib.update_book_metadata(book_id, "New Title", "New Author")
            .unwrap();

        // 3. Verify DB Update
        let new_path: String = lib
            .conn()
            .query_row("SELECT path FROM books WHERE id = ?1", [book_id], |row| {
                row.get(0)
            })
            .unwrap();
        // Path normalization might vary on OS, but simple check:
        assert!(
            new_path.contains("New Author/New Title") || new_path.contains("New Author\\New Title")
        );

        // 4. Verify FS Update
        let new_full_dir = temp_dir.join("New Author/New Title");
        assert!(new_full_dir.exists(), "New directory should exist");

        let new_book_file = new_full_dir.join("New Title.mock");
        assert!(
            new_book_file.exists(),
            "File should be moved and renamed to New Title.mock"
        );

        // 5. Verify Old Cleanup
        let old_full_dir = temp_dir.join("Old_Author/Old_Title");
        assert!(!old_full_dir.exists(), "Old directory should be gone");

        let old_author_dir = temp_dir.join("Old_Author");
        assert!(
            !old_author_dir.exists(),
            "Old author directory should be gone (empty)"
        );
    }

    #[test]
    fn test_update_book_cover() {
        let mut lib = Library::open_test().unwrap();
        let temp_dir = lib.path().to_path_buf();

        // 1. Add Book
        let author = "Cover Author";
        let title = "Cover Title";
        let rel_path = "Cover_Author/Cover_Title";
        let full_book_dir = temp_dir.join(rel_path);
        std::fs::create_dir_all(&full_book_dir).unwrap();

        lib.conn().execute(
            "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index)
             VALUES (?1, ?2, ?3, 0, '', '', 1.0)",
            (title, author, rel_path),
        ).unwrap();
        let book_id = lib.conn().last_insert_rowid() as i32;

        // 2. Create a dummy cover source
        let cover_source = temp_dir.join("source_cover.jpg");
        std::fs::write(&cover_source, "fake image content").unwrap();

        // 3. Update Cover
        lib.update_book_cover(book_id, &cover_source).unwrap();

        // 4. Verify DB
        let has_cover: i32 = lib
            .conn()
            .query_row(
                "SELECT has_cover FROM books WHERE id = ?1",
                [book_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_cover, 1);

        // 5. Verify File
        let dest_cover = full_book_dir.join("cover.jpg");
        assert!(dest_cover.exists());
    }

    #[test]
    fn search_uses_the_real_query_engine_not_the_old_like_stub() {
        let mut lib = Library::open_test().unwrap();
        lib.insert_test_book("Foundation").unwrap();
        lib.conn()
            .execute("INSERT INTO books (title) VALUES ('Dune')", [])
            .unwrap();

        // A bare word still matches via the "all" location, same as
        // the old LIKE-on-title-or-author_sort stub did.
        assert_eq!(lib.search("foundation").unwrap(), vec![1]);

        // But real query syntax (AND/location prefixes) now works too,
        // which the old stub never supported at all.
        assert_eq!(
            lib.search("title:foundation or title:dune").unwrap().len(),
            2
        );
        assert!(lib
            .search("title:foundation and title:dune")
            .unwrap()
            .is_empty());
    }
}
