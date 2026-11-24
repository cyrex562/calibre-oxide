//! Bringing a calibre library that sits **inside** this library in, in place
//! (tranche 14a).
//!
//! The collection this is built for is a NAS root holding several calibre
//! libraries (each a folder with a `metadata.db` and calibre's
//! `Author/Title (id)/` layout) next to loose folders of books. One Lexmechanic
//! library rooted at the share should see all of it, so that duplicate
//! detection works across everything. The scan used to step around a folder
//! holding a `metadata.db` (descending would double-track its books), and the
//! folder import refuses a source inside the library, so a nested calibre
//! library had no way in. This is the way in.
//!
//! # What absorbing does
//!
//! [`absorb_library`] reads the nested `metadata.db` **read-only** and
//! registers each of its books **in place**: `books.path` becomes the nested
//! folder's path relative to this library, followed by calibre's own
//! `Author/Title (id)`. No file is moved, copied or written. Everything the
//! database knows about the book is carried (see [`NOT_CARRIED`] for what is
//! not): title, sort, authors with their sort and link, series and index,
//! tags, publisher, identifiers, languages, rating, comments, the three
//! dates (timestamp is "date added"), the calibre uuid, the formats with
//! their real names and sizes, the cover, every custom column of every kind
//! (columns this library lacks are created with the same label, type and
//! display), annotations and reading positions.
//!
//! * **Covers** are not copied. A calibre cover is `<book folder>/cover.jpg`,
//!   which is exactly the location [`crate::covers::legacy_cover_path`] reads
//!   from, so the book simply points at it. Setting a new cover later writes
//!   the `.lexmechanic/covers/<uuid>.jpg` sidecar and retires the old file,
//!   as for any other book.
//! * **Uuids** are kept. If this library already has a book with the same
//!   uuid (the same library copied twice, say) the book gets a fresh uuid and
//!   the mapping is recorded in the report and in
//!   `.lexmechanic/absorbed-libraries.json`.
//! * **Hashes**: every format file is streamed through BLAKE3 in the job's own
//!   read (in parallel, bounded like the scan's), so content duplicate
//!   detection sees the books at once and the hash backfill finds nothing to
//!   do.
//!
//! Writes are batched like the scan's: one `BEGIN IMMEDIATE` transaction per
//! [`PipelineOptions::batch_size`] books (a savepoint each, so one bad book is
//! rolled back alone) and one change-log batch entry, database first, log
//! second.
//!
//! # Resume
//!
//! The database is the record of progress. A source book is already absorbed
//! when this library has a book whose `path` is the prefixed path of it
//! (calibre's folder name carries the book id, so the path identifies the
//! book); a cancelled or crashed absorb is resumed by running it again, and
//! what was done is skipped without being read. `.lexmechanic/
//! absorbed-libraries.json` additionally records, per nested library, its
//! state, counts and uuid remaps.
//!
//! # Finishing, and undoing
//!
//! When every book is in (no failures, not cancelled) the nested `metadata.db`
//! is renamed to [`ABSORBED_DB_NAME`] (and its `-wal`/`-shm`/`-journal` get
//! the same suffix), so the scan no longer treats the folder as a library of
//! its own and descends into it: the absorbed books' files are then found to
//! be tracked already, not new. **Undo by renaming the file back**, or call
//! [`unabsorb_library`], which does that and removes the absorbed books from
//! this library's database (rows only; never a file).
//!
//! # Safety
//!
//! The nested database is opened with `SQLITE_OPEN_READ_ONLY` and `query_only`
//! and is never written; the only thing done to it is the final rename.
//! Absorbing is refused when calibre appears to be using it (the in-use probes
//! of [`crate::storage_safety`], and a non-empty `-journal`/`-wal` file), for
//! a path outside the library, for the library root itself, and for a folder
//! that is a Lexmechanic library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::cache::Cache;
use crate::change_log::ChangeOp;
use crate::constants::LIBRARY_HANDLE_DIR_NAME;
use crate::custom_columns::ColumnDef;
use crate::scan_pipeline::{format_file_path, hash_stream, parallel_map, FileDigest, JobControl, PipelineOptions, MAX_REPORTED};

/// What a nested library's `metadata.db` is renamed to once its books are in.
pub const ABSORBED_DB_NAME: &str = "metadata.db.absorbed-by-lexmechanic";
/// What a database absorbed before the rename was parked as. Still
/// recognised (a library absorbed long ago must stay absorbed, and its undo
/// must still work), never written.
pub const LEGACY_ABSORBED_DB_NAME: &str = "metadata.db.absorbed-by-calibre-oxide";

/// Whether `dir` holds a parked (absorbed) database under either name.
fn has_parked_database(dir: &Path) -> bool {
    dir.join(ABSORBED_DB_NAME).is_file() || dir.join(LEGACY_ABSORBED_DB_NAME).is_file()
}

/// The suffixes of SQLite's companion files, renamed along with the database.
const COMPANIONS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// What is deliberately not carried over.
pub const NOT_CARRIED: &[&str] = &[
    "calibre's saved searches, virtual libraries and other library preferences (the preferences table)",
    "notes (the .calnotes folder) and conversion options",
    "per-author link/sort edits for authors this library already has (the existing author wins)",
    "the date last modified in the change log (it is stored, but cannot be replayed)",
    "annotations and reading positions in the change log (they are stored in the database only)",
];

/// How long [`find_absorbable`] may walk before reporting what it has.
pub const FIND_BUDGET: Duration = Duration::from_secs(4);

/// Nested libraries listed at most.
const MAX_LISTED: usize = 500;

// ---------------------------------------------------------------------
// Finding nested libraries
// ---------------------------------------------------------------------

/// A calibre library found inside this library.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AbsorbableLibrary {
    /// Relative to this library, `/`-separated.
    pub path: String,
    /// `available`: holds a live `metadata.db`. `absorbed`: has been
    /// absorbed (its database carries [`ABSORBED_DB_NAME`]).
    pub state: &'static str,
    /// For `available`, the books in the nested database; for `absorbed`,
    /// the books of this library under that folder.
    pub books: u64,
    /// For `available`: how many of those are in this library already (a
    /// partly done absorb).
    pub already_absorbed: u64,
    /// Why the nested database could not be read, if it could not.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AbsorbableList {
    pub libraries: Vec<AbsorbableLibrary>,
    /// The walk covered every folder: false if it ran out of time or a
    /// folder could not be read.
    pub conclusive: bool,
}

