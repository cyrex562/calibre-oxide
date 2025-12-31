use rusqlite::{Connection, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Backend {
    pub library_path: PathBuf,
    pub db_path: PathBuf,
    pub conn: Arc<Mutex<Connection>>,
    pub prefs: HashMap<String, String>,
}

impl Backend {
    pub fn new<P: AsRef<Path>>(library_path: P) -> Result<Self> {
        let library_path = library_path.as_ref().to_path_buf();
        let db_path = library_path.join("metadata.db");

        let mut conn = Connection::open(&db_path)?;

        // Basic optimization pragmas used in Calibre
        conn.execute("PRAGMA cache_size=-5000", [])?;
        conn.execute("PRAGMA temp_store=2", [])?;
        conn.execute("PRAGMA foreign_keys=ON", [])?;

        // Run schema upgrades (stub for now)
        crate::schema_upgrades::SchemaUpgrade::upgrade_to_latest(&mut conn, &library_path)?;

        let backend = Backend {
            library_path,
            db_path,
            conn: Arc::new(Mutex::new(conn)),
            prefs: HashMap::new(),
        };
        Ok(backend)
    }

    pub fn field_for(&self, book_id: i32, field_name: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();

        // Allowed fields whitelist to prevent injection
        let sql = match field_name {
            "title" | "sort" | "author_sort" | "isbn" | "path" | "series_index" | "uuid" => {
                format!("SELECT {} FROM books WHERE id = ?", field_name)
            }
            _ => return Ok(None),
        };

        let mut stmt = conn.prepare(&sql)?;
        let result: Result<String> = stmt.query_row([book_id], |row| {
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

    pub fn update(&self, book_id: i32, field: &str, value: &str) -> Result<()> {
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
    ) -> Result<i32> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (title, sort, author_sort, uuid, series_index) VALUES (?, ?, ?, ?, 1.0)",
            (title, sort, author_sort, uuid),
        )?;
        Ok(conn.last_insert_rowid() as i32)
    }

    pub fn get_all_authors(&self) -> Result<HashMap<i32, String>> {
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

    pub fn get_all_series(&self) -> Result<HashMap<i32, String>> {
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

    pub fn load_prefs(&mut self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, val FROM preferences")?;
        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let val: String = row.get(1)?;
            Ok((key, val))
        })?;

        for row in rows {
            let (key, val) = row?;
            self.prefs.insert(key, val);
        }
        Ok(())
    }
}
