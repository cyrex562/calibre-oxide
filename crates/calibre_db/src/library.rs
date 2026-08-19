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

#[derive(Debug, serde::Serialize)]
pub struct Category {
    pub name: String,
    pub count: i32,
    // Add other fields as needed (rating, etc)
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
        // Simple sanitization for folder name
        let author_name = metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        let author_folder = sanitize_filename(author_name);
        let title_folder = sanitize_filename(&metadata.title);
        let rel_path = Path::new(&author_folder).join(&title_folder);
        // "Author/Title"
        let rel_path_str = rel_path.to_string_lossy().replace("\\", "/");

        let book_id = self.add_book_db_entry(metadata, &rel_path_str)?;

        let dest_dir = self.path.join(&rel_path);
        fs::create_dir_all(&dest_dir)?;

        let ext = source_path.extension().unwrap_or_default();
        let dest_file = dest_dir
            .join(sanitize_filename(&metadata.title))
            .with_extension(ext);

        fs::copy(source_path, dest_file)?;

        Ok(book_id)
    }

    /// adds a book entry to the database without copying files.
    /// used for restore_database.
    ///
    /// `books_insert_trg` (real schema) unconditionally overwrites
    /// `sort`/`uuid` right after the `INSERT` fires, so a caller-
    /// supplied `metadata.uuid` (e.g. from an OPF backup being
    /// restored) is restored via an explicit `UPDATE` afterward --
    /// same pattern real calibre's own restore path uses. A book with
    /// no supplied uuid just keeps the trigger's freshly generated one.
    pub fn add_book_db_entry(
        &mut self,
        metadata: &MetaInformation,
        rel_path: &str,
    ) -> Result<i32, LibraryError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;

        // 1. Author Logic
        let author_name = metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("Unknown");

        // 2. Insert Book
        tx.execute(
            "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
            (
                &metadata.title,
                author_name,
                rel_path,
                metadata.timestamp.unwrap_or(chrono::Utc::now()).to_rfc3339(),
                metadata.pubdate.unwrap_or(chrono::Utc::now()).to_rfc3339(),
                metadata.series_index,
            ),
        )?;
        let book_id = tx.last_insert_rowid() as i32;

        if let Some(uuid) = metadata.uuid.as_deref() {
            tx.execute("UPDATE books SET uuid = ?1 WHERE id = ?2", (uuid, book_id))?;
        }

        // 3. Insert/Get Author
        let author_id: i32 = {
            let mut stmt = tx.prepare("SELECT id FROM authors WHERE name = ?1")?;
            let mut rows = stmt.query([author_name])?;
            if let Some(row) = rows.next()? {
                row.get(0)?
            } else {
                tx.execute("INSERT INTO authors (name) VALUES (?1)", [author_name])?;
                tx.last_insert_rowid() as i32
            }
        };

        // 4. Link
        tx.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (book_id, author_id),
        )?;

        tx.commit()?;
        Ok(book_id)
    }

    pub fn update_book_metadata(
        &mut self,
        book_id: i32,
        title: &str,
        author: &str,
    ) -> Result<(), LibraryError> {
        // Warning: Transaction safety with file ops is hard.
        // Ideally we do file ops after commit, or rollback file ops if DB fails.
        // For this simple implementation, we try file ops first, then commit.
        // If file ops fail, we abort.
        self.rename_book_files(book_id, title, author)
            .map_err(LibraryError::Io)?;

        let mut conn = self.conn();
        let tx = conn.transaction()?;

        // 1. Update Title in Books
        // Note: rename_book_files already updated the path column if it
        // succeeded; this still needs to update title/author_sort.
        tx.execute(
            "UPDATE books SET title = ?1, author_sort = ?2 WHERE id = ?3",
            (title, author, book_id),
        )?;

        // 2. Update Author
        tx.execute("DELETE FROM books_authors_link WHERE book = ?1", (book_id,))?;

        let author_id: i32 = {
            let mut stmt = tx.prepare("SELECT id FROM authors WHERE name = ?1")?;
            let mut rows = stmt.query([author])?;
            if let Some(row) = rows.next()? {
                row.get(0)?
            } else {
                tx.execute("INSERT INTO authors (name) VALUES (?1)", [author])?;
                tx.last_insert_rowid() as i32
            }
        };

        tx.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (book_id, author_id),
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn add_format(
        &mut self,
        book_id: i32,
        source_path: &Path,
        format: &str,
        replace: bool,
    ) -> Result<bool, LibraryError> {
        let book_opt = self.get_book(book_id)?;
        if let Some(book) = book_opt {
            let book_rel_path = book.path;
            if book_rel_path.is_empty() {
                return Err(LibraryError::Transaction("Book has no path".to_string()));
            }

            let book_dir = self.path.join(&book_rel_path);
            if !book_dir.exists() {
                std::fs::create_dir_all(&book_dir)?;
            }

            // Construct destination filename: Title.EXT
            // For safety, we should probably stick to the title in the DB, sanitized.
            // But usually Calibre uses the filename of the book record (which matches title usually).
            let file_name = format!(
                "{}.{}",
                sanitize_filename(&book.title),
                format.to_lowercase()
            );
            let dest_path = book_dir.join(&file_name);

            if dest_path.exists() && !replace {
                return Ok(false);
            }

            std::fs::copy(source_path, dest_path)?;

            // Update timestamp of the book
            self.conn().execute(
                "UPDATE books SET timestamp = datetime('now') WHERE id = ?1",
                (book_id,),
            )?;

            Ok(true)
        } else {
            Err(LibraryError::Transaction(format!(
                "Book {} not found",
                book_id
            )))
        }
    }

    fn rename_book_files(
        &mut self,
        book_id: i32,
        new_title: &str,
        new_author: &str,
    ) -> std::io::Result<()> {
        // Query old path
        let old_rel_path: String = self
            .conn()
            .query_row("SELECT path FROM books WHERE id = ?1", [book_id], |row| {
                row.get(0)
            })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        if old_rel_path.is_empty() {
            return Ok(()); // Nothing to rename
        }

        let old_full_dir = self.path.join(&old_rel_path);
        if !old_full_dir.exists() {
            return Ok(()); // Directory missing, can't rename
        }

        let new_author_folder = sanitize_filename(new_author);
        let new_title_folder = sanitize_filename(new_title);
        let new_rel_path = Path::new(&new_author_folder).join(&new_title_folder);
        let new_full_dir = self.path.join(&new_rel_path);

        if old_full_dir == new_full_dir {
            return Ok(());
        }

        // Create new parent dir (Author) if needed
        let new_author_full_path = self.path.join(&new_author_folder);
        if !new_author_full_path.exists() {
            fs::create_dir_all(&new_author_full_path)?;
        }

        // Move the book directory
        fs::rename(&old_full_dir, &new_full_dir)?;

        // Rename files inside
        for entry in fs::read_dir(&new_full_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // preserve special files
                    if file_name == "cover.jpg" || file_name == "metadata.opf" {
                        continue;
                    }

                    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
                        // Simple heuristic: rename all other files to match new title
                        // This aligns with Calibre's behavior for the main book files
                        let new_file_name =
                            format!("{}.{}", sanitize_filename(new_title), extension);
                        let new_file_path = new_full_dir.join(new_file_name);
                        if path != new_file_path {
                            let _ = fs::rename(path, new_file_path);
                        }
                    }
                }
            }
        }

        // Cleanup old author dir if empty
        if let Some(parent) = old_full_dir.parent() {
            if parent.exists() && fs::read_dir(parent)?.next().is_none() {
                let _ = fs::remove_dir(parent); // Ignore error if not empty
            }
        }

        // Update DB with NEW path
        let new_rel_path_str = new_rel_path.to_string_lossy().replace("\\", "/");
        self.conn()
            .execute(
                "UPDATE books SET path = ?1 WHERE id = ?2",
                (&new_rel_path_str, book_id),
            )
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(())
    }

    pub fn update_book_cover(
        &mut self,
        book_id: i32,
        new_cover_path: &Path,
    ) -> Result<(), LibraryError> {
        let path_query: Option<String> = self
            .conn()
            .query_row("SELECT path FROM books WHERE id = ?1", (book_id,), |row| {
                row.get(0)
            })
            .ok();

        if let Some(rel_path) = path_query {
            if !rel_path.is_empty() {
                let dir_path = self.path.join(rel_path);
                if dir_path.exists() {
                    let dest_path = dir_path.join("cover.jpg");
                    fs::copy(new_cover_path, dest_path)?;

                    // Update DB
                    self.conn()
                        .execute("UPDATE books SET has_cover = 1 WHERE id = ?1", (book_id,))?;
                }
            }
        }
        Ok(())
    }

    pub fn delete_book(&mut self, book_id: i32) -> Result<(), LibraryError> {
        // Get path before deleting to remove files
        let path_query: Option<String> = self
            .conn()
            .query_row("SELECT path FROM books WHERE id = ?1", (book_id,), |row| {
                row.get(0)
            })
            .ok();

        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM books WHERE id = ?1", (book_id,))?;
            // Note: the real schema's `books_delete_trg` already cascades
            // this cleanup (books_authors_link and more); left explicit
            // here too since it's harmless (no-op on an already-empty set).
            tx.execute("DELETE FROM books_authors_link WHERE book = ?1", (book_id,))?;
            tx.commit()?;
        }

        // File Cleanup
        if let Some(rel_path) = path_query {
            if !rel_path.is_empty() {
                let dir_path = self.path.join(rel_path);
                if dir_path.exists() {
                    // Try to remove the directory (and contents)
                    if let Err(e) = fs::remove_dir_all(&dir_path) {
                        eprintln!("Warning: Failed to delete directory {:?}: {}", dir_path, e);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get_custom_column_label_map(
        &self,
    ) -> Result<std::collections::HashMap<String, serde_json::Value>, LibraryError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized FROM custom_columns")?;

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

        let mut custom_columns = std::collections::HashMap::new();
        for row in rows {
            let (label, data) = row?;
            custom_columns.insert(label, data);
        }

        Ok(custom_columns)
    }

    /// Real query-syntax search (issue #210): shares this `Library`'s
    /// own live connection with a [`crate::cache::Cache`] (via
    /// `Backend`'s cheap `Clone` -- no second connection opened) and
    /// delegates to [`crate::search::search`]. See this module's docs
    /// for why that engine was previously unreachable from the CLI.
    pub fn search(&self, query: &str) -> Result<Vec<i32>, LibraryError> {
        let cache = Arc::new(Mutex::new(crate::cache::Cache {
            backend: self.backend.clone(),
        }));
        crate::search::search(&cache, query).map_err(|e| LibraryError::Transaction(e.to_string()))
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
        if let Some(book) = book_opt {
            let book_rel_path = book.path;
            if book_rel_path.is_empty() {
                // Should we error or skip? Python skips invisible books or similar?
                // For now, if no path, we can't write OPF.
                return Ok(());
            }

            let book_dir = self.path.join(&book_rel_path);
            if !book_dir.exists() {
                std::fs::create_dir_all(&book_dir)?;
            }

            let mut meta = MetaInformation::default();
            meta.title = book.title;
            meta.authors = vec![book.author_sort.unwrap_or_default()];
            // TODO: Fill more metadata from DB?
            // For now, basic metadata is enough for a port start
            meta.uuid = book.uuid;

            let xml = meta.to_xml();
            let opf_path = book_dir.join("metadata.opf");
            fs::write(opf_path, xml)?;

            Ok(())
        } else {
            Err(LibraryError::InvalidPath) // Or BookNotFound
        }
    }

    pub fn vacuum(&self, vacuum_fts: bool) -> Result<(), LibraryError> {
        self.conn().execute("VACUUM", [])?;
        if vacuum_fts {
            // Placeholder: functionality for FTS vacuum if we have FTS db
        }
        Ok(())
    }

    pub fn get_categories(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<Category>>, LibraryError> {
        let mut categories = std::collections::HashMap::new();

        // 1. Authors
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT name, (SELECT COUNT(*) FROM books_authors_link WHERE author = authors.id) as count FROM authors")?;
        let author_rows = stmt.query_map([], |row| {
            Ok(Category {
                name: row.get(0)?,
                count: row.get(1)?,
            })
        })?;

        let mut authors = Vec::new();
        for row in author_rows {
            authors.push(row?);
        }
        categories.insert("authors".to_string(), authors);

        // TODO: Add other categories (Series, Tags, etc.)
        // For series:
        // let mut stmt = self.conn().prepare("SELECT name, (SELECT COUNT(*) FROM books WHERE series_index IS NOT NULL) ...")?
        // We'll tackle series when we have a series table or clearer schema.

        Ok(categories)
    }

    pub fn remove_format(&mut self, book_id: i32, fmt: &str) -> Result<(), LibraryError> {
        let book_opt = self.get_book(book_id)?;
        if let Some(book) = book_opt {
            let book_rel_path = book.path;
            if book_rel_path.is_empty() {
                return Ok(()); // Or fail?
            }

            let book_dir = self.path.join(&book_rel_path);
            if !book_dir.exists() {
                return Ok(());
            }

            // Construct filename.
            // Warning: We need to know the exact filename.
            // If we constructed it in add_format using title, we should try that.
            // Or scan directory for extensions?
            // Python: "fmt should be a file extension like LRF or TXT or EPUB"

            // Strategy: Look for file with extension in the directory.
            // Since we don't store exact filenames in DB (only path to dir),
            // we have to search.
            let target_ext = fmt.to_lowercase();

            if let Ok(entries) = fs::read_dir(&book_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if ext.to_lowercase() == target_ext {
                                fs::remove_file(path)?;
                                // Only remove one? Or all matching?
                                // Ideally there is only one per format.
                                // I'll stop after one.
                                return Ok(());
                            }
                        }
                    }
                }
            }

            // If not found, do nothing?
            Ok(())
        } else {
            Err(LibraryError::Transaction(format!(
                "Book {} not found",
                book_id
            )))
        }
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
        if !dest.exists() {
            fs::create_dir_all(dest)?;
        }

        self.copy_recursive(&self.path, dest)
            .map_err(LibraryError::Io)?;
        Ok(())
    }

    fn copy_recursive(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        if !dst.exists() {
            fs::create_dir_all(dst)?;
        }
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let dest_path = dst.join(entry.file_name());
            if path.is_dir() {
                self.copy_recursive(&path, &dest_path)?;
            } else {
                fs::copy(&path, &dest_path)?;
            }
        }
        Ok(())
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
        let mut conn = self.conn();

        // Validation: Verify label is unique
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = ?1",
            [label],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Err(LibraryError::Transaction(format!(
                "Column with label '{}' already exists",
                label
            )));
        }

        let tx = conn.transaction()?;

        // 1. Insert into custom_columns
        tx.execute(
            "INSERT INTO custom_columns
            (label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized)
            VALUES (?1, ?2, ?3, 0, 1, '{}', ?4, 0)",
            (label, name, datatype, is_multiple),
        )?;
        let col_id = tx.last_insert_rowid() as i32;

        // 2. Create the table for the column
        // Calibre naming convention: custom_column_{id}
        let table_name = format!("custom_column_{}", col_id);

        // Simplified schema generation based on generic behavior
        match datatype {
            "bool" | "int" | "float" | "rating" => {
                // One-to-one mapping
                tx.execute(
                    &format!(
                        "CREATE TABLE {} (id INTEGER PRIMARY KEY, book INTEGER, value {})",
                        table_name,
                        if datatype == "float" || datatype == "rating" {
                            "REAL"
                        } else {
                            "INTEGER"
                        }
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{}_book ON {} (book)", col_id, table_name),
                    [],
                )?;
            }
            "text" | "comments" | "series" => {
                if is_multiple {
                    return Err(LibraryError::Transaction(
                        "Multiple-value text columns not yet supported in this port".to_string(),
                    ));
                }
                tx.execute(
                    &format!(
                        "CREATE TABLE {} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)",
                        table_name
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{}_book ON {} (book)", col_id, table_name),
                    [],
                )?;
            }
            _ => {
                tx.execute(
                    &format!(
                        "CREATE TABLE {} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)",
                        table_name
                    ),
                    [],
                )?;
            }
        }

        tx.commit()?;
        Ok(col_id)
    }

    pub fn set_custom_column_value(
        &mut self,
        book_id: i32,
        label: &str,
        value: &str,
    ) -> Result<(), LibraryError> {
        // 1. Get column info
        let col_info: Option<(i32, String)> = self
            .conn()
            .query_row(
                "SELECT id, datatype FROM custom_columns WHERE label = ?1",
                [label],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((col_id, datatype)) = col_info {
            let table_name = format!("custom_column_{}", col_id);

            // 2. Validate/Convert value based on datatype
            match datatype.as_str() {
                "bool" => {
                    let val = value.parse::<bool>().unwrap_or(false);
                    self.conn().execute(
                        &format!(
                            "INSERT OR REPLACE INTO {} (book, value) VALUES (?1, ?2)",
                            table_name
                        ),
                        (book_id, val as i32),
                    )?;
                }
                "int" => {
                    let val = value.parse::<i32>().unwrap_or(0);
                    self.conn().execute(
                        &format!(
                            "INSERT OR REPLACE INTO {} (book, value) VALUES (?1, ?2)",
                            table_name
                        ),
                        (book_id, val),
                    )?;
                }
                "float" | "rating" => {
                    let val = value.parse::<f64>().unwrap_or(0.0);
                    self.conn().execute(
                        &format!(
                            "INSERT OR REPLACE INTO {} (book, value) VALUES (?1, ?2)",
                            table_name
                        ),
                        (book_id, val),
                    )?;
                }
                _ => {
                    // Text and others
                    self.conn().execute(
                        &format!(
                            "INSERT OR REPLACE INTO {} (book, value) VALUES (?1, ?2)",
                            table_name
                        ),
                        (book_id, value),
                    )?;
                }
            }
            Ok(())
        } else {
            Err(LibraryError::Transaction(format!(
                "Custom column with label '{}' not found",
                label
            )))
        }
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
        let col_info: Option<(i32, String)> = self
            .conn()
            .query_row(
                "SELECT id, datatype FROM custom_columns WHERE label = ?1",
                [label],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((col_id, datatype)) = col_info {
            let table_name = format!("custom_column_{}", col_id);
            let val: Option<String> = self
                .conn()
                .query_row(
                    &format!("SELECT value FROM {} WHERE book = ?1", table_name),
                    [book_id],
                    |row| match datatype.as_str() {
                        "int" | "bool" => {
                            let v: i32 = row.get(0)?;
                            Ok(v.to_string())
                        }
                        "float" | "rating" => {
                            let v: f64 = row.get(0)?;
                            Ok(v.to_string())
                        }
                        _ => row.get(0),
                    },
                )
                .optional()?;
            Ok(val)
        } else {
            Ok(None)
        }
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
        let mut conn = self.conn();
        let col_id: Option<i32> = conn
            .query_row(
                "SELECT id FROM custom_columns WHERE label = ?1",
                [label],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = col_id {
            let tx = conn.transaction()?;

            // 1. Delete meta
            tx.execute("DELETE FROM custom_columns WHERE id = ?1", [id])?;

            // 2. Drop table
            let table_name = format!("custom_column_{}", id);
            // We use format! string since we can't parametrize table names.
            // id is an integer controlled by us, so injection risk is minimal/none.
            tx.execute(&format!("DROP TABLE IF EXISTS {}", table_name), [])?;

            tx.commit()?;
            Ok(())
        } else {
            Err(LibraryError::Transaction(format!(
                "Column '{}' not found",
                label
            )))
        }
    }
}

fn sanitize_filename(name: &str) -> String {
    name.replace("/", "_")
        .replace("\\", "_")
        .replace(":", "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '_' || *c == '-')
        .collect()
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