/// Lists the calibre libraries inside `cache`'s library: a bounded,
/// time-budgeted walk of folders only (no file is stat'ed), like
/// [`crate::scan::count_untracked`]. Does not descend into a calibre library
/// it finds (its subfolders are its books).
pub fn find_absorbable(cache: &Cache, budget: Duration) -> Result<AbsorbableList> {
    let root = cache.backend.library_path.clone();
    let deadline = Instant::now() + budget;
    let mut out = AbsorbableList { conclusive: true, ..Default::default() };
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        if Instant::now() >= deadline || out.libraries.len() >= MAX_LISTED {
            out.conclusive = false;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            out.conclusive = false;
            continue;
        };
        let (mut live, mut absorbed, mut handle) = (false, false, false);
        let mut subdirs = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                out.conclusive = false;
                continue;
            };
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if ft.is_dir() {
                if name == LIBRARY_HANDLE_DIR_NAME || name == crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME {
                    handle = true;
                } else if !crate::scan::is_junk_dir(&name) && !(dir == root && name == crate::constants::DATA_DIR_NAME) {
                    subdirs.push(entry.path());
                }
            } else if name == "metadata.db" {
                live = true;
            } else if name == ABSORBED_DB_NAME || name == LEGACY_ABSORBED_DB_NAME {
                absorbed = true;
            }
        }
        if dir != root && !handle && (live || absorbed) {
            let rel = relative(&root, &dir);
            let listed = if live {
                let (books, problem) = match count_source_books(&dir.join("metadata.db")) {
                    Ok(n) => (n, None),
                    Err(e) => (0, Some(format!("{e:#}"))),
                };
                AbsorbableLibrary { already_absorbed: count_absorbed(cache, &rel).unwrap_or(0), path: rel, state: "available", books, problem }
            } else {
                AbsorbableLibrary { books: count_absorbed(cache, &rel).unwrap_or(0), path: rel, state: "absorbed", already_absorbed: 0, problem: None }
            };
            out.libraries.push(listed);
            continue; // its subfolders are its books
        }
        if handle && dir != root {
            continue; // a Lexmechanic library of its own
        }
        subdirs.sort();
        // Depth-first in name order: reversed onto the stack.
        stack.extend(subdirs.into_iter().rev());
    }
    out.libraries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn relative(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default()
}

