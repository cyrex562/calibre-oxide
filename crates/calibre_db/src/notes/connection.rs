//! Port of `old_src/src/calibre/db/notes/connect.py`'s `Notes` class
//! (issue #227, a #201 follow-up): free-form Markdown/HTML notes
//! attachable to any book/author/tag/etc. field value, separate from
//! the `comments` field, with resource (image/file) attachments and
//! full-text search over note bodies.
//!
//! # Scope of this pass
//!
//! Real, matching `notes_sqlite.sql`/`connect.py`: schema creation
//! (`notes`, `resources`, `notes_resources_link`, two FTS5 virtual
//! tables) and the triggers that keep the FTS index in sync and
//! cascade-clean `notes_resources_link` when a note is deleted; core
//! CRUD ([`NotesConnection::set_note`]/`get_note`/`get_note_data`/
//! `rename_note`/`delete_field`/`items_with_notes_for_field`/
//! `all_items_with_notes`); resource attachment storage
//! ([`NotesConnection::add_resource`]/`get_resource_data`/
//! `path_for_resource`, with real orphaned-resource-file cleanup via
//! [`NotesConnection::remove_unreferenced_resources`]); and real
//! search ([`NotesConnection::all_notes`]/[`NotesConnection::search`],
//! the latter raising the real `FtsQueryError` from #218/#226 on a
//! malformed MATCH query, same real dynamic-SQL shape as
//! `fts/connection.rs`'s `search`).
//!
//! # Disclosed simplifications
//!
//! - **No custom `calibre`/`porter` FTS5 tokenizers** -- same #93
//!   dependency and same `unicode61` fallback as `fts/connection.rs`
//!   (#226); see that file's module doc for the full explanation.
//! - **No retire/backup/undo-trail.** Upstream keeps a `backup_dir`/
//!   `retired_dir` on disk: every `set_note` call also writes a
//!   plain-text backup copy (`set_backup_for`), and deleting a note
//!   moves it to a `retired/` directory instead of just dropping it
//!   (`retire_entry`/`unretire`/`remove_retired_entry`/
//!   `trim_retired_dir`, capped at `max_retired_items`) so a later
//!   `set_note` on the same field/item can silently recover the prior
//!   text. None of that undo-history machinery is ported --
//!   [`NotesConnection::set_note`] with empty `marked_up_text` deletes
//!   the note outright.
//! - **No `resources/<hash>.metadata` JSON sidecar files** -- upstream
//!   writes one per resource (just `{"name": ...}`, used as an export/
//!   `get_resource_data` fallback); the `resources` table's own `name`
//!   column is this crate's single source of truth instead.
//! - **Resource hashing** uses `std::collections::hash_map::DefaultHasher`
//!   (SipHash), not upstream's `xxhash`/`xxh3_64` -- this is purely a
//!   content-addressed identifier for this crate's own resource store
//!   (never compared against or interchanged with a real calibre
//!   library's resource files), so exact algorithm choice doesn't
//!   affect correctness, only the string prefix (`"siphash64:"` here
//!   vs. `"xxh64:"` upstream).
//! - **No `field_metadata`/`supports_notes` gating** -- upstream only
//!   allows notes on fields whose real per-library metadata marks
//!   `supports_notes` (and never on `rating`); no such subsystem
//!   exists in this crate (the recurring #201 gap), so any `field`
//!   string is accepted here.
//! - **No Windows hidden-attribute setting** on the notes directory.
//! - `export_non_db_data`/`restore` (full-library backup/restore
//!   integration) and `export_note`/`import_note` are not ported here
//!   -- the latter is explicitly tracked separately as `notes/exim.rs`
//!   (a distinct, already-filed issue per #227's own description).

use crate::constants::{NOTES_DB_NAME, NOTES_DIR_NAME};
use crate::errors::FtsQueryError;
use calibre_utils::filenames::sanitize_file_name;
use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A full note record -- port of `get_note_data`'s dict.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteData {
    pub id: i32,
    pub doc: String,
    pub searchable_text: String,
    pub ctime: f64,
    pub mtime: f64,
    pub resource_hashes: HashSet<String>,
}

/// A stored resource's bytes -- port of `get_resource_data`'s dict.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceData {
    pub name: String,
    pub data: Vec<u8>,
    pub hash: String,
}

/// One [`NotesConnection::all_notes`]/[`NotesConnection::search`] hit.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteSearchResult {
    pub id: i32,
    pub field: String,
    pub item_id: i32,
    pub text: Option<String>,
}

