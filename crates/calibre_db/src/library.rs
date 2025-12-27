use crate::book::Book;
use rusqlite::{Connection, OpenFlags, Result};
use std::path::{Path, PathBuf};
use std::fs;
use thiserror::Error;
use calibre_ebooks::opf::OpfMetadata;

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

pub struct Library {
    conn: Connection,
    path: PathBuf,
}

impl Library {
    pub fn open(path: PathBuf) -> Result<Self, LibraryError> {
        let db_path = path.join("metadata.db");
        if !db_path.exists() {
            return Err(LibraryError::InvalidPath);
        }

        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
        )?;

        // Register custom functions expected by Calibre triggers
        conn.create_scalar_function("title_sort", 1, rusqlite::functions::FunctionFlags::SQLITE_UTF8, |ctx| {
            let title: String = ctx.get(0)?;
            // strict title sort logic is complex, returning identity for now
            Ok(title)
        })?;
        
        conn.create_scalar_function("author_to_author_sort", 1, rusqlite::functions::FunctionFlags::SQLITE_UTF8, |ctx| {
            let author: String = ctx.get(0)?;
            Ok(author)
        })?;

        conn.create_scalar_function("uuid4", 0, rusqlite::functions::FunctionFlags::SQLITE_UTF8, |_ctx| {
             Ok(uuid::Uuid::new_v4().to_string())
        })?;