fn open_source(db: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).with_context(|| format!("opening {} read-only", db.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

fn is_calibre_schema(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('books','data','authors','books_authors_link','custom_columns')", [], |r| r.get(0))?;
    Ok(n == 5)
}

fn count_source_books(db: &Path) -> Result<u64> {
    let conn = open_source(db)?;
    if !is_calibre_schema(&conn)? {
        bail!("not a calibre library database");
    }
    Ok(conn.query_row("SELECT COUNT(*) FROM books", [], |r| r.get::<_, i64>(0))? as u64)
}

/// `LIKE` pattern and exact match for "this folder or below".
fn prefix_patterns(prefix: &str) -> (String, String) {
    let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    (prefix.to_string(), format!("{escaped}/%"))
}

fn count_absorbed(cache: &Cache, prefix: &str) -> Result<u64> {
    let (exact, like) = prefix_patterns(prefix);
    let conn = cache.backend.conn.lock().unwrap();
    Ok(conn.query_row("SELECT COUNT(*) FROM books WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\'", (&exact, &like), |r| r.get::<_, i64>(0))? as u64)
}

// ---------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct AbsorbOptions {
    /// Finish (rename the database) even though some books failed. Those
    /// books' files are then found by the next scan as loose files.
    pub allow_failures: bool,
    /// Leave the database where it is when done.
    pub keep_database_name: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AbsorbProblem {
    /// The book's id in the nested library, and its title.
    pub source_id: i64,
    pub title: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UuidRemap {
    pub source_uuid: String,
    pub new_uuid: String,
    pub book_id: i32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AbsorbReport {
    /// The nested library, relative to this one.
    pub folder: String,
    pub source_books: usize,
    /// Books registered by this run.
    pub absorbed: usize,
    /// Books that were in already (an earlier, partial run).
    pub already_absorbed: usize,
    pub formats: usize,
    pub covers: usize,
    pub annotations: usize,
    pub custom_values: usize,
    pub failed: Vec<AbsorbProblem>,
    pub failed_count: usize,
    /// Format and cover files the nested database lists that are not on
    /// disk. The book is registered without them.
    pub missing_files: Vec<String>,
    pub missing_count: usize,
    pub uuid_remapped: Vec<UuidRemap>,
    pub custom_columns_created: Vec<String>,
    /// Columns whose label exists here with another type: their values were
    /// not carried.
    pub custom_column_conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
    /// The nested `metadata.db` was renamed to [`ABSORBED_DB_NAME`].
    pub renamed: bool,
    /// Why it was not, when every book is not in.
    pub not_renamed_because: Option<String>,
    pub not_carried: Vec<&'static str>,
}

fn push_capped<T>(list: &mut Vec<T>, item: T) {
    if list.len() < MAX_REPORTED {
        list.push(item);
    }
}

/// Resolves what the user named (absolute, or relative to the library) to the
/// folder's canonical path and its `/`-separated path relative to the library.
pub fn resolve_folder(cache: &Cache, folder: &str) -> Result<(PathBuf, String)> {
    let library = std::fs::canonicalize(&cache.backend.library_path).context("resolving the library folder")?;
    let given = folder.trim();
    if given.is_empty() {
        bail!("a folder is needed");
    }
    let candidate = if Path::new(given).is_absolute() { PathBuf::from(given) } else { library.join(given) };
    let abs = std::fs::canonicalize(&candidate).with_context(|| format!("{} is not a folder this server can read", candidate.display()))?;
    if !abs.is_dir() {
        bail!("{} is not a folder", abs.display());
    }
    let rel = abs.strip_prefix(&library).map_err(|_| anyhow!("{} is outside the library; only a folder inside the library can be absorbed", abs.display()))?;
    let rel = rel.to_str().ok_or_else(|| anyhow!("the folder's name is not valid text"))?.replace('\\', "/");
    if rel.is_empty() {
        bail!("that is the library itself; choose a calibre library folder inside it");
    }
    Ok((abs, rel))
}

/// Refuses when something looks to be using the database: the in-use probes of
/// `storage_safety` (an exclusive open denied), a non-empty rollback journal
/// or write-ahead log (calibre mid-write, or died mid-write). A running calibre
/// that has not got this library open is only a warning.
fn check_not_in_use(db: &Path) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for suffix in ["-journal", "-wal"] {
        let mut name = db.as_os_str().to_os_string();
        name.push(suffix);
        if std::fs::metadata(PathBuf::from(name)).map(|m| m.len() > 0).unwrap_or(false) {
            bail!("calibre appears to be using this library right now (metadata.db{suffix} is not empty); close calibre, or let it finish, and try again");
        }
    }
    let mut report = crate::storage_safety::InUseReport::default();
    crate::storage_safety::probe_before_open(db, false, &mut report);
    if let Some(reason) = report.blocking_message() {
        bail!("calibre appears to be using this library: {reason}");
    }
    warnings.extend(report.signals.iter().map(|s| s.message.clone()));
    Ok(warnings)
}

struct SourceAuthor {
    name: String,
    sort: Option<String>,
    link: String,
}

enum Custom {
    /// A column stored in one table: the value as SQLite holds it.
    Plain(Sql),
    /// A normalized column: values in link order, each with the series index.
    Items(Vec<(Sql, Option<f64>)>),
}

struct SourceBook {
    id: i64,
    title: String,
    sort: Option<String>,
    author_sort: Option<String>,
    series_index: f64,
    timestamp: Option<String>,
    pubdate: Option<String>,
    last_modified: Option<String>,
    path: String,
    has_cover: bool,
    isbn: String,
    lccn: String,
    uuid: String,
    authors: Vec<SourceAuthor>,
    series: Option<(String, Option<String>, String)>,
    publisher: Option<(String, Option<String>, String)>,
    tags: Vec<(String, String)>,
    languages: Vec<(String, String)>,
    identifiers: Vec<(String, String)>,
    rating: Option<i64>,
    comments: Option<String>,
    /// `(format, recorded size, file stem)`.
    formats: Vec<(String, i64, String)>,
    custom: Vec<(usize, Custom)>,
    annotations: Vec<[Sql; 8]>,
    positions: Vec<[Sql; 7]>,
}

fn text(v: Sql) -> Option<String> {
    match v {
        Sql::Null => None,
        Sql::Text(s) => Some(s),
        Sql::Integer(n) => Some(n.to_string()),
        Sql::Real(f) => Some(f.to_string()),
        Sql::Blob(b) => Some(String::from_utf8_lossy(&b).into_owned()),
    }
}

struct SourceReader<'a> {
    conn: &'a Connection,
    has_annotations: bool,
    has_positions: bool,
    /// Matched columns: `(source column, destination column)`.
    columns: &'a [(ColumnDef, ColumnDef)],
}

impl SourceReader<'_> {
    fn read(&self, id: i64) -> Result<SourceBook> {
        let c = self.conn;
        let mut book = c.query_row(
            "SELECT title, sort, author_sort, series_index, timestamp, pubdate, last_modified, path, has_cover, isbn, lccn, uuid FROM books WHERE id = ?1",
            [id],
            |r| {
                Ok(SourceBook {
                    id,
                    title: text(r.get::<_, Sql>(0)?).unwrap_or_else(|| "Unknown".to_string()),
                    sort: text(r.get::<_, Sql>(1)?),
                    author_sort: text(r.get::<_, Sql>(2)?),
                    series_index: match r.get::<_, Sql>(3)? {
                        Sql::Real(f) => f,
                        Sql::Integer(n) => n as f64,
                        _ => 1.0,
                    },
                    timestamp: text(r.get::<_, Sql>(4)?),
                    pubdate: text(r.get::<_, Sql>(5)?),
                    last_modified: text(r.get::<_, Sql>(6)?),
                    path: text(r.get::<_, Sql>(7)?).unwrap_or_default().replace('\\', "/"),
                    has_cover: matches!(r.get::<_, Sql>(8)?, Sql::Integer(n) if n != 0),
                    isbn: text(r.get::<_, Sql>(9)?).unwrap_or_default(),
                    lccn: text(r.get::<_, Sql>(10)?).unwrap_or_default(),
                    uuid: text(r.get::<_, Sql>(11)?).unwrap_or_default(),
                    authors: Vec::new(),
                    series: None,
                    publisher: None,
                    tags: Vec::new(),
                    languages: Vec::new(),
                    identifiers: Vec::new(),
                    rating: None,
                    comments: None,
                    formats: Vec::new(),
                    custom: Vec::new(),
                    annotations: Vec::new(),
                    positions: Vec::new(),
                })
            },
        )?;

        book.authors = c
            .prepare_cached("SELECT a.name, a.sort, a.link FROM books_authors_link l JOIN authors a ON a.id = l.author WHERE l.book = ?1 ORDER BY l.id")?
            .query_map([id], |r| Ok(SourceAuthor { name: r.get(0)?, sort: r.get(1)?, link: r.get::<_, Option<String>>(2)?.unwrap_or_default() }))?
            .collect::<rusqlite::Result<_>>()?;
        book.series = c
            .query_row("SELECT s.name, s.sort, s.link FROM series s JOIN books_series_link l ON l.series = s.id WHERE l.book = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))
            .optional()?;
        book.publisher = c
            .query_row("SELECT p.name, p.sort, p.link FROM publishers p JOIN books_publishers_link l ON l.publisher = p.id WHERE l.book = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))
            .optional()?;
        book.tags = c
            .prepare_cached("SELECT t.name, t.link FROM tags t JOIN books_tags_link l ON l.tag = t.id WHERE l.book = ?1 ORDER BY l.id")?
            .query_map([id], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?
            .collect::<rusqlite::Result<_>>()?;
        book.languages = c
            .prepare_cached("SELECT g.lang_code, g.link FROM languages g JOIN books_languages_link l ON l.lang_code = g.id WHERE l.book = ?1 ORDER BY l.item_order, l.id")?
            .query_map([id], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?
            .collect::<rusqlite::Result<_>>()?;
        book.identifiers = c
            .prepare_cached("SELECT type, val FROM identifiers WHERE book = ?1 ORDER BY id")?
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        book.rating = c
            .query_row("SELECT r.rating FROM ratings r JOIN books_ratings_link l ON l.rating = r.id WHERE l.book = ?1", [id], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten();
        book.comments = c.query_row("SELECT text FROM comments WHERE book = ?1", [id], |r| r.get::<_, String>(0)).optional()?;
        book.formats = c
            .prepare_cached("SELECT format, uncompressed_size, name FROM data WHERE book = ?1 ORDER BY id")?
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get::<_, String>(2)?)))?
            .collect::<rusqlite::Result<_>>()?;

        for (i, (src, _)) in self.columns.iter().enumerate() {
            if src.datatype == "composite" {
                continue;
            }
            let table = src.table();
            if !src.normalized {
                let v: Option<Sql> = c.query_row(&format!("SELECT value FROM {table} WHERE book = ?1"), [id], |r| r.get::<_, Sql>(0)).optional()?;
                if let Some(v) = v.filter(|v| !matches!(v, Sql::Null)) {
                    book.custom.push((i, Custom::Plain(v)));
                }
            } else {
                let extra = if src.datatype == "series" { "l.extra" } else { "NULL" };
                let items: Vec<(Sql, Option<f64>)> = c
                    .prepare(&format!("SELECT v.value, {extra} FROM {} l JOIN {table} v ON v.id = l.value WHERE l.book = ?1 ORDER BY l.id", src.link_table()))?
                    .query_map([id], |r| Ok((r.get::<_, Sql>(0)?, r.get::<_, Option<f64>>(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                if !items.is_empty() {
                    book.custom.push((i, Custom::Items(items)));
                }
            }
        }

        if self.has_annotations {
            book.annotations = c
                .prepare_cached("SELECT format, user_type, user, timestamp, annot_id, annot_type, annot_data, searchable_text FROM annotations WHERE book = ?1")?
                .query_map([id], |r| {
                    let mut row: [Sql; 8] = std::array::from_fn(|_| Sql::Null);
                    for (i, cell) in row.iter_mut().enumerate() {
                        *cell = r.get(i)?;
                    }
                    Ok(row)
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        if self.has_positions {
            book.positions = c
                .prepare_cached("SELECT format, user, device, cfi, epoch, pos_frac FROM last_read_positions WHERE book = ?1")?
                .query_map([id], |r| {
                    let mut row: [Sql; 7] = std::array::from_fn(|_| Sql::Null);
                    for (i, cell) in row.iter_mut().take(6).enumerate() {
                        *cell = r.get(i)?;
                    }
                    Ok(row)
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(book)
    }
}

/// A file to read for a book in the batch.
struct FileJob {
    book: usize,
    /// `Some(i)`: the book's `i`th format; `None`: its cover.
    format: Option<usize>,
    path: Option<PathBuf>,
}

type Hashed = Option<std::result::Result<(FileDigest, Option<lexmechanic_utils::filenames::FileIdentity>), String>>;

/// What one book's registration produced, for the log and the report.
struct Done {
    id: i32,
    ops: Vec<ChangeOp>,
    remap: Option<UuidRemap>,
    formats: usize,
    cover: bool,
    annotations: usize,
    custom_values: usize,
}

fn sql_text(v: &Sql, def: &ColumnDef) -> String {
    match v {
        Sql::Null => String::new(),
        Sql::Integer(n) => n.to_string(),
        Sql::Real(f) => {
            if def.datatype == "int" || def.datatype == "bool" {
                (*f as i64).to_string()
            } else {
                f.to_string()
            }
        }
        Sql::Text(s) => s.clone(),
        Sql::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

/// An id in a name-keyed table, creating the row (with its sort and link)
/// when there is none.
fn intern(tx: &Connection, table: &str, column: &str, name: &str, sort: Option<&str>, link: &str, has_sort: bool) -> Result<i64> {
    if let Some(id) = tx.query_row(&format!("SELECT id FROM {table} WHERE {column} = ?1"), [name], |r| r.get::<_, i64>(0)).optional()? {
        return Ok(id);
    }
    tx.execute(&format!("INSERT INTO {table} ({column}) VALUES (?1)"), [name])?;
    let id = tx.last_insert_rowid();
    // The insert triggers compute a sort; the source's own wins.
    if has_sort {
        if let Some(sort) = sort {
            tx.execute(&format!("UPDATE {table} SET sort = ?1 WHERE id = ?2"), (sort, id))?;
        }
    }
    if !link.is_empty() {
        tx.execute(&format!("UPDATE {table} SET link = ?1 WHERE id = ?2"), (link, id))?;
    }
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
fn register_one(tx: &Connection, src: &SourceBook, prefixed: &str, columns: &[(ColumnDef, ColumnDef)], hashed: &HashMap<(usize, Option<usize>), (FileDigest, Option<lexmechanic_utils::filenames::FileIdentity>)>, book_index: usize) -> Result<Done> {
    let first_author = src.authors.first().map(|a| a.name.as_str()).unwrap_or("Unknown");
    tx.execute(
        "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index, isbn, lccn, last_modified)
         VALUES (?1, ?2, ?3, 0, COALESCE(?4, CURRENT_TIMESTAMP), COALESCE(?5, CURRENT_TIMESTAMP), ?6, ?7, ?8, COALESCE(?9, '2000-01-01 00:00:00+00:00'))",
        rusqlite::params![src.title, src.author_sort.as_deref().unwrap_or(first_author), prefixed, src.timestamp, src.pubdate, src.series_index, src.isbn, src.lccn, src.last_modified],
    )?;
    let id = tx.last_insert_rowid() as i32;
    // `books_insert_trg` has just given the row a uuid and a computed sort.
    let generated: String = tx.query_row("SELECT uuid FROM books WHERE id = ?1", [id], |r| r.get(0))?;
    let mut uuid = generated.clone();
    let mut remap = None;
    if !src.uuid.is_empty() {
        let taken: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM books WHERE uuid = ?1 AND id != ?2)", (&src.uuid, id), |r| r.get(0))?;
        if taken {
            remap = Some(UuidRemap { source_uuid: src.uuid.clone(), new_uuid: generated.clone(), book_id: id });
        } else {
            tx.execute("UPDATE books SET uuid = ?1 WHERE id = ?2", (&src.uuid, id))?;
            uuid = src.uuid.clone();
        }
    }
    if let Some(sort) = src.sort.as_deref().filter(|s| !s.is_empty()) {
        tx.execute("UPDATE books SET sort = ?1 WHERE id = ?2", (sort, id))?;
    }
    // Written after the trigger-sensitive updates above: this and the
    // timestamps are the source's, whatever the insert did.
    tx.execute("UPDATE books SET last_modified = COALESCE(?1, last_modified) WHERE id = ?2", (&src.last_modified, id))?;

    let mut ops = vec![ChangeOp::BookAdded { book: uuid.clone() }];
    let mut field = |name: &str, value: String| ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: name.to_string(), value: Some(value) });
    field("title", src.title.clone());
    if let Some(sort) = src.sort.as_deref().filter(|s| !s.is_empty()) {
        field("sort", sort.to_string());
    }
    field("author_sort", src.author_sort.clone().unwrap_or_else(|| first_author.to_string()));
    field("path", prefixed.to_string());
    field("series_index", src.series_index.to_string());
    let (ts, pd): (String, String) = tx.query_row("SELECT timestamp, pubdate FROM books WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    field("timestamp", ts);
    field("pubdate", pd);
    if !src.isbn.is_empty() {
        field("isbn", src.isbn.clone());
    }
    if !src.lccn.is_empty() {
        field("lccn", src.lccn.clone());
    }

    // Authors, in order, with the source's sort and link for authors that
    // are new here.
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for a in &src.authors {
        let aid = intern(tx, "authors", "name", &a.name, a.sort.as_deref(), &a.link, true)?;
        if seen.insert(aid) {
            tx.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (id, aid))?;
            names.push(a.name.clone());
        }
    }
    if names.is_empty() {
        let aid = intern(tx, "authors", "name", "Unknown", None, "", true)?;
        tx.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (id, aid))?;
        names.push("Unknown".to_string());
    }
    field("authors", names.join(" & "));

    if let Some((name, sort, link)) = &src.series {
        let sid = intern(tx, "series", "name", name, sort.as_deref(), link, true)?;
        tx.execute("INSERT INTO books_series_link (book, series) VALUES (?1, ?2)", (id, sid))?;
        field("series", name.clone());
    }
    if let Some((name, sort, link)) = &src.publisher {
        let pid = intern(tx, "publishers", "name", name, sort.as_deref(), link, true)?;
        tx.execute("INSERT INTO books_publishers_link (book, publisher) VALUES (?1, ?2)", (id, pid))?;
        field("publisher", name.clone());
    }
    for (name, link) in &src.tags {
        let tid = intern(tx, "tags", "name", name, None, link, false)?;
        tx.execute("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES (?1, ?2)", (id, tid))?;
    }
    if !src.tags.is_empty() {
        field("tags", src.tags.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", "));
    }
    for (order, (code, link)) in src.languages.iter().enumerate() {
        let lid = intern(tx, "languages", "lang_code", code, None, link, false)?;
        tx.execute("INSERT OR IGNORE INTO books_languages_link (book, lang_code, item_order) VALUES (?1, ?2, ?3)", (id, lid, order as i64))?;
    }
    if !src.languages.is_empty() {
        field("languages", src.languages.iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join(", "));
    }
    let mut idents = Vec::new();
    for (kind, val) in &src.identifiers {
        tx.execute("INSERT OR IGNORE INTO identifiers (book, type, val) VALUES (?1, ?2, ?3)", (id, kind, val))?;
        idents.push(format!("{kind}:{val}"));
    }
    if !idents.is_empty() {
        field("identifiers", idents.join(","));
    }
    if let Some(r) = src.rating.filter(|r| *r > 0) {
        tx.execute("INSERT OR IGNORE INTO ratings (rating) VALUES (?1)", [r])?;
        let rid: i64 = tx.query_row("SELECT id FROM ratings WHERE rating = ?1", [r], |row| row.get(0))?;
        tx.execute("INSERT INTO books_ratings_link (book, rating) VALUES (?1, ?2)", (id, rid))?;
        field("rating", r.to_string());
    }
    if let Some(c) = src.comments.as_deref().filter(|c| !c.is_empty()) {
        tx.execute("INSERT INTO comments (book, text) VALUES (?1, ?2)", (id, c))?;
        field("comments", c.to_string());
    }

    // Custom columns, written straight into the destination's tables so that
    // a value containing a separator survives.
    let mut custom_values = 0;
    for (i, value) in &src.custom {
        let (src_def, dest) = &columns[*i];
        match value {
            Custom::Plain(v) => {
                tx.execute(&format!("INSERT OR REPLACE INTO {} (book, value) VALUES (?1, ?2)", dest.table()), rusqlite::params![id, v])?;
                field(&format!("#{}", dest.label), sql_text(v, src_def));
            }
            Custom::Items(items) => {
                let table = dest.table();
                let mut names = Vec::new();
                let mut indexes: Vec<String> = Vec::new();
                for (v, extra) in items {
                    tx.execute(&format!("INSERT OR IGNORE INTO {table} (value) VALUES (?1)"), [v])?;
                    let vid: i64 = tx.query_row(&format!("SELECT id FROM {table} WHERE value = ?1"), [v], |r| r.get(0))?;
                    if dest.datatype == "series" {
                        tx.execute(&format!("INSERT OR IGNORE INTO {} (book, value, extra) VALUES (?1, ?2, ?3)", dest.link_table()), rusqlite::params![id, vid, extra])?;
                    } else {
                        tx.execute(&format!("INSERT OR IGNORE INTO {} (book, value) VALUES (?1, ?2)", dest.link_table()), (id, vid))?;
                    }
                    names.push(sql_text(v, src_def));
                    if dest.datatype == "series" {
                        indexes.extend(extra.map(|e| e.to_string()));
                    }
                }
                field(&format!("#{}", dest.label), names.join(dest.separators().1));
                // After the name: an index logged first has no value to belong to on replay.
                for index in indexes {
                    field(&format!("#{}_index", dest.label), index);
                }
            }
        }
        custom_values += 1;
    }

    // Formats: the real file name and size, hashed from the same read.
    let mut formats = 0;
    for (fi, (format, recorded, stem)) in src.formats.iter().enumerate() {
        let Some((digest, identity)) = hashed.get(&(book_index, Some(fi))) else { continue };
        let format = format.to_uppercase();
        let size = digest.size as i64;
        let _ = recorded;
        tx.execute("INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)", (id, &format, size, stem))?;
        crate::checksums::ChecksumStore::record_hash_on(tx, id, "format", &format, &digest.hash, size, *identity)?;
        ops.push(ChangeOp::FormatSet { book: uuid.clone(), format, name: stem.clone(), size, hash: Some(digest.hash.clone()) });
        formats += 1;
    }

    // The cover is the file beside the book (see the module docs).
    let mut cover = false;
    if let Some((digest, _)) = hashed.get(&(book_index, None)) {
        tx.execute("UPDATE books SET has_cover = 1 WHERE id = ?1", [id])?;
        crate::checksums::ChecksumStore::record_hash_on(tx, id, "cover", "", &digest.hash, digest.size as i64, None)?;
        ops.push(ChangeOp::CoverSet { book: uuid.clone(), blob: Some(digest.hash.clone()) });
        cover = true;
    }

    for row in &src.annotations {
        tx.execute(
            "INSERT OR IGNORE INTO annotations (book, format, user_type, user, timestamp, annot_id, annot_type, annot_data, searchable_text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![id, row[0], row[1], row[2], row[3], row[4], row[5], row[6], row[7]],
        )?;
    }
    for row in &src.positions {
        tx.execute("INSERT OR IGNORE INTO last_read_positions (book, format, user, device, cfi, epoch, pos_frac) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", rusqlite::params![id, row[0], row[1], row[2], row[3], row[4], row[5]])?;
    }

    Ok(Done { id, ops, remap, formats, cover, annotations: src.annotations.len(), custom_values })
}

fn state_path(cache: &Cache) -> PathBuf {
    cache.backend.library_path.join(LIBRARY_HANDLE_DIR_NAME).join("absorbed-libraries.json")
}

fn read_state(cache: &Cache) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(state_path(cache)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn write_state(cache: &Cache, folder: &str, entry: Option<serde_json::Value>) {
    let mut state = read_state(cache);
    match entry {
        Some(e) => {
            state.insert(folder.to_string(), e);
        }
        None => {
            state.remove(folder);
        }
    }
    let path = state_path(cache);
    let temp = path.with_extension("json.partial");
    let written = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&temp, serde_json::Value::Object(state).to_string())).and_then(|_| std::fs::rename(&temp, &path));
    if let Err(e) = written {
        log::warn!("could not record the absorb state of {folder}: {e}");
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Whether the prefixed path of a source book is safe to register.
fn safe_relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains(':') && path.split('/').all(|c| c != ".." && c != ".")
}

/// The checks that need no reading of the database: the folder is inside the
/// library and is a calibre library (not absorbed already, not one of this
/// program's own), and nothing appears to be using it. Returns the folder's
/// path, its path relative to the library, the database file and any
/// warnings. The server runs this before starting a job so a refusal is an
/// immediate answer.
pub fn preflight(cache: &Cache, folder: &str) -> Result<(PathBuf, String, PathBuf, Vec<String>)> {
    let (abs, prefix) = resolve_folder(cache, folder)?;
    let db = abs.join("metadata.db");
    if !db.is_file() {
        if has_parked_database(&abs) {
            bail!("{prefix} has been absorbed already (its database is named {ABSORBED_DB_NAME})");
        }
        bail!("{prefix} does not hold a calibre library (no metadata.db)");
    }
    if abs.join(LIBRARY_HANDLE_DIR_NAME).is_dir() || abs.join(crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME).is_dir() {
        bail!("{prefix} is a library of this program's own, not a calibre library; open it as a library instead");
    }
    let warnings = check_not_in_use(&db)?;
    Ok((abs, prefix, db, warnings))
}

/// Registers every book of the calibre library at `folder` in place. See the
/// module docs. Honours `control` (progress, cancel); `pipeline` sets the
/// batch size and the number of file readers.
pub fn absorb_library(cache: &Cache, folder: &str, options: &AbsorbOptions, pipeline: &PipelineOptions, control: &JobControl) -> Result<AbsorbReport> {
    control.set_phase("checking");
    let (abs, prefix, db, warnings) = preflight(cache, folder)?;
    let mut report = AbsorbReport { folder: prefix.clone(), not_carried: NOT_CARRIED.to_vec(), warnings, ..Default::default() };

    let src_conn = open_source(&db)?;
    if !is_calibre_schema(&src_conn)? {
        bail!("{} is not a calibre library database", db.display());
    }
    let table_exists = |name: &str| -> bool { src_conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1)", [name], |r| r.get(0)).unwrap_or(false) };
    let (has_annotations, has_positions) = (table_exists("annotations"), table_exists("last_read_positions"));
    let library_uuid: Option<String> = src_conn.query_row("SELECT uuid FROM library_id LIMIT 1", [], |r| r.get(0)).optional().ok().flatten();

    // Custom columns: the same label, type and display here.
    control.set_phase("columns");
    let mut columns: Vec<(ColumnDef, ColumnDef)> = Vec::new();
    for src_def in crate::custom_columns::all_columns(&src_conn)? {
        let dest = {
            let conn = cache.backend.conn.lock().unwrap();
            crate::custom_columns::column(&conn, &src_def.label)?
        };
        let dest = match dest {
            Some(d) => d,
            None => {
                let name: String = src_conn.query_row("SELECT name FROM custom_columns WHERE id = ?1", [src_def.num], |r| r.get(0)).unwrap_or_else(|_| src_def.label.clone());
                cache.add_custom_column_with_display(&src_def.label, &name, &src_def.datatype, src_def.is_multiple, &src_def.display).with_context(|| format!("creating the custom column #{}", src_def.label))?;
                report.custom_columns_created.push(src_def.label.clone());
                let conn = cache.backend.conn.lock().unwrap();
                crate::custom_columns::column(&conn, &src_def.label)?.ok_or_else(|| anyhow!("the custom column #{} was not created", src_def.label))?
            }
        };
        if dest.datatype != src_def.datatype || dest.is_multiple != src_def.is_multiple {
            report.custom_column_conflicts.push(format!("#{}: {} here, {} in the nested library", src_def.label, dest.datatype, src_def.datatype));
            continue;
        }
        columns.push((src_def, dest));
    }
    let columns = columns;

    cache.checksums().initialize()?;
    let library = cache.backend.library_path.clone();

    // What is in already, and what is to do.
    let mut existing: HashSet<String> = {
        let (exact, like) = prefix_patterns(&prefix);
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path FROM books WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\'")?;
        let rows = stmt.query_map((&exact, &like), |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let ids: Vec<i64> = src_conn.prepare("SELECT id FROM books ORDER BY id")?.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<_>>()?;
    report.source_books = ids.len();
    control.total.store(ids.len() as u64, Ordering::Relaxed);
    control.set_phase("absorbing");
    write_state(
        cache,
        &prefix,
        Some(serde_json::json!({"state": "in_progress", "source_books": ids.len(), "library_uuid": library_uuid, "updated_at": now_secs(), "uuid_remapped": remap_map(&state_remaps(cache, &prefix), &[])})),
    );

    let reader = SourceReader { conn: &src_conn, has_annotations, has_positions, columns: &columns };
    let mut remaps: Vec<UuidRemap> = Vec::new();
    let batch_size = pipeline.batch_size.max(1);
    let mut pending: Vec<i64> = Vec::with_capacity(ids.len());
    for id in &ids {
        pending.push(*id);
    }

    for chunk in pending.chunks(batch_size) {
        if control.is_cancelled() {
            break;
        }
        // 1. Read the batch from the source. A book that cannot be read, or
        //    is already in, takes no further part.
        let mut books: Vec<SourceBook> = Vec::new();
        for &id in chunk {
            match reader.read(id) {
                Ok(book) => {
                    if !safe_relative(&book.path) {
                        control.errors.fetch_add(1, Ordering::Relaxed);
                        report.failed_count += 1;
                        push_capped(&mut report.failed, AbsorbProblem { source_id: id, title: book.title, error: format!("the folder {:?} is not a usable path", book.path) });
                        continue;
                    }
                    let prefixed = format!("{prefix}/{}", book.path.trim_matches('/'));
                    if existing.contains(&prefixed) {
                        report.already_absorbed += 1;
                        control.already_known.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    books.push(book);
                }
                Err(e) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.failed_count += 1;
                    push_capped(&mut report.failed, AbsorbProblem { source_id: id, title: String::new(), error: format!("{e:#}") });
                }
            }
        }
        if books.is_empty() {
            pipeline.batch_done(control);
            continue;
        }

        // 2. Hash every format file (and the cover) in parallel, streamed.
        let started = Instant::now();
        let mut jobs: Vec<FileJob> = Vec::new();
        for (bi, book) in books.iter().enumerate() {
            let folder = format!("{prefix}/{}", book.path.trim_matches('/'));
            for (fi, (format, _, stem)) in book.formats.iter().enumerate() {
                jobs.push(FileJob { book: bi, format: Some(fi), path: format_file_path(&library, &folder, stem, format) });
            }
            if book.has_cover {
                let cover = library.join(&folder).join("cover.jpg");
                jobs.push(FileJob { book: bi, format: None, path: cover.is_file().then_some(cover) });
            }
        }
        let hashed_slots: Vec<Option<Hashed>> = parallel_map(&jobs, pipeline.workers, control, |job| -> Hashed {
            let path = job.path.as_ref()?;
            let identity = lexmechanic_utils::filenames::identity(path);
            Some(hash_stream(path, Some(control)).map(|d| (d, identity)).map_err(|e| e.to_string()))
        });
        control.add_time("absorb_hash", started.elapsed());
        if control.is_cancelled() {
            break; // nothing of this batch is written
        }
        let mut hashed = HashMap::new();
        for (job, slot) in jobs.iter().zip(hashed_slots) {
            let book = &books[job.book];
            let what = match job.format {
                Some(fi) => format!("{}/{}.{}", book.path, book.formats[fi].2, book.formats[fi].0.to_lowercase()),
                None => format!("{}/cover.jpg", book.path),
            };
            match slot.flatten() {
                Some(Ok(done)) => {
                    control.hashed.fetch_add(1, Ordering::Relaxed);
                    hashed.insert((job.book, job.format), done);
                }
                Some(Err(e)) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.missing_count += 1;
                    push_capped(&mut report.missing_files, format!("{what}: {e}"));
                }
                None => {
                    report.missing_count += 1;
                    push_capped(&mut report.missing_files, what);
                }
            }
        }

        // 3. One transaction for the batch, a savepoint per book.
        let write_started = Instant::now();
        let mut outcomes: Vec<Result<Done>> = Vec::with_capacity(books.len());
        {
            let mut conn = cache.backend.conn.lock().unwrap();
            let mut tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(tx) => tx,
                Err(e) => return Err(e).context("starting a write to the library database"),
            };
            for (bi, book) in books.iter().enumerate() {
                let prefixed = format!("{prefix}/{}", book.path.trim_matches('/'));
                let one = (|| -> Result<Done> {
                    let sp = tx.savepoint()?;
                    let done = register_one(&sp, book, &prefixed, &columns, &hashed, bi)?;
                    sp.commit()?;
                    Ok(done)
                })();
                outcomes.push(one);
            }
            tx.commit().context("committing a batch of absorbed books")?;
        }

        // 4. The log, one entry for the batch; then the report.
        let mut all_ops = Vec::new();
        for (book, outcome) in books.iter().zip(outcomes) {
            match outcome {
                Ok(mut done) => {
                    existing.insert(format!("{prefix}/{}", book.path.trim_matches('/')));
                    all_ops.append(&mut done.ops);
                    report.absorbed += 1;
                    report.formats += done.formats;
                    report.covers += done.cover as usize;
                    report.annotations += done.annotations;
                    report.custom_values += done.custom_values;
                    control.added.fetch_add(1, Ordering::Relaxed);
                    if let Some(r) = done.remap {
                        remaps.push(r.clone());
                        push_capped(&mut report.uuid_remapped, r);
                    }
                    let _ = done.id;
                }
                Err(e) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.failed_count += 1;
                    push_capped(&mut report.failed, AbsorbProblem { source_id: book.id, title: book.title.clone(), error: format!("{e:#}") });
                }
            }
        }
        cache.record_many(all_ops);
        control.add_time("absorb_write", write_started.elapsed());
        write_state(
            cache,
            &prefix,
            Some(serde_json::json!({"state": "in_progress", "source_books": ids.len(), "library_uuid": library_uuid, "updated_at": now_secs(), "uuid_remapped": remap_map(&state_remaps(cache, &prefix), &remaps)})),
        );
        pipeline.batch_done(control);
    }

    report.cancelled = control.is_cancelled();
    drop(reader);
    drop(src_conn);

    // Finish: rename the database so the scan sees the folder as ordinary.
    if report.cancelled {
        report.not_renamed_because = Some("the job was cancelled; run it again to carry on".to_string());
    } else if report.failed_count > 0 && !options.allow_failures {
        report.not_renamed_because = Some(format!("{} book(s) could not be absorbed; the database was left in place so the absorb can be retried", report.failed_count));
    } else if options.keep_database_name {
        report.not_renamed_because = Some("asked to leave the database where it is".to_string());
    } else {
        control.set_phase("finishing");
        match rename_database(&abs, true) {
            Ok(()) => report.renamed = true,
            Err(e) => report.not_renamed_because = Some(format!("the books are in, but metadata.db could not be renamed ({e:#}); close whatever has it open and run the absorb again")),
        }
    }
    let final_state = if report.renamed { "absorbed" } else { "in_progress" };
    write_state(
        cache,
        &prefix,
        Some(serde_json::json!({"state": final_state, "source_books": ids.len(), "library_uuid": library_uuid, "updated_at": now_secs(), "uuid_remapped": remap_map(&state_remaps(cache, &prefix), &remaps)})),
    );
    Ok(report)
}

fn state_remaps(cache: &Cache, prefix: &str) -> serde_json::Map<String, serde_json::Value> {
    read_state(cache).get(prefix).and_then(|e| e.get("uuid_remapped")).and_then(|m| m.as_object().cloned()).unwrap_or_default()
}

fn remap_map(old: &serde_json::Map<String, serde_json::Value>, new: &[UuidRemap]) -> serde_json::Value {
    let mut map = old.clone();
    for r in new {
        map.insert(r.source_uuid.clone(), serde_json::Value::String(r.new_uuid.clone()));
    }
    serde_json::Value::Object(map)
}

/// Renames `metadata.db` (and its companions) to or from [`ABSORBED_DB_NAME`].
fn rename_database(dir: &Path, absorb: bool) -> Result<()> {
    let live = dir.join("metadata.db");
    // Undoing an absorb made before the rename finds the database under its old name.
    let parked = if !absorb && !dir.join(ABSORBED_DB_NAME).exists() && dir.join(LEGACY_ABSORBED_DB_NAME).exists() {
        dir.join(LEGACY_ABSORBED_DB_NAME)
    } else {
        dir.join(ABSORBED_DB_NAME)
    };
    let (from, to) = if absorb { (&live, &parked) } else { (&parked, &live) };
    if to.exists() {
        bail!("{} already exists", to.display());
    }
    std::fs::rename(from, to).with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
    for suffix in COMPANIONS {
        let with = |p: &Path| {
            let mut s = p.as_os_str().to_os_string();
            s.push(suffix);
            PathBuf::from(s)
        };
        let (from, to) = (with(from), with(to));
        if from.exists() {
            if let Err(e) = std::fs::rename(&from, &to) {
                log::warn!("could not rename {}: {e}", from.display());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct UnabsorbReport {
    pub folder: String,
    /// Books removed from this library's database. No file was touched.
    pub removed: usize,
    /// `metadata.db` was put back.
    pub database_restored: bool,
    pub cancelled: bool,
}

/// Undoes [`absorb_library`]: puts the nested `metadata.db` back and removes
/// the books under that folder from this library's database. **Database rows
/// only -- never a file**, and nothing is put on the ignore list (the folder
/// is a library of its own again, which the scan steps around).
pub fn unabsorb_library(cache: &Cache, folder: &str, control: &JobControl) -> Result<UnabsorbReport> {
    let (abs, prefix) = resolve_folder(cache, folder)?;
    let mut report = UnabsorbReport { folder: prefix.clone(), ..Default::default() };
    // The database goes back first: a scan between the two steps then still
    // steps around the folder instead of re-adding its files as loose books.
    if has_parked_database(&abs) && !abs.join("metadata.db").exists() {
        rename_database(&abs, false)?;
        report.database_restored = true;
    }
    let (exact, like) = prefix_patterns(&prefix);
    let ids: Vec<i32> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM books WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' ORDER BY id")?;
        let rows = stmt.query_map((&exact, &like), |r| r.get::<_, i32>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    control.total.store(ids.len() as u64, Ordering::Relaxed);
    control.set_phase("removing");
    cache.checksums().initialize()?;
    for chunk in ids.chunks(100) {
        if control.is_cancelled() {
            report.cancelled = true;
            return Ok(report);
        }
        let mut uuids = Vec::new();
        {
            let mut conn = cache.backend.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for &id in chunk {
                let uuid: Option<String> = tx.query_row("SELECT uuid FROM books WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
                tx.execute("DELETE FROM checksums_db.file_checksums WHERE book_id = ?1", [id])?;
                tx.execute("DELETE FROM checksums_db.enrichment_queue WHERE book_id = ?1", [id])?;
                crate::custom_columns::purge_book(&tx, id)?;
                tx.execute("DELETE FROM books WHERE id = ?1", [id])?;
                uuids.extend(uuid);
            }
            tx.commit()?;
        }
        report.removed += chunk.len();
        control.added.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        cache.record_many(uuids.into_iter().map(|book| ChangeOp::BookRemoved { book }).collect());
    }
    write_state(cache, &prefix, None);
    Ok(report)
}
