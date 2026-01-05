use rusqlite::{Connection, OptionalExtension, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct NotesConnection {
    conn: Arc<Mutex<Connection>>,
    notes_dir: PathBuf,
}

impl NotesConnection {
    pub fn new(conn: Arc<Mutex<Connection>>, library_path: &Path) -> Self {
        // Using a safe default or based on python's NOTES_DIR_NAME which is usually related to library root.
        // Python: self.notes_dir = os.path.join(libdir, NOTES_DIR_NAME) where NOTES_DIR_NAME is often ".calnotes" or similar?
        // Actually constants.py says NOTES_DIR_NAME = '.calnotes'.
        let notes_dir = library_path.join(".calnotes");

        NotesConnection { conn, notes_dir }
    }

    pub fn initialize(&self) -> Result<()> {
        if !self.notes_dir.exists() {
            fs::create_dir_all(&self.notes_dir).unwrap_or_default();
            // In a full impl, we'd hide this dir on Windows
        }

        let notes_db_path = self.notes_dir.join("notes.db");

        let conn = self.conn.lock().unwrap();

        // Attach
        let attached: i32 = conn
            .query_row(
                "SELECT count(*) FROM pragma_database_list WHERE name='notes_db'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if attached == 0 {
            conn.execute(
                "ATTACH DATABASE ? AS notes_db",
                [notes_db_path.to_str().unwrap()],
            )?;

            // Create Schema if needed
            conn.execute(
                "CREATE TABLE IF NOT EXISTS notes_db.notes (
                    id INTEGER PRIMARY KEY,
                    colname TEXT NOT NULL,
                    item INTEGER NOT NULL,
                    doc TEXT,
                    searchable_text TEXT,
                    ctime REAL,
                    mtime REAL,
                    UNIQUE(colname, item)
                 )",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS notes_db.resources (
                    hash TEXT PRIMARY KEY,
                    name TEXT
                 )",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS notes_db.notes_resources_link (
                    note INTEGER,
                    resource TEXT,
                    UNIQUE(note, resource)
                 )",
                [],
            )?;
        }
        Ok(())
    }

    pub fn set_note(
        &self,
        field: &str,
        item_id: i32,
        doc: &str,
        searchable_text: &str,
    ) -> Result<i32> {
        let conn = self.conn.lock().unwrap();
        // Check if exists
        let existing_id: Option<i32> = conn
            .query_row(
                "SELECT id FROM notes_db.notes WHERE colname=? AND item=?",
                [field, &item_id.to_string()], // rusqlite params need to match, item_id is int
                |row| row.get(0),
            )
            .optional()?;

        // Fix param types: item_id is i32.
        // Need to drop the lock if we use methods that lock, but here we are inside one "transaction" logic.
        // Rusqlite execute with params:

        // Use a block or new vars to avoid borrow issues if complex
        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE notes_db.notes SET doc=?, searchable_text=?, mtime=? WHERE id=?",
                (doc, searchable_text, 0.0, id), // Simplified time
            )?;
            Ok(id)
        } else {
            conn.execute(
                 "INSERT INTO notes_db.notes (colname, item, doc, searchable_text, ctime, mtime) VALUES (?, ?, ?, ?, ?, ?)",
                 (field, item_id, doc, searchable_text, 0.0, 0.0)
            )?;
            Ok(conn.last_insert_rowid() as i32)
        }
    }

    pub fn get_note(&self, field: &str, item_id: i32) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT doc FROM notes_db.notes WHERE colname=? AND item=?",
            (field, item_id),
            |row| row.get(0),
        )
        .optional()
    }
}