fn hash_data(data: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    format!("siphash64:{:016x}", hasher.finish())
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub struct NotesConnection {
    conn: Arc<Mutex<Connection>>,
    notes_dir: PathBuf,
    resources_dir: PathBuf,
}

impl NotesConnection {
    pub fn new(conn: Arc<Mutex<Connection>>, library_path: &Path) -> Self {
        let notes_dir = library_path.join(NOTES_DIR_NAME);
        let resources_dir = notes_dir.join("resources");
        NotesConnection {
            conn,
            notes_dir,
            resources_dir,
        }
    }

    /// Attaches `notes.db` (if not already) and creates the real
    /// schema/triggers -- this crate has only ever had schema version
    /// 1, so there's no upgrade ladder to walk, just the initial
    /// creation.
    pub fn initialize(&self) -> SqlResult<()> {
        fs::create_dir_all(&self.resources_dir).ok();

        let conn = self.conn.lock().unwrap();
        let attached: i32 = conn
            .query_row(
                "SELECT count(*) FROM pragma_database_list WHERE name='notes_db'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if attached == 0 {
            let db_path = self.notes_dir.join(NOTES_DB_NAME);
            conn.execute("ATTACH DATABASE ? AS notes_db", [db_path.to_str().unwrap()])?;

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS notes_db.notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    item INTEGER NOT NULL,
                    colname TEXT NOT NULL COLLATE NOCASE,
                    doc TEXT NOT NULL DEFAULT '',
                    searchable_text TEXT NOT NULL DEFAULT '',
                    ctime REAL,
                    mtime REAL,
                    UNIQUE(item, colname)
                );
                CREATE INDEX IF NOT EXISTS notes_db.notes_colname_idx ON notes (colname);
                CREATE TABLE IF NOT EXISTS notes_db.resources (
                    hash TEXT NOT NULL PRIMARY KEY ON CONFLICT FAIL,
                    name TEXT NOT NULL UNIQUE ON CONFLICT FAIL
                );
                CREATE TABLE IF NOT EXISTS notes_db.notes_resources_link (
                    id INTEGER PRIMARY KEY,
                    note INTEGER NOT NULL,
                    resource TEXT NOT NULL,
                    UNIQUE(note, resource)
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS notes_db.notes_fts USING fts5(
                    searchable_text, content = 'notes', content_rowid = 'id'
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS notes_db.notes_fts_stemmed USING fts5(
                    searchable_text, content = 'notes', content_rowid = 'id'
                );
                CREATE TRIGGER IF NOT EXISTS notes_db.notes_fts_insert_trg AFTER INSERT ON notes BEGIN
                    INSERT INTO notes_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    INSERT INTO notes_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                END;
                CREATE TRIGGER IF NOT EXISTS notes_db.notes_delete_trg BEFORE DELETE ON notes BEGIN
                    DELETE FROM notes_resources_link WHERE note=OLD.id;
                    INSERT INTO notes_fts(notes_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO notes_fts_stemmed(notes_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                END;
                CREATE TRIGGER IF NOT EXISTS notes_db.notes_fts_update_trg AFTER UPDATE ON notes BEGIN
                    INSERT INTO notes_fts(notes_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO notes_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                    INSERT INTO notes_fts_stemmed(notes_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
                    INSERT INTO notes_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
                END;
                CREATE TRIGGER IF NOT EXISTS notes_db.resources_delete_trg BEFORE DELETE ON resources BEGIN
                    DELETE FROM notes_resources_link WHERE resource=OLD.hash;
                END;",
            )?;
        }
        Ok(())
    }

    fn note_id_for(&self, conn: &Connection, field: &str, item_id: i32) -> SqlResult<Option<i32>> {
        conn.query_row(
            "SELECT id FROM notes_db.notes WHERE item=?1 AND colname=?2",
            (item_id, field),
            |r| r.get(0),
        )
        .optional()
    }

    fn resources_used_by(&self, conn: &Connection, note_id: i32) -> SqlResult<HashSet<String>> {
        let mut stmt =
            conn.prepare("SELECT resource FROM notes_db.notes_resources_link WHERE note=?1")?;
        let rows = stmt.query_map([note_id], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// Real orphan cleanup: any resource file no longer referenced by
    /// any note's link row is deleted from disk and from `resources`.
    pub fn remove_unreferenced_resources(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let orphans: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT hash FROM notes_db.resources WHERE hash NOT IN (SELECT resource FROM notes_db.notes_resources_link)",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let collected: SqlResult<Vec<String>> = rows.collect();
            collected?
        };
        for hash in &orphans {
            let _ = fs::remove_file(self.path_for_resource(hash));
            conn.execute("DELETE FROM notes_db.resources WHERE hash=?1", [hash])?;
        }
        Ok(())
    }

    /// `resources_dir/<first-2-digest-chars>/<alg>-<digest>`.
    pub fn path_for_resource(&self, resource_hash: &str) -> PathBuf {
        let (alg, digest) = resource_hash
            .split_once(':')
            .unwrap_or(("raw", resource_hash));
        let prefix: String = digest.chars().take(2).collect();
        self.resources_dir
            .join(prefix)
            .join(format!("{alg}-{digest}"))
    }

    /// Stores `data` content-addressed under `path_for_resource`,
    /// registering it in `resources` under `name` (sanitized,
    /// disambiguated with a `-1`/`-2`/... suffix on a name collision
    /// with a *different* resource). Returns the resource hash.
    pub fn add_resource(&self, data: &[u8], name: &str) -> SqlResult<String> {
        let hash = hash_data(data);
        let path = self.path_for_resource(&hash);
        let needs_write = fs::metadata(&path)
            .map(|m| m.len() as usize != data.len())
            .unwrap_or(true);
        if needs_write {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::write(&path, data).ok();
        }

        let sanitized = sanitize_file_name(name);
        let (base, ext) = match sanitized.rsplit_once('.') {
            Some((b, e)) => (b.to_string(), format!(".{e}")),
            None => (sanitized.clone(), String::new()),
        };

        let conn = self.conn.lock().unwrap();
        let existing_name: Option<String> = conn
            .query_row(
                "SELECT name FROM notes_db.resources WHERE hash=?1",
                [&hash],
                |r| r.get(0),
            )
            .optional()?;
        match existing_name {
            Some(existing) if existing != sanitized => {
                let mut candidate = sanitized;
                let mut n = 0;
                loop {
                    match conn.execute(
                        "UPDATE notes_db.resources SET name=?1 WHERE hash=?2",
                        (&candidate, &hash),
                    ) {
                        Ok(_) => break,
                        Err(_) => {
                            n += 1;
                            candidate = format!("{base}-{n}{ext}");
                        }
                    }
                }
            }
            Some(_) => {}
            None => {
                let mut candidate = sanitized;
                let mut n = 0;
                loop {
                    match conn.execute(
                        "INSERT INTO notes_db.resources (hash, name) VALUES (?1, ?2)",
                        (&hash, &candidate),
                    ) {
                        Ok(_) => break,
                        Err(_) => {
                            n += 1;
                            candidate = format!("{base}-{n}{ext}");
                        }
                    }
                }
            }
        }
        Ok(hash)
    }

    pub fn get_resource_data(&self, resource_hash: &str) -> SqlResult<Option<ResourceData>> {
        let conn = self.conn.lock().unwrap();
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM notes_db.resources WHERE hash=?1",
                [resource_hash],
                |r| r.get(0),
            )
            .optional()?;
        let Some(name) = name else { return Ok(None) };
        let path = self.path_for_resource(resource_hash);
        Ok(fs::read(path).ok().map(|data| ResourceData {
            name,
            data,
            hash: resource_hash.to_string(),
        }))
    }

    /// Real port of `set_note`. `item_value` is the plain-text label
    /// of the item the note is attached to (e.g. a book's title) and
    /// is prepended to `searchable_text`, matching upstream's default
    /// `add_item_value_to_searchable_text=True`. An empty
    /// `marked_up_text` deletes the note (returning `-1`) instead of
    /// creating one -- no retire/backup, see the module doc.
    pub fn set_note(
        &self,
        field: &str,
        item_id: i32,
        item_value: &str,
        marked_up_text: &str,
        used_resource_hashes: &HashSet<String>,
    ) -> SqlResult<i32> {
        let conn = self.conn.lock().unwrap();
        let note_id = self.note_id_for(&conn, field, item_id)?;
        let old_resources = match note_id {
            Some(id) => self.resources_used_by(&conn, id)?,
            None => HashSet::new(),
        };

        if marked_up_text.is_empty() {
            if let Some(id) = note_id {
                conn.execute("DELETE FROM notes_db.notes WHERE id=?1", [id])?;
                drop(conn);
                if !old_resources.is_empty() {
                    self.remove_unreferenced_resources()?;
                }
            }
            return Ok(-1);
        }

        let searchable_text = format!("{item_value}\n{marked_up_text}");
        let new_id = match note_id {
            Some(id) => {
                conn.execute(
                    "UPDATE notes_db.notes SET doc=?1, searchable_text=?2, mtime=?3 WHERE id=?4",
                    (marked_up_text, &searchable_text, now_secs(), id),
                )?;
                id
            }
            None => {
                let now = now_secs();
                conn.execute(
                    "INSERT INTO notes_db.notes (item, colname, doc, searchable_text, ctime, mtime) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (item_id, field, marked_up_text, &searchable_text, now, now),
                )?;
                conn.last_insert_rowid() as i32
            }
        };

        let to_remove: Vec<&String> = old_resources.difference(used_resource_hashes).collect();
        let to_add: Vec<&String> = used_resource_hashes.difference(&old_resources).collect();
        for hash in &to_remove {
            conn.execute(
                "DELETE FROM notes_db.notes_resources_link WHERE note=?1 AND resource=?2",
                (new_id, hash.as_str()),
            )?;
        }
        for hash in &to_add {
            conn.execute(
                "INSERT INTO notes_db.notes_resources_link (note, resource) VALUES (?1, ?2)",
                (new_id, hash.as_str()),
            )?;
        }
        let had_removals = !to_remove.is_empty();
        drop(conn);
        if had_removals {
            self.remove_unreferenced_resources()?;
        }
        Ok(new_id)
    }

    pub fn get_note(&self, field: &str, item_id: i32) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT doc FROM notes_db.notes WHERE item=?1 AND colname=?2",
            (item_id, field),
            |r| r.get(0),
        )
        .optional()
    }

    pub fn get_note_data(&self, field: &str, item_id: i32) -> SqlResult<Option<NoteData>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, doc, searchable_text, ctime, mtime FROM notes_db.notes WHERE item=?1 AND colname=?2",
                (item_id, field),
                |r| {
                    Ok((
                        r.get::<_, i32>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, f64>(3)?,
                        r.get::<_, f64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, doc, searchable_text, ctime, mtime)) = row else {
            return Ok(None);
        };
        let resource_hashes = self.resources_used_by(&conn, id)?;
        Ok(Some(NoteData {
            id,
            doc,
            searchable_text,
            ctime,
            mtime,
            resource_hashes,
        }))
    }

    pub fn items_with_notes_for_field(&self, field: &str) -> SqlResult<HashSet<i32>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT item FROM notes_db.notes WHERE colname=?1")?;
        let rows = stmt.query_map([field], |r| r.get::<_, i32>(0))?;
        rows.collect()
    }

    pub fn all_items_with_notes(
        &self,
    ) -> SqlResult<std::collections::HashMap<String, HashSet<i32>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT item, colname FROM notes_db.notes")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))?;
        let mut out: std::collections::HashMap<String, HashSet<i32>> =
            std::collections::HashMap::new();
        for row in rows {
            let (item, colname) = row?;
            out.entry(colname).or_default().insert(item);
        }
        Ok(out)
    }

    /// Real port of `rename_note`: moves an existing note from
    /// `old_item_id` to `new_item_id` (e.g. after a tag rename),
    /// re-deriving `searchable_text` from `new_item_value`. A no-op if
    /// there's nothing to move or `new_item_id` already has a note.
    pub fn rename_note(
        &self,
        field: &str,
        old_item_id: i32,
        new_item_id: i32,
        new_item_value: &str,
    ) -> SqlResult<()> {
        let old = self.get_note_data(field, old_item_id)?;
        let Some(old) = old else { return Ok(()) };
        if old.doc.is_empty() {
            return Ok(());
        }
        if self.get_note(field, new_item_id)?.is_some() {
            return Ok(());
        }
        self.set_note(
            field,
            new_item_id,
            new_item_value,
            &old.doc,
            &old.resource_hashes,
        )?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM notes_db.notes WHERE id=?1", [old.id])?;
        Ok(())
    }

    /// Removes every note attached to `field` -- used when a whole
    /// custom column is deleted.
    pub fn delete_field(&self, field: &str) -> SqlResult<()> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute("DELETE FROM notes_db.notes WHERE colname=?1", [field])?;
        }
        self.remove_unreferenced_resources()
    }

    pub fn vacuum(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("VACUUM notes_db")
    }

    /// Port of `all_notes`: every note (optionally restricted to
    /// `restrict_to_fields`), newest-first, with a truncated text
    /// snippet -- browsing/listing, not a MATCH query.
    pub fn all_notes(
        &self,
        restrict_to_fields: &[&str],
        limit: Option<usize>,
        snippet_size: usize,
    ) -> SqlResult<Vec<NoteSearchResult>> {
        let char_size = snippet_size.max(1) * 8;
        let mut query = format!(
            "SELECT notes.id, notes.colname, notes.item, substr(notes.searchable_text, 1, {char_size}) FROM notes_db.notes AS notes"
        );
        if !restrict_to_fields.is_empty() {
            let placeholders = vec!["?"; restrict_to_fields.len()].join(",");
            query.push_str(&format!(" WHERE notes.colname IN ({placeholders})"));
        }
        query.push_str(" ORDER BY notes.mtime DESC");
        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {limit}"));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&query)?;
        let params = rusqlite::params_from_iter(restrict_to_fields.iter());
        let rows = stmt.query_map(params, |r| {
            Ok(NoteSearchResult {
                id: r.get(0)?,
                field: r.get(1)?,
                item_id: r.get(2)?,
                text: r.get::<_, Option<String>>(3)?,
            })
        })?;
        rows.collect()
    }

    /// Port of `Notes.search`: a real FTS5 MATCH query when
    /// `fts_engine_query` is non-empty, falling back to
    /// [`NotesConnection::all_notes`] when it's empty (matching
    /// upstream). Real `FtsQueryError` on a malformed MATCH query.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        fts_engine_query: &str,
        use_stemming: bool,
        highlight: Option<(&str, &str)>,
        snippet_size: Option<usize>,
        restrict_to_fields: &[&str],
        return_text: bool,
        limit: Option<usize>,
    ) -> Result<Vec<NoteSearchResult>, FtsQueryError> {
        if fts_engine_query.is_empty() {
            return self
                .all_notes(restrict_to_fields, limit, snippet_size.unwrap_or(64))
                .map_err(|e| sql_err(fts_engine_query, "all_notes", &e));
        }
        let fts_table = if use_stemming {
            "notes_fts_stemmed"
        } else {
            "notes_fts"
        };

        let mut text_col = String::new();
        let mut hl_params: Vec<String> = Vec::new();
        if return_text {
            text_col = if let Some((start, end)) = highlight {
                hl_params.push(start.to_string());
                hl_params.push(end.to_string());
                match snippet_size {
                    Some(n) => format!(
                        ", snippet(\"{fts_table}\", 0, ?, ?, '…', {})",
                        n.clamp(1, 64)
                    ),
                    None => format!(", highlight(\"{fts_table}\", 0, ?, ?)"),
                }
            } else {
                ", notes.searchable_text".to_string()
            };
        }

        let mut query = format!(
            "SELECT notes.id, notes.colname, notes.item{text_col} FROM notes_db.notes AS notes"
        );
        query.push_str(&format!(
            " JOIN {fts_table} ON notes_db.notes.id = {fts_table}.rowid WHERE"
        ));
        if !restrict_to_fields.is_empty() {
            let placeholders = vec!["?"; restrict_to_fields.len()].join(",");
            query.push_str(&format!(" notes.colname IN ({placeholders}) AND"));
        }
        query.push_str(&format!(" \"{fts_table}\" MATCH ?"));
        query.push_str(&format!(" ORDER BY {fts_table}.rank"));
        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {limit}"));
        }

        let mut params: Vec<rusqlite::types::Value> =
            hl_params.into_iter().map(Into::into).collect();
        params.extend(restrict_to_fields.iter().map(|f| f.to_string().into()));
        params.push(fts_engine_query.to_string().into());

        let conn = self.conn.lock().unwrap();
        let result = (|| -> SqlResult<Vec<NoteSearchResult>> {
            let mut stmt = conn.prepare(&query)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
                Ok(NoteSearchResult {
                    id: r.get(0)?,
                    field: r.get(1)?,
                    item_id: r.get(2)?,
                    text: if return_text {
                        r.get::<_, Option<String>>(3)?
                    } else {
                        None
                    },
                })
            })?;
            rows.collect()
        })();
        result.map_err(|e| sql_err(fts_engine_query, &query, &e))
    }
}

fn sql_err(query: &str, sql_statement: &str, e: &rusqlite::Error) -> FtsQueryError {
    FtsQueryError {
        query: query.to_string(),
        sql_statement: sql_statement.to_string(),
        apsw_error: e.to_string(),
    }
}