        Ok(Library { conn, path })
    }

    /// Open an in-memory database for testing
    pub fn open_test() -> Result<Self, LibraryError> {
        let conn = Connection::open_in_memory()?;
        // Create minimal schema matching list_books query
        conn.execute_batch(
            "CREATE TABLE books (
                id INTEGER PRIMARY KEY,
                title TEXT,
                sort TEXT,
                timestamp TEXT,
                pubdate TEXT,
                series_index REAL,
                author_sort TEXT,
                isbn TEXT,
                lccn TEXT,
                path TEXT,
                has_cover INTEGER,
                uuid TEXT
            );
            CREATE TABLE authors (
                id INTEGER PRIMARY KEY,
                name TEXT UNIQUE,
                sort TEXT,
                link TEXT
            );
            CREATE TABLE books_authors_link (
                id INTEGER PRIMARY KEY,
                book INTEGER,
                author INTEGER
            );"
        )?;
        Ok(Library { 
            conn, 
            path: PathBuf::from(":memory:") 
        })
    }

    pub fn insert_test_book(&self, title: &str) -> Result<(), LibraryError> {
        self.conn.execute(
            "INSERT INTO books (title, sort, author_sort, has_cover, series_index, path) 
             VALUES (?1, ?1, 'Author', 0, 1.0, '')",
            (title,),
        )?;
        Ok(())
    }

    pub fn book_count(&self) -> Result<i32, LibraryError> {
        let mut stmt = self.conn.prepare("SELECT COUNT(*) FROM books")?;
        let count: i32 = stmt.query_row([], |row| row.get(0))?;
        Ok(count)
    }

    pub fn list_books(&self) -> Result<Vec<Book>, LibraryError> {
        let mut stmt = self.conn.prepare(
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

    pub fn add_book(&mut self, source_path: &Path, metadata: &OpfMetadata) -> Result<i32, LibraryError> {
        let tx = self.conn.transaction()?;
        
        // 1. Author Logic
        let author_name = metadata.authors.first().map(|s| s.as_str()).unwrap_or("Unknown");
        // Simple sanitization for folder name
        let author_folder = sanitize_filename(author_name);
        let title_folder = sanitize_filename(&metadata.title);
        
        let rel_path = Path::new(&author_folder).join(&title_folder);
        // "Author/Title"
        let rel_path_str = rel_path.to_string_lossy().replace("\\", "/"); 

        // 2. Insert Book
        tx.execute(
            "INSERT INTO books (title, sort, author_sort, path, has_cover, timestamp, pubdate, uuid)
             VALUES (?1, ?1, ?2, ?3, 0, datetime('now'), datetime('now'), ?4)",
            (
                &metadata.title,
                author_name,
                &rel_path_str,
                metadata.uuid.as_deref().unwrap_or(""),
            ),
        )?;
        let book_id = tx.last_insert_rowid() as i32;

        // 3. Insert/Get Author
        let author_id: i32 = {
            let mut stmt = tx.prepare("SELECT id FROM authors WHERE name = ?1")?;
            let mut rows = stmt.query([author_name])?;
            if let Some(row) = rows.next()? {
                row.get(0)?
            } else {
                tx.execute("INSERT INTO authors (name, sort) VALUES (?1, ?1)", [author_name])?;
                tx.last_insert_rowid() as i32
            }
        };

        // 4. Link
        tx.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (book_id, author_id))?;

        // 5. File System Operations
        if self.path != PathBuf::from(":memory:") {
            let dest_dir = self.path.join(&rel_path);
            fs::create_dir_all(&dest_dir)?;
            
            let ext = source_path.extension().unwrap_or_default();
            let dest_file = dest_dir.join(sanitize_filename(&metadata.title)).with_extension(ext);
            
            fs::copy(source_path, dest_file)?;
        }

        tx.commit()?;
        
        Ok(book_id)
    }

    pub fn update_book_metadata(&mut self, book_id: i32, title: &str, author: &str) -> Result<(), LibraryError> {
        // Warning: Transaction safety with file ops is hard. 
        // Ideally we do file ops after commit, or rollback file ops if DB fails.
        // For this simple implementation, we try file ops first (if not memory), then commit. 
        // If file ops fail, we abort. 

        if self.path != PathBuf::from(":memory:") {
             self.rename_book_files(book_id, title, author).map_err(|e| LibraryError::Io(e))?;
        }

        let tx = self.conn.transaction()?;
        
        // 1. Update Title in Books
        // Note: rename_book_files might have already updated the path in DB? 
        // Actually, rename_book_files needs to know the NEW path, so it calculates it.
        // But it should also update the path in the DB.
        
        // Re-calculate path to save in DB
        // Logic duplicated from rename for safety, or we rely on rename_book_files to do it?
        // Let's rely on rename_book_files to UPDATE the path column if it succeeds.
        // But we still need to update Title/Author columns here.
        
        tx.execute(
            "UPDATE books SET title = ?1, sort = ?1, author_sort = ?2 WHERE id = ?3",
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
                tx.execute("INSERT INTO authors (name, sort) VALUES (?1, ?1)", [author])?;
                tx.last_insert_rowid() as i32
            }
        };

        tx.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (book_id, author_id))?;

        tx.commit()?;
        Ok(())
    }

    fn rename_book_files(&mut self, book_id: i32, new_title: &str, new_author: &str) -> std::io::Result<()> {
        // Query old path
        let old_rel_path: String = self.conn.query_row(
            "SELECT path FROM books WHERE id = ?1",
            [book_id],
            |row| row.get(0)
        ).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        
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
        // Note: renaming across filesystems might fail, but typical for library to be on one fs.
        fs::rename(&old_full_dir, &new_full_dir)?;

        // Rename files inside? 
        // If Title changed, Calibre usually renames the EPUB too: `Old Title.epub` -> `New Title.epub`.
        // Let's attempt that.
        for entry in fs::read_dir(&new_full_dir)? {
             let entry = entry?;
             let path = entry.path();
             if path.is_file() {
                 if let Some(_stem) = path.file_stem() {
                     let _ext = path.extension().unwrap_or_default();
                     // Assumption: The old filename was `SanitizedOldTitle.ext`
                     // But we don't strictly know "Old Title" here easily without querying DB. 
                     // Or check if filename checks out?
                     // Calibre strategy: Rename ALL media files to match new title?
                     // Unsafe. 
                     // Safest: Rename only if it matches current title logic, or just leave files as is? 
                     // Calibre DOES rename files. 
                     // Let's blindly rename all known ebook extensions or just main file?
                     // Hard to guess.
                     // For now: Only rename directory. Renaming internal files is risky without full format tracking.
                     // Wait, `cover.jpg` should stay `cover.jpg`.
                     // `metadata.opf` should stay `metadata.opf`.
                     // The actual book file... 
                     // Let's SKIP renaming individual files for this iteration to reduce risk, as agreed in Plan.
                     // Plan said: "Rename Book folder. Rename files inside Book folder ... wait, cover usually stays".
                     // "Logic to move/rename book files on disk" -> Folder move is most critical for structure.
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
        self.conn.execute(
             "UPDATE books SET path = ?1 WHERE id = ?2",
             (&new_rel_path_str, book_id)
        ).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(())
    }

    pub fn delete_book(&mut self, book_id: i32) -> Result<(), LibraryError> {
        // Get path before deleting to remove files
        let path_query: Option<String> = self.conn.query_row(
            "SELECT path FROM books WHERE id = ?1",
            (book_id,),
            |row| row.get(0),
        ).ok();

        let tx = self.conn.transaction()?;
        
        tx.execute("DELETE FROM books WHERE id = ?1", (book_id,))?;
        tx.execute("DELETE FROM books_authors_link WHERE book = ?1", (book_id,))?;
        // Note: Authors are left even if they have no books, typical Calibre behavior (or maybe cleanup?)
        // We leave them for now.

        tx.commit()?;

        // File Cleanup
        if let Some(rel_path) = path_query {
            if self.path != PathBuf::from(":memory:") && !rel_path.is_empty() {
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
        // Insert manually for test since add_book requires OpfMetadata
        lib.conn.execute(
            "INSERT INTO books (title, sort, author_sort, path, has_cover, timestamp, pubdate, uuid, series_index) VALUES ('Old Title', 'Old Title', 'Old Author', '', 0, '', '', '', 1.0)", 
            [],
        ).unwrap();
        let book_id = lib.conn.last_insert_rowid() as i32;
        
        // Link author
        lib.conn.execute("INSERT INTO authors (name, sort) VALUES ('Old Author', 'Old Author')", []).unwrap();
        let auth_id = lib.conn.last_insert_rowid();
        lib.conn.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (book_id, auth_id)).unwrap();

        // Update
        lib.update_book_metadata(book_id, "New Title", "New Author").unwrap();

        // Verify Book
        let book: Book = lib.conn.query_row("SELECT id, title, sort, timestamp, pubdate, series_index, author_sort, isbn, lccn, path, has_cover, uuid FROM books WHERE id = ?1", [book_id], |row| {
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
        }).unwrap();

        assert_eq!(book.title, "New Title");
        assert_eq!(book.author_sort, Some("New Author".to_string())); // In our simple logic, author_sort = author name

        // Verify Author Link
        let auth_name: String = lib.conn.query_row(
            "SELECT name FROM authors JOIN books_authors_link ON authors.id = books_authors_link.author WHERE books_authors_link.book = ?1",
            [book_id],
            |row| row.get(0)
        ).unwrap();
        assert_eq!(auth_name, "New Author");
    }

    #[test]
    fn test_delete_book() {
        let mut lib = Library::open_test().unwrap();
        lib.conn.execute(
            "INSERT INTO books (title) VALUES ('To Delete')",
            [],
        ).unwrap();
        let book_id = lib.conn.last_insert_rowid() as i32;

        lib.delete_book(book_id).unwrap();

        let count: i32 = lib.conn.query_row("SELECT COUNT(*) FROM books WHERE id = ?1", [book_id], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}

    #[test]
    fn test_rename_book() {
        // Use a temp dir for real FS test
        let temp_dir = std::env::temp_dir().join("calibre_oxide_test_rename");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
        std::fs::create_dir_all(&temp_dir).unwrap();
        // Touch metadata.db
        std::fs::File::create(temp_dir.join("metadata.db")).unwrap();

        let mut lib = Library::open(temp_dir.clone()).unwrap();
        
        // Setup schema (since open only does connection, does it create schema? No, Library::open assumes existing DB or we need to init it. 
        // Wait, Library::open checks if metadata.db exists. 
        // We need to create a dummy metadata.db or use open_test logic but with real path? 
        // Library::open assumes existing library. 
        // We should manually initialize schema for this test or factor out "init schema".
        // For now, let's just use open_test logic but manually swapped path, OR manually run migration.
        
        // Manual migration for test
        lib.conn.execute_batch(
            "CREATE TABLE books (
                id INTEGER PRIMARY KEY,
                title TEXT,
                sort TEXT,
                timestamp TEXT,
                pubdate TEXT,
                series_index REAL,
                author_sort TEXT,
                isbn TEXT,
                lccn TEXT,
                path TEXT,
                has_cover INTEGER,
                uuid TEXT
            );
            CREATE TABLE authors (
                id INTEGER PRIMARY KEY,
                name TEXT UNIQUE,
                sort TEXT,
                link TEXT
            );
            CREATE TABLE books_authors_link (
                id INTEGER PRIMARY KEY,
                book INTEGER,
                author INTEGER
            );"
        ).unwrap();

        // 1. Add Book (Manual insert to bypass OpfMetadata requirement for now, mimicking add_book logic partially)
        let old_author = "Old Author";
        let old_title = "Old Title";
        let old_rel_path = "Old_Author/Old_Title"; // Sanitized
        
        // Create files
        let full_book_dir = temp_dir.join(old_rel_path);
        std::fs::create_dir_all(&full_book_dir).unwrap();
        std::fs::write(full_book_dir.join("book.mock"), "content").unwrap();

        lib.conn.execute(
            "INSERT INTO books (title, sort, author_sort, path, has_cover, timestamp, pubdate, uuid, series_index) 
             VALUES (?1, ?1, ?2, ?3, 0, '', '', '', 1.0)",
            (old_title, old_author, old_rel_path),
        ).unwrap();
        let book_id = lib.conn.last_insert_rowid() as i32;
        
        // Link Author
        lib.conn.execute("INSERT INTO authors (name, sort) VALUES (?1, ?1)", [old_author]).unwrap();
        let auth_id = lib.conn.last_insert_rowid();
        lib.conn.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (book_id, auth_id)).unwrap();

        // 2. Rename
        lib.update_book_metadata(book_id, "New Title", "New Author").unwrap();

        // 3. Verify DB Update
        let new_path: String = lib.conn.query_row("SELECT path FROM books WHERE id = ?1", [book_id], |row| row.get(0)).unwrap();
        // Path normalization might vary on OS, but simple check:
        assert!(new_path.contains("New Author/New Title") || new_path.contains("New Author\\New Title"));

        // 4. Verify FS Update
        let new_full_dir = temp_dir.join("New Author/New Title");
        assert!(new_full_dir.exists(), "New directory should exist");
        assert!(new_full_dir.join("book.mock").exists(), "File should be moved");
        
        let old_full_dir = temp_dir.join("Old_Author/Old_Title");
        assert!(!old_full_dir.exists(), "Old directory should be gone");
        
        let old_author_dir = temp_dir.join("Old_Author");
        assert!(!old_author_dir.exists(), "Old author directory should be gone (empty)");

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
