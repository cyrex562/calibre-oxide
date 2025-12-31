use rusqlite::{Connection, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct FtsConnection {
    conn: Arc<Mutex<Connection>>,
    fts_db_path: PathBuf,
}

impl FtsConnection {
    pub fn new(conn: Arc<Mutex<Connection>>, main_db_path: &Path) -> Self {
        let fts_db_path = main_db_path
            .parent()
            .unwrap_or(main_db_path)
            .join("full-text-search.db");
        FtsConnection { conn, fts_db_path }
    }

    pub fn initialize(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Check if already attached?
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

            // Ensure schema exists (Basic subset for verification)
            // In a real port, we'd use SchemaUpgrade logic here.
            // For now, we assume if it didn't exist, we create the table.
            conn.execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS fts_db.books_fts USING fts5(text, content='books_text', content_rowid='id')", 
                []
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS fts_db.books_text (id INTEGER PRIMARY KEY, book INTEGER, format TEXT, searchable_text TEXT)", 
                []
            )?;
        }
        Ok(())
    }

    pub fn search(&self, query_text: &str) -> Result<Vec<(i32, i32, String)>> {
        let conn = self.conn.lock().unwrap();

        // Simple FTS5 query
        // Matches Python logic: SELECT ... FROM books_text JOIN books_fts ...
        let sql = r#"
            SELECT books_text.id, books_text.book, books_text.format 
            FROM fts_db.books_text 
            JOIN fts_db.books_fts ON fts_db.books_text.id = fts_db.books_fts.rowid
            WHERE books_fts MATCH ? 
            ORDER BY books_fts.rank
        "#;

        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([query_text], |row| {
            let id: i32 = row.get(0)?;
            let book_id: i32 = row.get(1)?;
            let format: String = row.get(2)?;
            Ok((id, book_id, format))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // Helper for testing to add content
    pub fn add_document(&self, book_id: i32, format: &str, text: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO fts_db.books_text (book, format, searchable_text) VALUES (?, ?, ?)",
            (book_id, format, text),
        )?;
        // Assuming FTS5 content='books_text', we typically need to rebuild or insert into FTS index
        // depending on triggers. If external content table, we might need triggers or manual insert.
        // For simple FTS5 with external content, inserts into the content table are NOT automatically indexed
        // unless triggers are set up.
        // Let's add manually to FTS table for this sprint's basic verification if triggers aren't there.
        // Actually, simpler to just use normal FTS table for test if we don't assume existing triggers.
        // But let's stick to the structure `books_fts` USING fts5(content='books_text').
        // We need to insert into the FTS index to make it searchable.
        let last_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO fts_db.books_fts(rowid, text) VALUES (?, ?)",
            (last_id, text),
        )?;
        Ok(())
    }
}
