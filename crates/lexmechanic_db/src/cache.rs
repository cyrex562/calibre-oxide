//! Port of `old_src/src/calibre/db/cache.py`'s `Cache` (issue #204).
//!
//! # Scope of this pass
//!
//! `cache.py`'s real `Cache` is calibre's single largest class (~3,700
//! lines, 200+ methods): field access, writes, custom columns, notes,
//! FTS, virtual libraries, saved searches, categories, trash, format/
//! cover storage, dump/restore, and more. This pass ports the one
//! piece everything else in the class (and this crate's `search.rs`/
//! `view.rs`/`write.rs`) actually depends on: real field access for
//! the *standard* (non-custom-column) fields, plus real book-id
//! enumeration and real preference access. Real, verified against
//! upstream schema/semantics:
//!
//! - [`Cache::all_book_ids`]: `SELECT id FROM books` -- previously
//!   nonexistent; `search.rs` had a comment reading "let's assume
//!   valid IDs are 1..100" in its place.
//! - [`Cache::field_for`]: resolves every *standard* field
//!   (`id`, `title`, `sort`, `author_sort`, `isbn`, `path`, `uuid`,
//!   `series_index`, `timestamp`, `pubdate`, `last_modified`,
//!   `comments`, `series`, `publisher`, `rating`, `authors`, `tags`,
//!   `languages`, `formats`, `identifiers`, `size`) via the real
//!   schema -- not the narrow 7-column passthrough whitelist
//!   `Backend::field_for` has (which this leaves untouched; it's used
//!   directly by a few other modules for exactly those 7 columns and
//!   isn't part of `cache.py` upstream at all -- `DB`/`Backend` has no
//!   `field_for` in real calibre, that concept only exists on `Cache`).
//! - [`Cache::pref`]/[`Cache::set_pref`]: real, including namespaced
//!   keys, delegating to `Backend`'s JSON preference storage from
//!   #203.
//!
//! # A real, disclosed simplification: string-joined multi-value fields
//!
//! Upstream's `field_for` returns real typed Python values: a tuple of
//! item names (in book-link order) for `is_multiple` fields like
//! `authors`/`tags`/`languages`/`formats`, and a `dict[type, val]` for
//! `identifiers`. This crate's pre-existing `field_for` contract
//! (`Cache::field_for`/`Backend::field_for`, used throughout
//! `backup.rs`/`covers.rs`/`lazy.rs`/etc.) returns `Option<String>` --
//! changing that to a typed enum is a real, separate refactor with a
//! wide blast radius across the crate, not part of this pass. Instead,
//! multi-value fields here return a joined string (`" & "` for
//! authors, matching `author_sort_from_authors`'s separator; `", "`
//! for tags/languages/formats; `"type:val,type:val"` for identifiers).
//! This is a correct, useful value for display/search purposes but is
//! *not* upstream's tuple/dict shape -- callers that need the real
//! per-item structure (e.g. to add/remove one tag) need a typed API
//! this pass doesn't add.
//!
//! [`Cache::set_field`] (issue #223 follow-up) adds a real generic
//! writer for the same standard-field set, backing `legacy.rs`'s
//! setter API -- see its own doc comment for what's faithfully ported
//! vs. simplified.
//!
//! # The #222 cutover: `field_for` now reads from an in-memory model
//!
//! [`Cache::field_for`] originally ran one SQL query per call (the
//! "real, disclosed simplification" section above describes its value
//! shape, which is unchanged). Issue #222 phases 1-2 built a real,
//! upstream-shaped in-memory model to replace that -- `tables.rs`'s
//! [`crate::tables::StandardTables`] (bulk `read()`, matching
//! upstream's `Table` subclasses) and `fields.rs`'s
//! [`crate::fields::FieldStore`] (the typed access layer wrapping it,
//! matching upstream's `Field` classes' role). Phase 3, landed here,
//! cuts `Cache::field_for` over to it: a `Cache` value lazily loads
//! one [`crate::fields::FieldStore`] snapshot on first
//! [`Cache::field_for`] call and reuses it for every subsequent call
//! on that same `Cache` value, instead of hitting SQL every time.
//!
//! **Staying correct across writes** is the real risk in a change
//! like this -- this crate has many write paths (this file's own
//! methods, `restore.rs`, `legacy.rs`'s item rename/delete, and every
//! test file's raw-SQL fixtures throughout the whole crate), and
//! missing even one would silently reintroduce stale reads. Rather
//! than hunting down and instrumenting every call site with an
//! explicit "you wrote something, please invalidate" call (a real,
//! open-ended maintenance burden -- miss one today or in the future
//! and it's a silent bug), [`Cache::field_for`] checks SQLite's own
//! built-in `total_changes()` running counter (a real SQL function,
//! `SELECT total_changes()` -- not a rusqlite wrapper) before trusting
//! its cached snapshot: if that number has moved since the snapshot
//! was loaded, *any* write happened through this connection since
//! then (through this file's methods, raw SQL elsewhere, or a test
//! fixture -- `total_changes()` doesn't care which), so it reloads.
//! [`Cache::invalidate_field_cache`] still exists as an explicit,
//! harmless escape hatch, but nothing in this crate actually needs to
//! call it -- the automatic check already catches everything, which
//! the full existing test suite (unchanged, all passing) is real
//! evidence for: dozens of tests across many files write via raw SQL
//! and immediately assert on `field_for`'s result, and none needed
//! modification for this cutover to stay green.
//!
//! Both `crate::view::View::sort` and every field access in `search.rs`
//! (`fetch_grouped`/`fetch_identifiers`/every matcher) already call
//! `Cache::field_for` per book rather than running their own SQL, so
//! *both* benefit from this cutover automatically -- no separate
//! rearchitecture of either was needed, satisfying issue #222's
//! "Cache/search.rs/view.rs" scope in one change.
//!
//! # Not ported
//!
//! Everything else: notes, FTS, composite fields, virtual libraries,
//! saved searches, categories, trash, dump/restore, `move_library_to`.
//! Each is its own follow-up.
//!
//! Two later, separately-issued follow-ups also live in this file now:
//! real custom-column support (issue #214) and real filesystem book/
//! format/cover/rename/clone management (issue #216) -- both moved
//! here from `library.rs`'s original duplicate implementations once
//! #212 unified `Library`/`Backend` onto the same real schema and
//! connection. See their own doc comments below for what's faithfully
//! ported vs. disclosed simplification in each.

use crate::backend::Backend;
use crate::book::Book;
use crate::fields::FieldStore;
use lexmechanic_ebooks::metadata::MetaInformation;
use lexmechanic_utils::filenames::sanitize_file_name;
use rusqlite::{OptionalExtension, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One of a book's real stored formats -- backs
/// [`Cache::format_file_info`] (issue #524).
pub struct FormatFileInfo {
    pub fmt: String,
    pub size: i64,
    pub path: PathBuf,
}

/// What [`Cache::delete_book_with`] does with the book's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookFiles {
    /// Move the book's own files into the library's `.caltrash`, from where
    /// they can be listed and restored ([`crate::trash`]). Calibre's own
    /// behaviour; and unlike the system wastebasket it works on a network
    /// share, which has none.
    Trash,
    /// Touch no file; record the format files in the ignore list.
    Keep,
}

/// Changes that reached `metadata.db` but not the change log; see
/// [`Cache::change_log_gap`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeLogGap {
    /// How many appends failed.
    pub missed: u64,
    /// When the first one did (unix seconds).
    pub first_at: i64,
    pub last_error: String,
}

/// What a book removal did on disk. Paths are relative to the library
/// root and `/`-separated.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BookDeletion {
    /// Files moved into the library's `.caltrash`.
    pub trashed: Vec<String>,
    /// Format files left on disk and put on the ignore list.
    pub ignored: Vec<String>,
    /// `(path, why)` for files that could not be trashed. They were left
    /// in place, never hard-deleted.
    pub failed: Vec<(String, String)>,
    /// The book's folder, if it was emptied and removed.
    pub folder_removed: Option<String>,
}

pub(crate) struct OwnFile {
    pub(crate) rel: String,
    pub(crate) hash: Option<String>,
    pub(crate) is_format: bool,
}

/// True for a relative path made only of ordinary components: no `..`,
/// no root or drive, no `.`, and nothing inside `.lexmechanic`.
pub(crate) fn path_is_plain(rel: &str) -> bool {
    use std::path::Component;
    let p = Path::new(rel);
    let mut first = true;
    for c in p.components() {
        match c {
            Component::Normal(n) => {
                if first
                    && (n.to_string_lossy().eq_ignore_ascii_case(crate::constants::LIBRARY_HANDLE_DIR_NAME)
                        || n.to_string_lossy().eq_ignore_ascii_case(crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME))
                {
                    return false;
                }
                first = false;
            }
            _ => return false,
        }
    }
    !first
}

#[cfg(test)]
thread_local! {
    pub(crate) static FORCE_TRASH_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What [`Cache::insert_book_in_tx`] created.
pub(crate) struct InsertedBook {
    pub id: i32,
    pub uuid: Option<String>,
    pub timestamp: String,
    pub pubdate: String,
}

/// One file already inside the library, ready to become a book:
/// everything [`Cache::register_books_in_place`] needs, so registering
/// never has to read the file.
#[derive(Debug, Clone)]
pub struct InPlaceFile {
    /// `/`-separated, relative to the library root.
    pub relative_path: String,
    pub metadata: MetaInformation,
    /// Bytes actually read while hashing.
    pub size: i64,
    /// BLAKE3, lowercase hex, from the same single read as `size`.
    pub hash: String,
    pub identity: Option<lexmechanic_utils::filenames::FileIdentity>,
    /// Owes the lazy metadata pass (embedded title/authors/cover).
    pub queue_enrichment: bool,
}

/// One file of a book being imported: already copied into the library's
/// staging area and verified there.
#[derive(Debug, Clone)]
pub struct ImportFile {
    pub staged: PathBuf,
    /// Lowercase extension, no dot.
    pub ext: String,
    pub size: i64,
    /// BLAKE3, lowercase hex, of the verified copy.
    pub hash: String,
}

/// A book for [`Cache::import_books_batch`].
#[derive(Debug, Clone)]
pub struct ImportItem {
    pub metadata: MetaInformation,
    pub files: Vec<ImportFile>,
}

/// A book [`Cache::import_books_batch`] created.
#[derive(Debug, Clone)]
pub struct ImportedBook {
    pub id: i32,
    pub uuid: Option<String>,
    /// `Author/Title (id)`.
    pub folder: String,
    /// Each file's path relative to the library root.
    pub files: Vec<String>,
}

/// calibre's `PATH_LIMIT`: how much of an author or title goes into a
/// folder name (shorter on Windows, where the whole path has 260).
fn path_limit() -> usize {
    if cfg!(windows) {
        40
    } else {
        100
    }
}

fn clip_name(name: &str, limit: usize) -> String {
    let sanitized = sanitize_file_name(name);
    let clipped: String = sanitized.chars().take(limit).collect();
    let clipped = clipped.trim_end_matches(['.', ' ']).trim_start().to_string();
    if clipped.is_empty() {
        "Unknown".to_string()
    } else {
        clipped
    }
}

/// calibre's `construct_path_name`: `("Author/Title (id)", "Title - Author")`.
/// The id stays whole however long the title is.
pub fn import_names(book_id: i32, title: &str, author: &str) -> (String, String) {
    let author_dir = clip_name(author, path_limit());
    let title_dir = clip_name(title, path_limit());
    let folder = format!("{author_dir}/{title_dir}{}", crate::constants::BOOK_ID_PATH_TEMPLATE.replace("{}", &book_id.to_string()));
    let stem = format!("{} - {}", clip_name(title, 35), clip_name(author, 35));
    (folder, stem)
}

/// The key under which a file is looked up in
/// [`Cache::indexed_file_keys`]: exactly what `book_for_file` matches on
/// -- folder, stem, and the upper-cased format.
pub fn file_key(folder: &str, stem: &str, format: &str) -> String {
    format!("{folder}\u{0}{stem}\u{0}{}", format.to_ascii_uppercase())
}

pub struct Cache {
    pub backend: Backend,
    /// Issue #222 phase 3: a lazily-loaded, per-`Cache`-instance
    /// snapshot of every standard field, backing [`Cache::field_for`].
    /// Paired with the connection's `total_changes()` count at load
    /// time; [`Cache::with_field_store`] reloads whenever that count
    /// has moved, rather than relying on every write path in this
    /// crate (there are many, including raw SQL in `restore.rs`/
    /// `legacy.rs`/test fixtures throughout the whole crate) to
    /// remember to call an explicit invalidation method. See this
    /// file's module doc for the full cutover story.
    field_cache: Mutex<Option<crate::field_feed::Loaded>>,
    /// How many times the whole snapshot has been (re)loaded, as opposed to
    /// patched for the books a write touched.
    field_full_loads: std::sync::atomic::AtomicUsize,
}

impl Cache {
    pub fn new<P: AsRef<Path>>(library_path: P) -> Result<Self> {
        let backend = Backend::new(library_path)?;
        Ok(Self::from_backend(backend))
    }

    /// Wraps an already-open [`Backend`] (e.g. [`crate::library::Library`]'s
    /// own, via `Backend`'s cheap `Clone`) in a fresh `Cache` -- no
    /// second connection opened, and a fresh (empty) field cache since
    /// this is a new `Cache` *value*, even though it shares its
    /// underlying connection with others.
    pub(crate) fn from_backend(backend: Backend) -> Self {
        Cache {
            backend,
            field_cache: Mutex::new(None),
            field_full_loads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Forces the next [`Cache::field_for`] call to reload the field
    /// snapshot, even if `total_changes()` hasn't moved (there is no
    /// such case in practice -- every write increments it -- but this
    /// stays available as an explicit, harmless escape hatch).
    pub fn invalidate_field_cache(&self) {
        *self.field_cache.lock().unwrap() = None;
    }

    /// A real [`crate::checksums::ChecksumStore`] over this library's
    /// `.lexmechanic/checksums.db`, sharing this `Cache`'s connection
    /// -- issue #93 §8.
    pub fn checksums(&self) -> crate::checksums::ChecksumStore {
        crate::checksums::ChecksumStore::new(self.backend.conn.clone(), &self.backend.library_path)
    }

    // ---------------------------------------------------------------
    // The change log (#901): every durable metadata write appends
    // before it touches SQL.
    // ---------------------------------------------------------------
    //
    // The log is the library's authority and `metadata.db` is a cache
    // rebuilt from it, so a write that reaches the database without
    // reaching the log is invisible to a rebuild -- it is data loss
    // that only shows up much later, when someone recovers from a
    // corrupt database and finds an edit missing. That is why these go
    // through one helper rather than each write path calling the log
    // itself.

    /// Appends `op` to the change log, unless a replay is feeding us
    /// changes that are already in it.
    ///
    /// A failure to append does not fail the write: refusing to edit
    /// metadata because history could not be recorded would make an
    /// unwritable log directory into an unusable library, and the
    /// database write that has already been committed is still good.
    ///
    /// It is not forgotten either. The failure is written to a persistent
    /// marker (see [`Cache::change_log_gap`]), and a rebuild from the log
    /// refuses while the marker stands, because a rebuild would silently
    /// drop every change the log never received -- and `metadata.db` is then
    /// the only copy of them.
    fn record(&self, op: crate::change_log::ChangeOp) {
        if self.backend.is_replaying() {
            return;
        }
        match self.backend.change_log() {
            Ok(log) => {
                if let Err(e) = log.append(op) {
                    log::warn!("could not record a change in the library log: {e}");
                    self.note_change_log_gap(&format!("{e}"));
                }
            }
            Err(e) => {
                log::warn!("could not open the library change log: {e}");
                self.note_change_log_gap(&format!("{e}"));
            }
        }
    }

    fn change_log_gap_path(&self) -> PathBuf {
        self.backend.library_path.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("change-log-behind.json")
    }

    /// Records, durably, that a change reached `metadata.db` but not the log.
    pub(crate) fn note_change_log_gap(&self, error: &str) {
        let path = self.change_log_gap_path();
        let mut gap = self.change_log_gap().unwrap_or(ChangeLogGap { missed: 0, first_at: crate::trash::now_secs(), last_error: String::new() });
        gap.missed += 1;
        gap.last_error = error.to_string();
        let written = path
            .parent()
            .map(fs::create_dir_all)
            .transpose()
            .and_then(|_| serde_json::to_vec_pretty(&gap).map_err(std::io::Error::other))
            .and_then(|bytes| fs::write(&path, bytes));
        if let Err(e) = written {
            log::error!("the change log is behind metadata.db and that could not be recorded either ({e}); the last error was: {error}");
        }
    }

    /// Whether changes have reached `metadata.db` without reaching the change
    /// log (an append failed), and how many. While this is `Some`, a rebuild
    /// from the log would lose them.
    pub fn change_log_gap(&self) -> Option<ChangeLogGap> {
        serde_json::from_slice(&fs::read(self.change_log_gap_path()).ok()?).ok()
    }

    /// Clears the marker [`Cache::change_log_gap`] reads. Call it once
    /// `metadata.db` has been backed up (or the changes are accepted as
    /// lost to a rebuild); it does not make the log whole.
    pub fn clear_change_log_gap(&self) -> anyhow::Result<()> {
        match fs::remove_file(self.change_log_gap_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// [`Cache::record`] for a run of ops, written as a single
    /// [`crate::change_log::ChangeOp::Batch`] entry: one file and one
    /// fsync however many ops. Same failure policy: logged and swallowed.
    pub(crate) fn record_many(&self, ops: Vec<crate::change_log::ChangeOp>) {
        if ops.is_empty() {
            return;
        }
        self.record(crate::change_log::ChangeOp::Batch { ops });
    }

    /// Records that a book's cover was set, for [`crate::covers::set_cover`],
    /// which writes `has_cover` itself rather than through `set_field`.
    ///
    /// `blob` is the cover's content hash: the log records the hash and not
    /// the image, because a change is a small JSON document and a cover is
    /// a megabyte of JPEG (see `change_log::change`).
    pub(crate) fn record_cover_set(&self, book_id: i32, blob: String) {
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::CoverSet { book, blob: Some(blob) });
    }

    /// Gives a book the folder it will keep its files in (`books.path`),
    /// recorded in the change log. For a book that was just created with no
    /// folder and has no files yet; it moves nothing.
    pub fn place_book_in_folder(&self, book_id: i32, folder: &str) -> anyhow::Result<()> {
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET path = ?1 WHERE id = ?2", (folder, book_id))?;
        }
        let folder = folder.to_string();
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "path".to_string(), value: Some(folder) });
        Ok(())
    }

    /// Convenience for the common case: look up the book's uuid and
    /// record an op built from it. Does nothing if the book has no
    /// uuid, which means it does not exist.
    fn record_for_book(&self, book_id: i32, build: impl FnOnce(String) -> crate::change_log::ChangeOp) {
        match self.book_uuid(book_id) {
            Ok(Some(uuid)) => self.record(build(uuid)),
            Ok(None) => log::warn!("no uuid for book {book_id}; change not recorded"),
            Err(e) => {
                log::warn!("could not read the uuid for book {book_id}: {e}");
                self.note_change_log_gap(&format!("could not read the uuid for book {book_id}: {e}"));
            }
        }
    }

    /// This library's orphan list (#895) -- book entries whose files are
    /// gone.
    pub fn orphans(&self) -> crate::orphans::OrphanStore {
        crate::orphans::OrphanStore::new(self.backend.conn.clone(), &self.backend.library_path)
    }

    /// Files the scanner must not re-add after a removal (#896).
    pub fn ignored(&self) -> crate::removal::IgnoreStore {
        crate::removal::IgnoreStore::new(self.backend.conn.clone(), &self.backend.library_path)
    }

    /// The folder holding a book's files.
    ///
    /// `None` only when the book does not exist. An **empty** `path` is
    /// the library root, which is where a book in a tracked folder
    /// normally lives (#893) -- and four separate call sites used to
    /// treat it as "this book has no folder", because before #893 every
    /// book got an `<author>/<title>/` one. Three of those became silent
    /// no-ops for any root-level book: `remove_format` deleted nothing,
    /// `rename_format_files` renamed nothing, and
    /// `available_format_stem` skipped collision detection entirely.
    ///
    /// One helper rather than four call sites making the same judgement,
    /// since the judgement was wrong in three of them.
    pub fn book_dir(&self, book_id: i32) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(self.field_for(book_id, "path")?.map(|rel| if rel.is_empty() { self.backend.library_path.clone() } else { self.backend.library_path.join(rel) }))
    }

    /// A book's uuid -- the only identifier that means the same thing
    /// on two machines, since `books.id` is a local autoincrement.
    pub fn book_uuid(&self, book_id: i32) -> anyhow::Result<Option<String>> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row("SELECT uuid FROM books WHERE id = ?1", (book_id,), |row| row.get::<_, Option<String>>(0)).ok().flatten())
    }

    /// The reverse: which local book, if any, a uuid refers to.
    pub fn book_id_for_uuid(&self, uuid: &str) -> anyhow::Result<Option<i32>> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row("SELECT id FROM books WHERE uuid = ?1", (uuid,), |row| row.get::<_, i32>(0)).ok())
    }

    /// Creates a bare book row carrying `uuid`, for replay.
    ///
    /// The schema's `books_insert_trg` overwrites `uuid` on every
    /// insert (matching real calibre), so it is set by a follow-up
    /// UPDATE -- the same workaround `add_book_db_entry` already needs,
    /// and for the same reason.
    pub fn insert_book_with_uuid(&self, uuid: &str) -> anyhow::Result<i32> {
        let conn = self.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title, author_sort, path, has_cover, series_index) VALUES ('Unknown', '', '', 0, 1.0)", ())?;
        let id = conn.last_insert_rowid() as i32;
        conn.execute("UPDATE books SET uuid = ?1 WHERE id = ?2", (uuid, id))?;
        Ok(id)
    }

    /// Sets a book's folder, relative to the library root.
    ///
    /// Separate from [`Cache::set_field`], which deliberately refuses
    /// `path`: that guard exists because `path` is where a book's files
    /// live, not a metadata field a user edits, and letting it through
    /// the general field writer would make a typo in a bulk edit move
    /// books. Replay needs to write it anyway, so it gets a named door
    /// rather than a hole in the guard.
    pub fn set_book_path(&self, book_id: i32, rel_path: &str) -> anyhow::Result<()> {
        {
            // Scoped so the log append below happens with the connection
            // lock released: appending wants an fsync, and holding the
            // mutex across it would serialise every other write behind
            // disk I/O.
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET path = ?1 WHERE id = ?2", (rel_path, book_id))?;
        }
        // Replay restores the folder from a `path` field-set, so that is
        // what this emits. Without it a relocation (#894) is applied to
        // the database and never recorded, and the next rebuild sends
        // the book back to a path with no file at it.
        let value = rel_path.to_string();
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "path".to_string(), value: Some(value) });
        Ok(())
    }

    /// Where a book's files now are, written as one unit: `books.path` (when
    /// `path` is given) and `data.name` for each of `names` (format to stem), in
    /// one transaction, and logged as one batch entry. Organize used to write
    /// each format's name and then the path as separate commits, so a failure
    /// part-way left names from the new layout beside a path from the old one.
    ///
    /// Nothing is written if any of it fails.
    pub fn set_book_location(&self, book_id: i32, path: Option<&str>, names: &std::collections::BTreeMap<String, String>) -> anyhow::Result<()> {
        let mut sizes: Vec<(String, String, i64)> = Vec::new();
        {
            let mut conn = self.backend.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(path) = path {
                tx.execute("UPDATE books SET path = ?1 WHERE id = ?2", (path, book_id))?;
            }
            for (format, stem) in names {
                let format = format.to_uppercase();
                let changed = tx.execute("UPDATE data SET name = ?1 WHERE book = ?2 AND format = ?3", (stem, book_id, &format))?;
                if changed == 0 {
                    // No such format row: nothing to rename (as set_format_name).
                    continue;
                }
                let size: i64 = tx.query_row("SELECT uncompressed_size FROM data WHERE book = ?1 AND format = ?2", (book_id, &format), |r| r.get(0)).unwrap_or(0);
                sizes.push((format, stem.clone(), size));
            }
            tx.commit()?;
        }
        let Some(uuid) = self.book_uuid(book_id)? else { return Ok(()) };
        let mut ops = Vec::new();
        if let Some(path) = path {
            ops.push(crate::change_log::ChangeOp::FieldSet { book: uuid.clone(), field: "path".to_string(), value: Some(path.to_string()) });
        }
        for (format, name, size) in sizes {
            let (hash, _) = self.checksums().recorded_identity(book_id, "format", &format).unwrap_or((None, None));
            ops.push(crate::change_log::ChangeOp::FormatSet { book: uuid.clone(), format, name, size, hash });
        }
        self.record_many(ops);
        Ok(())
    }

    /// Writes a `data` row without touching the filesystem.
    ///
    /// Replay's counterpart to [`Cache::add_format`], which copies a
    /// file in as well. Rebuilding an index must not move anybody's
    /// files around.
    pub fn record_format_row(&self, book_id: i32, format: &str, name: &str, size: i64, hash: Option<&str>) -> anyhow::Result<()> {
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                (book_id, format.to_uppercase(), size, name),
            )?;
        }
        if let Some(hash) = hash {
            self.checksums().record_hash(book_id, "format", &format.to_uppercase(), hash, size)?;
        }
        let (format_name, stem_name, recorded_hash) = (format.to_uppercase(), name.to_string(), hash.map(str::to_string));
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatSet { book, format: format_name, name: stem_name, size, hash: recorded_hash });
        Ok(())
    }

    /// Renames the file a format points at, without touching the disk.
    ///
    /// The counterpart to [`Cache::set_book_path`] for the other half of
    /// a file's location: `books.path` is the folder, `data.name` the
    /// filename, and a move can change either. Reads the existing size
    /// and hash forward rather than taking them as arguments, because a
    /// rename does not change the content -- and a `FormatSet` that
    /// dropped the hash would make the next scan re-read the file to
    /// work out what it already knew.
    pub fn set_format_name(&self, book_id: i32, format: &str, name: &str) -> anyhow::Result<()> {
        let format_upper = format.to_uppercase();
        let size: i64 = {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("UPDATE data SET name = ?1 WHERE book = ?2 AND format = ?3", (name, book_id, &format_upper))?;
            conn.query_row("SELECT uncompressed_size FROM data WHERE book = ?1 AND format = ?2", (book_id, &format_upper), |row| row.get(0)).unwrap_or(0)
        };
        let (recorded_hash, _) = self.checksums().recorded_identity(book_id, "format", &format_upper).unwrap_or((None, None));
        let stem_name = name.to_string();
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatSet { book, format: format_upper, name: stem_name, size, hash: recorded_hash });
        Ok(())
    }

    /// Drops a `data` row without touching the filesystem -- replay's
    /// counterpart to [`Cache::remove_format`].
    pub fn forget_format_row(&self, book_id: i32, format: &str) -> anyhow::Result<()> {
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("DELETE FROM data WHERE book = ?1 AND format = ?2", (book_id, format.to_uppercase()))?;
        }
        let format_name = format.to_uppercase();
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatRemoved { book, format: format_name });
        Ok(())
    }

    /// Rebuilds this library's tables from its change log.
    ///
    /// The recovery path the whole design rests on: `metadata.db` is a
    /// cache, so losing it costs a rebuild rather than the library.
    ///
    /// Refuses while [`Cache::change_log_gap`] says the log is missing changes
    /// that only `metadata.db` has: rebuilding would erase them.
    pub fn rebuild_from_change_log(&self) -> anyhow::Result<crate::change_log::ReplayReport> {
        if let Some(gap) = self.change_log_gap() {
            anyhow::bail!(
                "the change log is behind metadata.db: {} change(s) were applied there but could not be recorded in the log (last error: {}). Rebuilding from the log would lose them; back up metadata.db, then call clear_change_log_gap to proceed anyway",
                gap.missed,
                gap.last_error
            );
        }
        let log = self.backend.change_log()?;
        crate::change_log::replay_into(self, &log)
    }

    /// How many times the field snapshot has been loaded in full (rather
    /// than patched for the books a write touched). A bulk loop that keeps
    /// this flat is not doing work proportional to the library per book.
    pub fn field_cache_full_loads(&self) -> usize {
        self.field_full_loads.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Runs `f` over an up-to-date field snapshot.
    ///
    /// Current means: loaded once in full, then patched for just the books
    /// (and item tables) that writes through this connection touched since,
    /// found via SQLite's update hook; a write by another connection or
    /// process is caught by `PRAGMA data_version` and reloads everything.
    /// See `field_feed`.
    fn with_field_store<T>(&self, f: impl FnOnce(&FieldStore) -> T) -> Result<T> {
        let conn = self.backend.conn.lock().unwrap();
        let mut guard = self.field_cache.lock().unwrap();
        let store = crate::field_feed::refresh(&mut guard, &conn, &self.backend.field_feed, &self.field_full_loads)?;
        Ok(f(store))
    }

    pub fn library_id(&self) -> String {
        self.backend.library_id().unwrap_or_default()
    }

    /// A token that changes whenever the library does: this install's change
    /// log sequence (every logged write moves it), plus the number of books
    /// and of format rows, which catches changes the log does not see (a
    /// scan registering files, a rebuild). Equal tokens mean "nothing
    /// written in between", for caches of derived results such as the
    /// duplicate scan.
    pub fn change_watermark(&self) -> String {
        let seq = self.backend.change_log().map(|l| l.next_sequence()).unwrap_or(0);
        let conn = self.backend.conn.lock().unwrap();
        let books: i64 = conn.query_row("SELECT COUNT(*) FROM books", [], |r| r.get(0)).unwrap_or(-1);
        let formats: i64 = conn.query_row("SELECT COUNT(*) FROM data", [], |r| r.get(0)).unwrap_or(-1);
        format!("{seq}.{books}.{formats}")
    }

    /// Port of `Cache.all_book_ids`. Previously nonexistent -- callers
    /// (namely `search.rs`) worked around its absence by assuming book
    /// ids fell in some hardcoded range.
    pub fn all_book_ids(&self) -> Result<Vec<i32>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM books ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Port of `Cache.pref` (without the `default` param collapsing --
    /// callers get `None` for an unset key and decide their own
    /// default, which is equivalent).
    pub fn pref(&self, name: &str, namespace: Option<&str>) -> Option<serde_json::Value> {
        match namespace {
            Some(ns) => self.backend.get_pref(&format!("namespaced:{ns}:{name}")),
            None => self.backend.get_pref(name),
        }
    }

    /// Port of `Cache.set_pref`. The search-cache/dynamic-category
    /// invalidation upstream does for specific preference names
    /// (`grouped_search_terms`, `virtual_libraries`, ...) is not
    /// ported -- this crate doesn't have those caches yet.
    pub fn set_pref(
        &self,
        name: &str,
        val: &serde_json::Value,
        namespace: Option<&str>,
    ) -> Result<()> {
        let key = match namespace {
            Some(ns) => format!("namespaced:{ns}:{name}"),
            None => name.to_string(),
        };
        self.backend.set_pref(&key, val)?;

        // Saved searches, virtual libraries and user categories are all
        // stored through here, and it appended nothing: a rebuild from the
        // log lost every one of them. The value is recorded as the exact
        // JSON text `Backend::set_pref` stored, so replay -- which writes
        // `preferences.val` through `set_preference` -- reproduces the row
        // byte for byte.
        if let Ok(raw) = serde_json::to_string(val) {
            self.record(crate::change_log::ChangeOp::PrefSet { key, value: Some(raw) });
        }
        Ok(())
    }

    /// Port of `Cache.field_for` for the standard (non-custom-column)
    /// fields. See the module docs for the multi-value-field string-
    /// join simplification and what's not covered.
    /// Issue #222 phase 3: every field except `id` now reads from the
    /// in-memory [`FieldStore`] (built from [`crate::tables::StandardTables`],
    /// #222 phase 1/2) instead of running SQL per call -- see this
    /// file's module doc for the cutover and its invalidation story.
    pub fn field_for(&self, book_id: i32, field_name: &str) -> Result<Option<String>> {
        if field_name == "id" {
            // `id` is INTEGER, not TEXT like every other field, and
            // there's no dedicated table to bulk-load for it (a
            // book's id *is* every other table's map key) -- stays a
            // direct, trivial SQL lookup.
            let conn = self.backend.conn.lock().unwrap();
            return conn
                .query_row("SELECT id FROM books WHERE id = ?", [book_id], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .map(|v| v.map(|n| n.to_string()));
        }
        if field_name.starts_with('#') {
            // A custom column (`#label`, or `#label_index` for a series
            // column's index); never cached in the built-in field store.
            return self.get_custom_column_value(book_id, field_name);
        }
        self.with_field_store(|store| store.field_for(book_id, field_name))
    }

    /// Port of `author_sorts`'s/`get_link('authors', ...)`'s/
    /// `author_links`'s own per-author data (issue #514).
    pub fn author_sort_for_name(&self, name: &str) -> Result<Option<String>> {
        self.with_field_store(|store| store.author_sort_for_name(name))
    }

    pub fn author_link_for_name(&self, name: &str) -> Result<Option<String>> {
        self.with_field_store(|store| store.author_link_for_name(name))
    }

    pub fn author_names_for_book(&self, book_id: i32) -> Result<Vec<String>> {
        self.with_field_store(|store| store.author_names_for_book(book_id))
    }

    pub fn all_author_links(&self) -> Result<Vec<(String, String)>> {
        self.with_field_store(|store| store.all_author_links())
    }

    /// One of a book's formats: its uppercase extension, real stored
    /// size, and derived absolute on-disk path (issue #524's
    /// `formats_sizes`/`formats_paths`/`formats_modtimes`/
    /// `formats_path_segments`).
    pub fn format_file_info(&self, book_id: i32) -> Result<Vec<FormatFileInfo>> {
        let path_rel = self.field_for(book_id, "path")?.unwrap_or_default();
        let library_path = self.backend.library_path.clone();
        self.with_field_store(|store| {
            store
                .formats_for_book(book_id)
                .into_iter()
                .map(|(fmt, fname, size)| {
                    let file_name = format!("{fname}.{}", fmt.to_lowercase());
                    let path = library_path.join(&path_rel).join(file_name);
                    FormatFileInfo { fmt, size, path }
                })
                .collect()
        })
    }

    pub fn update_memory(&mut self, _book_id: i32, _field: &str, _value: &str) {
        // Placeholder for future in-memory cache invalidation.
        // Currently, field_for hits the DB directly so no cache to clear.
    }

    /// Real filesystem book/format/cover management (issue #214
    /// follow-up), moved here from `library.rs`'s previously
    /// Cache-side-only duplicate of the same logic -- `Library`'s
    /// equivalents now delegate to these instead of hand-rolling their
    /// own file operations. Folder-per-book layout matches upstream:
    /// `<library>/<sanitized author>/<sanitized title>/`. Fixed two
    /// real gaps while porting (both present in `Library`'s original,
    /// never-faithful-to-upstream version too): `add_format` never
    /// inserted a row into the `data` table, so `data`-driven reads
    /// (`field_for(id, "formats"/"size")`, `Library::format_files`)
    /// never saw a book's real formats; `add_book_db_entry` used to set
    /// `uuid` via the `INSERT` itself, but the real schema's
    /// `books_insert_trg` unconditionally overwrites `uuid`/`sort` on
    /// every insert (matching real calibre) -- fixed the same way #212
    /// fixed it in `library.rs`: an explicit `UPDATE` after insert.
    pub fn add_book(&self, source_path: &Path, metadata: &MetaInformation) -> anyhow::Result<i32> {
        // The library root, not `<author>/<title>/` (#893).
        //
        // This method brings a file in from outside the library; one
        // already inside it is indexed by
        // [`Cache::register_book_in_place`]. And "in" now means the
        // folder itself, so an imported book is indistinguishable from
        // one copied in by hand -- the point of a folder somebody
        // organises themselves. They move it where they want it and the
        // next scan follows.
        //
        // Author/title folders were calibre's way of guaranteeing a
        // unique path per book. With filenames coming from the source and
        // collisions resolved by `free_format_stem`, nothing needs them,
        // and creating them imposes a structure on somebody else's
        // folder.
        // A uuid parsed out of the book file is metadata *about* the
        // book, not this library's identity for the row. `opf.rs` maps
        // any `urn:uuid:` `dc:identifier` onto `MetaInformation::uuid`,
        // and `add_book_db_entry` would write it straight over the uuid
        // `books_insert_trg` generated -- so two files carrying the same
        // identifier (the same book added twice, or two EPUBs from one
        // generator) ended up sharing one `books.uuid`. Since #889 that
        // uuid is the filename of the book's cover sidecar, so the
        // second book's cover overwrote the first's and both then showed
        // the same picture (#950).
        //
        // The value is kept as an identifier so it is not lost. An
        // explicit uuid supplied by a caller still stands: restore and
        // change-log replay legitimately reinstate a uuid this library
        // issued earlier, and they do not come through here.
        let metadata = &{
            let mut owned = metadata.clone();
            if let Some(from_file) = owned.uuid.take() {
                owned.identifiers.entry("uuid".to_string()).or_insert(from_file);
            }
            owned
        };

        let book_id = self.add_book_db_entry(metadata, "")?;

        // Delegate the actual file copy to `add_format` (same naming
        // scheme, same `data` table row) rather than duplicating it --
        // the initial format a book is added with is still just a
        // format, and needs the same `data` row every other one gets
        // for `field_for(id, "formats"/"size")` to see it.
        let ext = source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        self.add_format(book_id, source_path, ext, true)?;

        // Store whatever cover the format carried. Nothing did this
        // before, which is why every book added through the UI or
        // `calibredb add` showed a placeholder tile no matter what was
        // embedded in it -- `get_metadata` has always returned
        // `cover_data` for EPUB, MOBI, FB2, DOCX and (since the PDF
        // rasterizer landed) PDF, and every one of those was thrown
        // away here.
        //
        // A cover that cannot be written is not worth failing an
        // import over: the book itself is already on disk and in the
        // database by this point, and a missing cover is visible and
        // fixable from the UI in a way a half-added book is not.
        let (_, cover) = &metadata.cover_data;
        if !cover.is_empty() {
            if let Err(e) = crate::covers::set_cover(self, book_id, cover) {
                log::warn!("book {book_id} was added but its cover could not be stored: {e}");
            }
        }

        Ok(book_id)
    }

    /// Indexes a file that is **already inside the library**, without
    /// copying or moving it.
    ///
    /// The scanner's entry point (#891): a folder the user filled is
    /// full of books that are already where they belong, and importing
    /// them would mean copying every file next to itself.
    ///
    /// Deliberately a separate method rather than teaching
    /// [`Cache::add_book`] to notice that its source happens to sit
    /// under the library root. Behaviour that changes based on an
    /// implicit path relationship is the kind that surprises somebody
    /// much later, and it would have silently reinterpreted every test
    /// fixture that writes its source file into the library directory
    /// for convenience.
    ///
    /// `relative_path` is `/`-separated and relative to the library
    /// root, exactly as [`crate::scan::DiscoveredFile`] reports it.
    pub fn register_book_in_place(&self, relative_path: &str, metadata: &MetaInformation) -> anyhow::Result<i32> {
        let relative = relative_path.trim_start_matches(['/', '\\']);
        let absolute = self.backend.library_path.join(relative);
        if !absolute.is_file() {
            anyhow::bail!("{} is not a file in this library", absolute.display());
        }
        // Streamed, never buffered: this used to `fs::read` the whole file.
        // No `copy_atomic` either, so nothing is written to the library --
        // indexing a folder must not rewrite it.
        let digest = crate::scan_pipeline::hash_stream(&absolute, None)?;
        let item = InPlaceFile {
            relative_path: relative.to_string(),
            metadata: metadata.clone(),
            size: digest.size as i64,
            hash: digest.hash,
            identity: lexmechanic_utils::filenames::identity(&absolute),
            queue_enrichment: false,
        };
        self.register_books_in_place(&[item]).remove(0)
    }

    /// Registers many already-in-place files in **one** `BEGIN IMMEDIATE`
    /// transaction, and records them in the change log as one batch.
    ///
    /// Each book runs inside its own savepoint, so a book that cannot be
    /// inserted is rolled back on its own and reported in its slot of the
    /// result, without losing the rest of the batch. Nothing is ever left
    /// half-registered: a book's `books` row, authors, `data` row, content
    /// hash and enrichment-queue row are committed together or not at all.
    ///
    /// Order of operations is the one every write here follows: database
    /// commit first, log second ([`Cache::set_field`] says why). A crash in
    /// the gap leaves books the log does not know -- the same window a
    /// single [`Cache::register_book_in_place`] always had, now one batch
    /// wide; see `docs/LIBRARY_MODEL.md`.
    ///
    /// Results are in the order of `items`.
    pub fn register_books_in_place(&self, items: &[InPlaceFile]) -> Vec<anyhow::Result<i32>> {
        if items.is_empty() {
            return Vec::new();
        }
        let fail_all = |e: anyhow::Error| -> Vec<anyhow::Result<i32>> { items.iter().map(|_| Err(anyhow::anyhow!("{e:#}"))).collect() };
        if let Err(e) = self.checksums().initialize() {
            return fail_all(e.into());
        }

        struct Done {
            inserted: InsertedBook,
            ops: Vec<crate::change_log::ChangeOp>,
        }
        let mut outcomes: Vec<anyhow::Result<Done>> = Vec::with_capacity(items.len());
        {
            let mut conn = self.backend.conn.lock().unwrap();
            let mut tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(tx) => tx,
                Err(e) => return fail_all(e.into()),
            };
            for item in items {
                let one = (|| -> anyhow::Result<Done> {
                    let relative = item.relative_path.trim_start_matches(['/', '\\']);
                    let path = Path::new(relative);
                    let stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| anyhow::anyhow!("a book file needs a name"))?.to_string();
                    let ext = path.extension().and_then(|e| e.to_str()).ok_or_else(|| anyhow::anyhow!("a book file needs an extension"))?.to_ascii_lowercase();
                    let folder = path.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                    let format = ext.to_uppercase();

                    let sp = tx.savepoint()?;
                    let inserted = Self::insert_book_in_tx(&sp, &item.metadata, &folder, None)?;
                    sp.execute(
                        "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                        (inserted.id, &format, item.size, &stem),
                    )?;
                    // The identity as well as the hash: a file that stays put is
                    // exactly the file whose rename we will later want to detect
                    // from one `stat` rather than by reading it again.
                    crate::checksums::ChecksumStore::record_hash_on(&sp, inserted.id, "format", &format, &item.hash, item.size, item.identity)?;
                    if item.queue_enrichment {
                        crate::checksums::ChecksumStore::queue_enrichment_on(&sp, inserted.id, &format)?;
                    }
                    sp.commit()?;

                    let mut ops = Self::book_added_ops(&inserted, &item.metadata, &folder);
                    if let Some(uuid) = inserted.uuid.clone() {
                        ops.push(crate::change_log::ChangeOp::FormatSet { book: uuid, format, name: stem, size: item.size, hash: Some(item.hash.clone()) });
                    }
                    Ok(Done { inserted, ops })
                })();
                outcomes.push(one);
            }
            if let Err(e) = tx.commit() {
                return fail_all(e.into());
            }
        }

        let mut all_ops = Vec::new();
        for outcome in outcomes.iter_mut().flatten() {
            all_ops.append(&mut outcome.ops);
        }
        self.record_many(all_ops);

        outcomes
            .into_iter()
            .zip(items)
            .map(|(outcome, item)| {
                let done = outcome?;
                let (_, cover) = &item.metadata.cover_data;
                if !cover.is_empty() {
                    if let Err(e) = crate::covers::set_cover(self, done.inserted.id, cover) {
                        log::warn!("indexed {} but its cover could not be stored: {e}", item.relative_path);
                    }
                }
                Ok(done.inserted.id)
            })
            .collect()
    }

    /// Creates books from files the caller has already copied and verified
    /// into the library's staging area, laying each out the way calibre's
    /// own add does: `<Author>/<Title> (<id>)/<Title> - <Author>.<ext>`.
    ///
    /// One `BEGIN IMMEDIATE` transaction for the whole batch, a savepoint per
    /// book. The id is only known once the row exists and the folder name
    /// contains it, so inside the transaction each book is inserted, its
    /// files are *renamed* from staging to their final names (journaled; a
    /// same-volume rename, never a copy), and its `data` rows and hashes
    /// written. If any step fails, that book's files go back to staging and
    /// its savepoint rolls back; the rest of the batch is unaffected. The
    /// change log gets one batch entry afterwards, as for a scan.
    ///
    /// Results are in the order of `items`.
    pub fn import_books_batch(&self, items: &[ImportItem]) -> Vec<anyhow::Result<ImportedBook>> {
        if items.is_empty() {
            return Vec::new();
        }
        let fail_all = |e: anyhow::Error| -> Vec<anyhow::Result<ImportedBook>> { items.iter().map(|_| Err(anyhow::anyhow!("{e:#}"))).collect() };
        if let Err(e) = self.checksums().initialize() {
            return fail_all(e.into());
        }
        let handle = match self.backend.write_handle() {
            Ok(h) => h,
            Err(e) => return fail_all(e.into()),
        };
        let library = self.backend.library_path.clone();

        struct Done {
            book: ImportedBook,
            ops: Vec<crate::change_log::ChangeOp>,
        }
        let mut outcomes: Vec<anyhow::Result<Done>> = Vec::with_capacity(items.len());
        {
            let mut conn = self.backend.conn.lock().unwrap();
            let mut tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(tx) => tx,
                Err(e) => return fail_all(e.into()),
            };
            for item in items {
                let one = (|| -> anyhow::Result<Done> {
                    let sp = tx.savepoint()?;
                    let mut metadata = item.metadata.clone();
                    // A uuid inside the file is metadata about the book, not
                    // this library's identity for the row (see `add_book`).
                    metadata.uuid = None;
                    let inserted = Self::insert_book_in_tx(&sp, &metadata, "", None)?;
                    let author = metadata.authors.first().map(String::as_str).unwrap_or("Unknown");
                    let (folder, stem) = import_names(inserted.id, &metadata.title, author);
                    sp.execute("UPDATE books SET path = ?1 WHERE id = ?2", (&folder, inserted.id))?;

                    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
                    let result = (|| -> anyhow::Result<Vec<(String, i64, String)>> {
                        let mut rows = Vec::new();
                        for file in &item.files {
                            let dest = library.join(&folder).join(format!("{stem}.{}", file.ext));
                            if dest.exists() {
                                anyhow::bail!("{} already exists", dest.display());
                            }
                            if let Some(parent) = dest.parent() {
                                fs::create_dir_all(parent)?;
                            }
                            // Never replaces: the check above is for the message.
                            handle.rename_atomic_no_replace(&file.staged, &dest)?;
                            moved.push((file.staged.clone(), dest.clone()));
                            let format = file.ext.to_uppercase();
                            sp.execute(
                                "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                                (inserted.id, &format, file.size, &stem),
                            )?;
                            crate::checksums::ChecksumStore::record_hash_on(&sp, inserted.id, "format", &format, &file.hash, file.size, lexmechanic_utils::filenames::identity(&dest))?;
                            rows.push((format, file.size, file.hash.clone()));
                        }
                        Ok(rows)
                    })();
                    let rows = match result.and_then(|rows| sp.commit().map(|_| rows).map_err(Into::into)) {
                        Ok(rows) => rows,
                        Err(e) => {
                            // Back to staging; the savepoint rolls back when
                            // `sp` is dropped uncommitted.
                            for (from, to) in moved.iter().rev() {
                                let _ = handle.rename_atomic(to, from);
                            }
                            let dir = library.join(&folder);
                            let _ = fs::remove_dir(&dir);
                            if let Some(parent) = dir.parent() {
                                let _ = fs::remove_dir(parent);
                            }
                            return Err(e);
                        }
                    };

                    let mut ops = Self::book_added_ops(&inserted, &metadata, &folder);
                    if let Some(uuid) = inserted.uuid.clone() {
                        for (format, size, hash) in &rows {
                            ops.push(crate::change_log::ChangeOp::FormatSet { book: uuid.clone(), format: format.clone(), name: stem.clone(), size: *size, hash: Some(hash.clone()) });
                        }
                    }
                    let files = item.files.iter().map(|f| format!("{folder}/{stem}.{}", f.ext)).collect();
                    Ok(Done { book: ImportedBook { id: inserted.id, uuid: inserted.uuid, folder, files }, ops })
                })();
                outcomes.push(one);
            }
            if let Err(e) = tx.commit() {
                // Nothing was committed, so every file must go back.
                drop(conn);
                for (item, outcome) in items.iter().zip(&outcomes) {
                    if let Ok(done) = outcome {
                        for (file, rel) in item.files.iter().zip(&done.book.files) {
                            let _ = handle.rename_atomic(&library.join(rel), &file.staged);
                        }
                    }
                }
                return fail_all(e.into());
            }
        }

        let mut all_ops = Vec::new();
        for outcome in outcomes.iter_mut().flatten() {
            all_ops.append(&mut outcome.ops);
        }
        self.record_many(all_ops);

        outcomes
            .into_iter()
            .zip(items)
            .map(|(outcome, item)| {
                let done = outcome?;
                let (_, cover) = &item.metadata.cover_data;
                if !cover.is_empty() {
                    if let Err(e) = crate::covers::set_cover(self, done.book.id, cover) {
                        log::warn!("imported book {} but its cover could not be stored: {e}", done.book.id);
                    }
                }
                self.apply_descriptive_metadata(done.book.id, &item.metadata);
                Ok(done.book)
            })
            .collect()
    }

    /// Writes the descriptive fields [`Cache::insert_book_in_tx`] does not
    /// (publisher, series, tags, identifiers, languages, comments, rating),
    /// through [`Cache::set_field`] so each one is logged like any edit.
    ///
    /// Best effort per field: the book and its files are already committed,
    /// and a field that cannot be written is worth a warning, not losing the
    /// book. Placeholder values (language `und`) are not written, and a tag
    /// or identifier containing the field's own separator is skipped rather
    /// than split into pieces.
    pub(crate) fn apply_descriptive_metadata(&self, book_id: i32, m: &MetaInformation) {
        let mut set = |field: &str, value: String| {
            if let Err(e) = self.set_field(book_id, field, &value) {
                log::warn!("book {book_id}: could not set {field}: {e:#}");
            }
        };
        if let Some(p) = m.publisher.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            set("publisher", p.to_string());
        }
        if let Some(s) = m.series.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            set("series", s.to_string());
        }
        let mut tags: Vec<&str> = Vec::new();
        for t in m.tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty() && !t.contains(',')) {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        if !tags.is_empty() {
            set("tags", tags.join(", "));
        }
        let langs: Vec<&str> = m.languages.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && *l != "und" && !l.contains(',')).collect();
        if !langs.is_empty() {
            set("languages", langs.join(", "));
        }
        let mut ids: Vec<String> = m.identifiers.iter().filter(|(k, v)| !k.is_empty() && !v.trim().is_empty() && !k.contains([',', ':']) && !v.contains(',')).map(|(k, v)| format!("{k}:{}", v.trim())).collect();
        ids.sort();
        if !ids.is_empty() {
            set("identifiers", ids.join(","));
        }
        if let Some(c) = m.comments.as_deref().filter(|c| !c.trim().is_empty()) {
            set("comments", c.to_string());
        }
        // calibre stores a rating as 0-10 (half stars).
        if let Some(r) = m.rating.filter(|r| *r > 0.0) {
            set("rating", (r.round() as i32).clamp(1, 10).to_string());
        }
    }

    /// Every `(folder, file stem, FORMAT)` the library already has a row
    /// for, as one string key per file -- see [`file_key`]. One query, so
    /// a scan can tell new files from known ones without asking the
    /// database about each file in turn.
    pub fn indexed_file_keys(&self) -> anyhow::Result<std::collections::HashSet<String>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT b.path, d.name, d.format FROM books b JOIN data d ON d.book = b.id")?;
        let rows = stmt.query_map([], |row| Ok(file_key(&row.get::<_, String>(0)?, &row.get::<_, String>(1)?, &row.get::<_, String>(2)?)))?;
        Ok(rows.collect::<Result<_>>()?)
    }

    /// Whether some book already claims this file.
    ///
    /// Compared on `(books.path, data.name, format)`, which is exactly
    /// what locates a file on disk -- so a second scan of the same
    /// folder recognises what it indexed the first time instead of
    /// adding every book again.
    pub fn book_for_file(&self, relative_path: &str) -> anyhow::Result<Option<i32>> {
        let path = Path::new(relative_path.trim_start_matches(['/', '\\']));
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { return Ok(None) };
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return Ok(None) };
        let folder = path.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();

        let conn = self.backend.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT b.id FROM books b JOIN data d ON d.book = b.id
                 WHERE b.path = ?1 AND d.name = ?2 AND d.format = ?3",
                (&folder, stem, ext.to_uppercase()),
                |row| row.get::<_, i32>(0),
            )
            .ok())
    }

    /// Adds a book entry to the database without copying files (used
    /// by `restore.rs`'s OPF-driven restore). See this section's docs
    /// for the `uuid`-preservation fix.
    pub fn add_book_db_entry(
        &self,
        metadata: &MetaInformation,
        rel_path: &str,
    ) -> anyhow::Result<i32> {
        self.add_book_db_entry_with_id(metadata, rel_path, None)
    }

    /// Same as [`Cache::add_book_db_entry`] but supports inserting
    /// with an explicit `id` -- used by `restore.rs` (#224) to
    /// preserve each book's original id (recovered from its OPF's
    /// embedded `calibre` identifier) across a restore, instead of
    /// every restored book silently getting a fresh autoincrement id.
    /// `explicit_id` is used only if no row with that id already
    /// exists; otherwise this falls back to autoincrement, same as
    /// `add_book_db_entry`.
    pub fn add_book_db_entry_with_id(
        &self,
        metadata: &MetaInformation,
        rel_path: &str,
        explicit_id: Option<i32>,
    ) -> anyhow::Result<i32> {
        let inserted = {
            let mut conn = self.backend.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let inserted = Self::insert_book_in_tx(&tx, metadata, rel_path, explicit_id)?;
            tx.commit()?;
            inserted
        };
        // One entry per op, as it always was: a single add is not worth
        // changing what its log looks like (see `register_books_in_place`
        // for the batched form).
        for op in Self::book_added_ops(&inserted, metadata, rel_path) {
            self.record(op);
        }
        Ok(inserted.id)
    }

    /// The rows of a new book -- `books`, its authors and the links --
    /// written on a transaction the caller owns, so one transaction can
    /// carry a whole batch of books (see [`Cache::register_books_in_place`]).
    ///
    /// Writes only the database. The caller commits, and then records
    /// [`Cache::book_added_ops`] in the change log -- in that order, for the
    /// reason [`Cache::set_field`] gives.
    pub(crate) fn insert_book_in_tx(
        tx: &rusqlite::Connection,
        metadata: &MetaInformation,
        rel_path: &str,
        explicit_id: Option<i32>,
    ) -> anyhow::Result<InsertedBook> {
        let author_name = metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("Unknown");

        let taken_id = match explicit_id {
            Some(id) => {
                let count: i64 =
                    tx.query_row("SELECT COUNT(*) FROM books WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })?;
                if count > 0 {
                    None
                } else {
                    Some(id)
                }
            }
            None => None,
        };

        // Taken once, so the value written to the row and the value
        // recorded in the log are the same string. Each `INSERT` used to
        // call `Utc::now()` for itself, and neither was recorded at all:
        // a rebuild gave every book the schema's default timestamps, so a
        // real publication date from the book's own metadata was lost.
        let timestamp = metadata.timestamp.unwrap_or_else(chrono::Utc::now).to_rfc3339();
        let pubdate = metadata.pubdate.unwrap_or_else(chrono::Utc::now).to_rfc3339();

        let book_id = match taken_id {
            Some(id) => {
                tx.execute(
                    "INSERT INTO books (id, title, author_sort, path, has_cover, timestamp, pubdate, series_index)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
                    (
                        id,
                        &metadata.title,
                        author_name,
                        rel_path,
                        &timestamp,
                        &pubdate,
                        metadata.series_index,
                    ),
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO books (title, author_sort, path, has_cover, timestamp, pubdate, series_index)
                     VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
                    (
                        &metadata.title,
                        author_name,
                        rel_path,
                        &timestamp,
                        &pubdate,
                        metadata.series_index,
                    ),
                )?;
                tx.last_insert_rowid() as i32
            }
        };

        // `books_insert_trg` unconditionally overwrites `sort` via
        // `title_sort()` on every INSERT -- restore a saved
        // `title_sort` from the OPF over that computed default, same
        // as `uuid` below.
        if let Some(sort) = metadata.title_sort.as_deref() {
            tx.execute("UPDATE books SET sort = ?1 WHERE id = ?2", (sort, book_id))?;
        }

        if let Some(uuid) = metadata.uuid.as_deref() {
            tx.execute("UPDATE books SET uuid = ?1 WHERE id = ?2", (uuid, book_id))?;
        }

        // Link every author, not just the first -- `author_name`
        // above is only used for `author_sort`, which upstream also
        // derives from just the primary author.
        // Deduplicated first: `books_authors_link` is UNIQUE(book, author)
        // and `authors.name` is NOCASE, so "A", "a" and " A " are one
        // author, and linking them twice used to abort the insert.
        let mut link_names: Vec<&str> = dedupe_names(metadata.authors.iter().map(|s| s.as_str()));
        if link_names.is_empty() {
            link_names.push("Unknown");
        }
        for name in link_names {
            let author_id: i32 = {
                let mut stmt = tx.prepare("SELECT id FROM authors WHERE name = ?1")?;
                let mut rows = stmt.query([name])?;
                if let Some(row) = rows.next()? {
                    row.get(0)?
                } else {
                    tx.execute("INSERT INTO authors (name) VALUES (?1)", [name])?;
                    tx.last_insert_rowid() as i32
                }
            };
            tx.execute(
                "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
                (book_id, author_id),
            )?;
        }

        let uuid: Option<String> = tx.query_row("SELECT uuid FROM books WHERE id = ?1", (book_id,), |row| row.get::<_, Option<String>>(0)).ok().flatten();
        Ok(InsertedBook { id: book_id, uuid, timestamp, pubdate })
    }

    /// The change-log entries for a book [`Cache::insert_book_in_tx`] just
    /// created.
    ///
    /// Recorded as an add plus a field write per column, rather than one
    /// fat op carrying the whole row. It keeps replay to a single
    /// `set_field` path, and it means a later edit to any one of these
    /// fields supersedes just that one during compaction.
    pub(crate) fn book_added_ops(inserted: &InsertedBook, metadata: &MetaInformation, rel_path: &str) -> Vec<crate::change_log::ChangeOp> {
        use crate::change_log::ChangeOp;
        let Some(uuid) = inserted.uuid.clone() else { return Vec::new() };
        let author_name = metadata.authors.first().map(|s| s.as_str()).unwrap_or("Unknown");
        let mut fields: Vec<(&str, String)> = vec![
            ("title", metadata.title.clone()),
            ("author_sort", author_name.to_string()),
            ("path", rel_path.to_string()),
            ("series_index", metadata.series_index.to_string()),
            ("timestamp", inserted.timestamp.clone()),
            ("pubdate", inserted.pubdate.clone()),
        ];
        // An explicit `title_sort` is written over the trigger's
        // computed default, so it has to be logged or a rebuild
        // silently goes back to the computed one.
        if let Some(sort) = metadata.title_sort.as_deref() {
            fields.push(("sort", sort.to_string()));
        }
        let mut ops = vec![ChangeOp::BookAdded { book: uuid.clone() }];
        for (field, value) in fields {
            ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: field.to_string(), value: Some(value) });
        }
        if !metadata.authors.is_empty() {
            ops.push(ChangeOp::FieldSet { book: uuid, field: "authors".to_string(), value: Some(metadata.authors.join(" & ")) });
        }
        ops
    }

    /// Copies `source_path` into the book's folder as `<title>.<fmt>`
    /// and records it in the `data` table (`UNIQUE(book, format)`, so
    /// re-adding the same format with `replace=true` overwrites the
    /// row). Returns `Ok(false)` without copying if the destination
    /// already exists and `replace` is false.
    pub fn add_format(
        &self,
        book_id: i32,
        source_path: &Path,
        format: &str,
        replace: bool,
    ) -> anyhow::Result<bool> {
        let path_rel = self
            .field_for(book_id, "path")?
            .ok_or_else(|| anyhow::anyhow!("Book {book_id} not found"))?;
        // An empty `path` is the library root, and that is now an
        // ordinary place for a book to live (#889/#893). It used to mean
        // "this book has not been placed in a folder yet", because every
        // book got an `<author>/<title>/` one -- so what was a useful
        // guard became a refusal to add a second format to any book in a
        // tracked folder.
        let title = self.field_for(book_id, "title")?.unwrap_or_default();

        let book_dir = self.backend.library_path.join(&path_rel);

        // `format` is sanitized the same as `title` -- a caller that
        // derives it from untrusted input (e.g. an HTTP request body,
        // see `lexmechanic_srv::cdb::set_fields`) must not be able to
        // embed a path separator here and write outside `book_dir`.
        let ext = sanitize_file_name(&format.to_lowercase());
        // The file keeps the name it arrived with (#893). It used to be
        // renamed to the book's title, which made the two inseparable:
        // the only way to change a filename was to retitle the book,
        // and retitling always renamed the file. In a folder the user
        // manages by hand, neither is wanted.
        //
        // The title is still the fallback, for a source with no usable
        // stem at all -- there has to be *some* name.
        let from_source = source_path.file_stem().and_then(|s| s.to_str()).map(sanitize_file_name).filter(|s| !s.trim().is_empty());
        let preferred = from_source.unwrap_or_else(|| sanitize_file_name(&title));

        // `replace` is asked about the book's *format*, not about a
        // filename. Checking the destination path instead was correct
        // only while filenames were derived from the title, so a book's
        // EPUB always had one possible name. Now that the name comes
        // from the source, a second EPUB arriving under a different name
        // would find its destination free, slip past a `replace = false`
        // caller, and overwrite the `data` row -- leaving the first file
        // on disk with nothing pointing at it.
        let existing_name: Option<String> = {
            let conn = self.backend.conn.lock().unwrap();
            conn.query_row("SELECT name FROM data WHERE book = ?1 AND format = ?2", (book_id, format.to_uppercase()), |row| row.get(0)).ok()
        };
        if existing_name.is_some() && !replace {
            return Ok(false);
        }

        let stem = self.free_format_stem(book_id, &format, &book_dir, &preferred, &ext, source_path)?;
        let file_name = format!("{stem}.{ext}");
        let dest_path = book_dir.join(&file_name);
        // The source may already *be* the destination, now that a book's
        // folder can be the library root: adding `<library>/dune.pdf`
        // computes a destination of exactly that path. Copying a file
        // onto itself is not a no-op -- the write truncates the source
        // before reading it -- so the copy is skipped and only the `data`
        // row written. Compared by filesystem identity rather than path
        // text, which sees through a symlink or a differently-spelled but
        // equal path.
        let already_in_place = lexmechanic_utils::filenames::samefile(source_path, &dest_path);

        // A file already at the destination refuses `replace = false` --
        // except when it is the source itself. Then nothing is overwritten
        // (the format row was checked above): the file is just being
        // claimed. This used to refuse that too, which is why `merge`
        // passes `replace = true` for the in-place sibling case.
        if dest_path.exists() && !replace && !already_in_place {
            return Ok(false);
        }

        // Port of issue #93's crate-wide write-path retrofit: real,
        // journaled, crash-safe, large-file-safe copy-in through
        // `LibraryHandle` instead of a raw `fs::copy` (creates
        // `book_dir` itself, no separate `create_dir_all` needed).
        // `copy_atomic` streams both its hashing and copying passes,
        // so this never buffers a whole book file in memory even for
        // a large audiobook/PDF.
        let hash = if already_in_place {
            blake3::hash(&fs::read(&dest_path)?).to_hex().to_string()
        } else {
            self.backend.write_handle()?.copy_atomic(source_path, &dest_path)?
        };
        let size = fs::metadata(&dest_path)?.len() as i64;

        // Port of docs/FAULT_TOLERANCE.md §8: "every book file's
        // BLAKE3 is stored... at add time" -- see checksums.rs's
        // module doc for why this lives in its own sidecar db rather
        // than a new metadata.db column. Uses the hash `copy_atomic`
        // already computed while streaming, rather than
        // `record_file`'s whole-file re-read.
        self.checksums()
            .record_hash(book_id, "format", &format.to_uppercase(), &hash, size)?;

        let conn = self.backend.conn.lock().unwrap();
        conn.execute(
            "UPDATE books SET timestamp = datetime('now') WHERE id = ?1",
            (book_id,),
        )?;
        // Read back rather than recomputed, so the log carries the exact
        // string the database now holds. `datetime('now')` is evaluated
        // by SQLite, and a second call from Rust would be a different
        // instant in a different format.
        let stamped: Option<String> = conn.query_row("SELECT timestamp FROM books WHERE id = ?1", (book_id,), |row| row.get(0)).ok();
        conn.execute(
            "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
            (book_id, format.to_uppercase(), size, &stem),
        )?;
        drop(conn);

        // Adding a format stamps the book's `timestamp`, and nothing
        // recorded that. It surfaced as soon as the change-log audit
        // compared a rebuilt library to the original: every book's
        // timestamp came back as whatever the add recorded *before* this
        // overwrote it. Later in the log than `add_book`'s own timestamp,
        // so it supersedes it on replay exactly as it did on the live
        // database.
        if let Some(stamped) = stamped {
            self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "timestamp".to_string(), value: Some(stamped) });
        }

        // A replacement that landed under a different name leaves the
        // file it replaced behind with no `data` row pointing at it --
        // an invisible file taking up space, and one that
        // `check_library` would later report as a stray.
        if let Some(old_name) = existing_name.filter(|old| *old != stem) {
            let orphan = book_dir.join(format!("{old_name}.{ext}"));
            if orphan.is_file() {
                match self.backend.write_handle() {
                    Ok(handle) => {
                        if let Err(e) = handle.remove_atomic(&orphan) {
                            log::warn!("replaced {format} for book {book_id} but could not remove {}: {e}", orphan.display());
                        }
                    }
                    Err(e) => log::warn!("replaced {format} for book {book_id} but could not remove {}: {e}", orphan.display()),
                }
            }
        }

        let (format_name, stem_name) = (format.to_uppercase(), stem.clone());
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatSet { book, format: format_name, name: stem_name, size, hash: Some(hash) });
        Ok(true)
    }

    /// A filename stem inside `book_dir` that this book may write its
    /// `format` file to without destroying somebody else's.
    ///
    /// `preferred` (the sanitized title) is used as-is in the ordinary
    /// case. It is not usable when a *different* book already owns a
    /// file of that name, which happens more easily than it sounds:
    /// a book's folder is `<author>/<title>`, so two books with the
    /// same author and title share one -- and both would then want
    /// `<title>.<ext>`.
    ///
    /// Before this, `add_format` took the name unconditionally and
    /// copied over whatever was there, because `add_book` passes
    /// `replace = true` for a book's first format. Adding two books
    /// with the same title therefore left one file on disk holding the
    /// second book's contents, with *both* `data` rows naming it: the
    /// first book silently became a copy of the second. Verified
    /// before fixing, and covered by
    /// `adding_a_second_book_with_the_same_title_does_not_overwrite_the_first`.
    ///
    /// A file this book already owns for this same format is not a
    /// collision -- that is a re-add, and `replace` decides it.
    fn free_format_stem(&self, book_id: i32, format: &str, book_dir: &Path, preferred: &str, ext: &str, source_path: &Path) -> anyhow::Result<String> {
        let own_name: Option<String> = {
            let conn = self.backend.conn.lock().unwrap();
            conn.query_row("SELECT name FROM data WHERE book = ?1 AND format = ?2", (book_id, format.to_uppercase()), |row| row.get(0)).ok()
        };

        let mut candidate = preferred.to_string();
        // Bounded so a directory somebody has filled with `Title
        // (1..n).pdf` cannot spin here forever.
        for suffix in 1..1000 {
            let occupant = book_dir.join(format!("{candidate}.{ext}"));
            // The source file is not a collision with itself. Now that a
            // book's folder can be the library root (#893), adding
            // `<library>/dune.pdf` computes a destination of exactly that
            // path -- so without this the file being added looks like an
            // existing occupant and every add walks its own name to
            // `dune (1).pdf`.
            let is_the_source = lexmechanic_utils::filenames::samefile(source_path, &occupant);
            if own_name.as_deref() == Some(candidate.as_str()) || is_the_source || !occupant.exists() {
                return Ok(candidate);
            }
            candidate = format!("{preferred} ({suffix})");
        }
        anyhow::bail!("could not find an unused filename for {preferred}.{ext} in {}", book_dir.display())
    }

    /// Removes this book's `fmt` file from its folder, and the `data`
    /// row naming it.
    ///
    /// The file is found through `data.name` rather than by scanning
    /// the folder for the first thing with a matching extension, which
    /// is what this used to do. Two books with the same author and
    /// title share a folder, so "first file with this extension" can
    /// easily be the *other* book's -- removing one format would then
    /// delete a different book entirely. `data.name` says which file
    /// is ours; nothing else does.
    ///
    /// The file goes to the library's trash (`.caltrash/f/<book id>`) and can
    /// be restored, as calibre does; [`Cache::remove_format_permanently`] is
    /// for a file that should not be kept.
    pub fn remove_format(&self, book_id: i32, fmt: &str) -> anyhow::Result<()> {
        self.remove_format_inner(book_id, fmt, false)
    }

    /// [`Cache::remove_format`] without the trash: the file is deleted.
    pub fn remove_format_permanently(&self, book_id: i32, fmt: &str) -> anyhow::Result<()> {
        self.remove_format_inner(book_id, fmt, true)
    }

    fn remove_format_inner(&self, book_id: i32, fmt: &str, permanently: bool) -> anyhow::Result<()> {
        let Some(book_dir) = self.book_dir(book_id)? else {
            return Ok(());
        };

        let name: Option<String> = {
            let conn = self.backend.conn.lock().unwrap();
            conn.query_row("SELECT name FROM data WHERE book = ?1 AND format = ?2", (book_id, fmt.to_uppercase()), |row| row.get(0)).ok()
        };

        if let Some(name) = name {
            let path = book_dir.join(format!("{name}.{}", fmt.to_lowercase()));
            if path.is_file() {
                if permanently {
                    // Port of issue #93's crate-wide write-path retrofit:
                    // real, journaled, crash-safe removal through
                    // `LibraryHandle` instead of a raw `fs::remove_file`.
                    self.backend.write_handle()?.remove_atomic(&path)?;
                } else {
                    // calibre's `remove_formats`: into `.caltrash/f/<id>`, so
                    // the format can be restored. Failing here leaves the row
                    // and the file exactly as they were.
                    let hash = self.checksums().recorded_identity(book_id, "format", &fmt.to_uppercase()).ok().and_then(|(h, _)| h);
                    self.move_format_to_trash(book_id, fmt, &name, &path, hash)?;
                }
            }
        }

        // The file is gone -- drop its stale checksum record too, so
        // the sidecar store doesn't keep asserting a hash for a format
        // that no longer exists.
        self.checksums()
            .remove(book_id, "format", &fmt.to_uppercase())?;

        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM data WHERE book = ?1 AND format = ?2",
                (book_id, fmt.to_uppercase()),
            )?;
        }

        let format = fmt.to_uppercase();
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatRemoved { book, format });
        Ok(())
    }

    /// Deletes a book's row, and **only the files that book owns**,
    /// sending them to the system wastebasket.
    ///
    /// This is the safe default for a caller that has no opinion about
    /// the files; see [`Cache::delete_book_with`] for the rules and for
    /// the keep-the-files variant. It no longer removes the book's
    /// folder: `books.path` is shared by every book a scan found in one
    /// folder (`register_book_in_place`), so removing "the folder" used
    /// to take the siblings and any non-book files with it.
    ///
    /// Files that could not be sent to the wastebasket are left where
    /// they are and logged; use [`Cache::delete_book_with`] to get them
    /// back as data.
    pub fn delete_book(&self, book_id: i32) -> anyhow::Result<()> {
        let outcome = self.delete_book_with(book_id, BookFiles::Trash)?;
        for (path, why) in &outcome.failed {
            log::warn!("book {book_id} was removed but {path} could not be sent to the wastebasket and was left in place: {why}");
        }
        Ok(())
    }

    /// Removes a book from the library, leaving every file where it is.
    /// The format files are recorded in the ignore list so a rescan
    /// does not add the book straight back. Used for "remove from
    /// library, keep the file", and by change-log replay (which must
    /// never touch a user's files on the strength of a log entry).
    pub fn delete_book_keep_files(&self, book_id: i32) -> anyhow::Result<BookDeletion> {
        self.delete_book_with(book_id, BookFiles::Keep)
    }

    /// The one real implementation of book removal.
    ///
    /// Everything that is read from the database (the folder, the format
    /// file names, whether the folder is shared, the checksums) is read
    /// **before** the row is deleted, since the `books_delete_trg`
    /// trigger cascades the rest away.
    ///
    /// With [`BookFiles::Trash`] only the book's own files go: each
    /// `data` row's `<name>.<format>` in the book's folder, and -- only
    /// when no other book row has the same `path`, checked in the
    /// database rather than inferred from naming -- the folder's
    /// `cover.jpg` and `metadata.opf`. Never a recursive directory
    /// delete. The folder is removed afterwards only if it is empty,
    /// is not the library root and is not inside `.lexmechanic`.
    ///
    /// A file that cannot be trashed is **not** hard-deleted. It stays,
    /// is reported in [`BookDeletion::failed`], and is put on the ignore
    /// list so the next scan does not resurrect the book.
    pub fn delete_book_with(&self, book_id: i32, files: BookFiles) -> anyhow::Result<BookDeletion> {
        let mut outcome = BookDeletion::default();
        let Some(path_rel) = self.field_for(book_id, "path")? else {
            // No such book: nothing on disk can be attributed to it.
            self.delete_book_rows(book_id)?;
            return Ok(outcome);
        };
        let title = self.field_for(book_id, "title")?.unwrap_or_default();
        // Read before the row is deleted: afterwards there is no uuid
        // to record the removal against, and an unrecorded removal
        // comes back on the next rebuild.
        let uuid = self.book_uuid(book_id)?;
        let dedicated = self.folder_is_dedicated(book_id, &path_rel)?;
        let own = self.own_files(book_id, &path_rel, dedicated)?;
        let cover_sidecar = crate::covers::sidecar_cover_path(self, book_id).ok();

        if files == BookFiles::Keep {
            // Recorded first, and with `?`: a book removed without its
            // ignore entry is re-added by the next scan, so failing here
            // (row untouched) beats succeeding without it. Only the format
            // files are what a scan would re-add.
            let library = &self.backend.library_path;
            for file in own.iter().filter(|f| f.is_format && library.join(&f.rel).is_file()) {
                self.ignored().ignore(&file.rel, file.hash.as_deref(), &title)?;
                outcome.ignored.push(file.rel.clone());
            }
        }

        // Everything the trash needs to put the book back, read while the
        // row still exists.
        let manifest = if files == BookFiles::Trash { Some(self.trash_manifest_for(book_id, uuid.clone(), &title, &path_rel, &own)?) } else { None };

        self.delete_book_rows(book_id)?;
        if let Some(uuid) = uuid.clone() {
            self.record(crate::change_log::ChangeOp::BookRemoved { book: uuid });
        }

        let library = self.backend.library_path.clone();
        match files {
            // Ignore entries were recorded before the row went.
            BookFiles::Keep => {}
            BookFiles::Trash => {
                // Into the library's own `.caltrash`, as calibre does -- see
                // `crate::trash`. A same-volume rename, so it works on a
                // network share that has no recycle bin.
                let moved = self.move_book_to_trash(manifest.expect("built above for Trash"), cover_sidecar.filter(|c| c.is_file()));
                outcome.trashed = moved.moved;
                for (rel, why) in moved.failed {
                    if own.iter().any(|f| f.rel == rel && f.is_format) {
                        let hash = own.iter().find(|f| f.rel == rel).and_then(|f| f.hash.clone());
                        match self.ignored().ignore(&rel, hash.as_deref(), &title) {
                            Ok(()) => outcome.ignored.push(rel.clone()),
                            Err(e) => log::warn!("could not record {rel} in the ignore list: {e}"),
                        }
                    }
                    outcome.failed.push((rel, why));
                }
                if outcome.failed.is_empty() {
                    // `data/` subfolders emptied by the move would keep the
                    // folder from going.
                    self.prune_empty_data_dirs(&path_rel, dedicated, uuid.as_deref());
                    outcome.folder_removed = self.remove_book_folder_if_empty(&path_rel, dedicated);
                }
            }
        }
        Ok(outcome)
    }

    /// Removes a book's row, its custom-column values and its checksums in
    /// one immediate transaction: either all of it goes or none does. As three
    /// separate statements, a failure between them left a book with no hashes
    /// (so nothing could verify its files) or custom values with no book.
    ///
    /// The checksums live in an attached sidecar database, which SQLite
    /// commits separately from `metadata.db` in WAL mode, so that delete is
    /// issued last: the worst a crash can then leave is a hash for a book
    /// that is gone, never a book that has lost its hashes.
    fn delete_book_rows(&self, book_id: i32) -> anyhow::Result<()> {
        // Attaches the sidecar if need be; ATTACH cannot run inside a transaction.
        self.checksums().initialize()?;
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::custom_columns::purge_book(&tx, book_id)?;
        tx.execute("DELETE FROM books WHERE id = ?1", (book_id,))?;
        tx.execute("DELETE FROM checksums_db.file_checksums WHERE book_id = ?1", (book_id,))?;
        tx.commit()?;
        Ok(())
    }

    /// Removes directories left empty under the book's `data/` (or
    /// `data/<uuid>` for a root book). `remove_dir` only ever removes an empty
    /// directory, so no file can be lost here.
    fn prune_empty_data_dirs(&self, path_rel: &str, dedicated: bool, uuid: Option<&str>) {
        if !path_is_plain(path_rel) && !path_rel.is_empty() {
            return;
        }
        let library = &self.backend.library_path;
        let root = if path_rel.is_empty() {
            let Some(uuid) = uuid.filter(|u| !u.is_empty()) else { return };
            library.join(crate::constants::DATA_DIR_NAME).join(uuid)
        } else if dedicated {
            library.join(path_rel).join(crate::constants::DATA_DIR_NAME)
        } else {
            return;
        };
        fn prune(dir: &Path) {
            if let Ok(entries) = fs::read_dir(dir) {
                for e in entries.flatten() {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        prune(&e.path());
                    }
                }
            }
            let _ = fs::remove_dir(dir);
        }
        prune(&root);
    }

    /// True when no *other* book row has this `path`. An empty path is
    /// the library root and is never dedicated. Case-insensitive, so a
    /// case-folding filesystem cannot make two books share a folder
    /// unnoticed.
    pub(crate) fn folder_is_dedicated(&self, book_id: i32, path_rel: &str) -> anyhow::Result<bool> {
        if path_rel.is_empty() {
            return Ok(false);
        }
        let conn = self.backend.conn.lock().unwrap();
        let others: i64 = conn.query_row(
            "SELECT COUNT(*) FROM books WHERE id != ?1 AND lower(path) = lower(?2)",
            (book_id, path_rel),
            |row| row.get(0),
        )?;
        Ok(others == 0)
    }

    /// The files this book owns on disk, relative to the library root.
    pub(crate) fn own_files(&self, book_id: i32, path_rel: &str, dedicated: bool) -> anyhow::Result<Vec<OwnFile>> {
        if !path_rel.is_empty() && !path_is_plain(path_rel) {
            log::warn!("book {book_id} has an unusable path {path_rel:?}; none of its files will be touched");
            return Ok(Vec::new());
        }
        let checksums = self.checksums();
        let mut out = Vec::new();
        for (format, name) in self.format_file_names(book_id)? {
            if name.is_empty() || name.contains(['/', '\\']) || !path_is_plain(&name) {
                log::warn!("book {book_id} has an unusable format file name {name:?}; skipping it");
                continue;
            }
            // Another book claiming the very same file must keep it.
            let claimed_elsewhere: bool = {
                let conn = self.backend.conn.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM data d JOIN books b ON b.id = d.book
                     WHERE d.book != ?1 AND lower(b.path) = lower(?2)
                       AND lower(d.name) = lower(?3) AND upper(d.format) = upper(?4)",
                    (book_id, path_rel, &name, &format),
                    |row| row.get::<_, i64>(0),
                )? > 0
            };
            if claimed_elsewhere {
                continue;
            }
            let file = format!("{name}.{}", format.to_lowercase());
            let rel = if path_rel.is_empty() { file } else { format!("{path_rel}/{file}") };
            let (hash, _) = checksums.recorded_identity(book_id, "format", &format).unwrap_or((None, None));
            out.push(OwnFile { rel, hash, is_format: true });
        }
        if dedicated {
            for extra in ["cover.jpg", "metadata.opf"] {
                out.push(OwnFile { rel: format!("{path_rel}/{extra}"), hash: None, is_format: false });
            }
        }
        // The book's `data/` files (a dedicated folder's own `data/`, or
        // `data/<uuid>/` for a book in the library root) go with it, so a
        // delete -- and a merge, which deletes its sources -- leaves nothing
        // behind and a restore brings them back.
        if dedicated || path_rel.is_empty() {
            let library = &self.backend.library_path;
            if let Ok(extras) = crate::extra_files::list_extra_files(self, book_id, crate::constants::DATA_FILE_PATTERN) {
                for extra in extras {
                    let Ok(rel) = extra.file_path.strip_prefix(library) else { continue };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if path_is_plain(&rel) {
                        out.push(OwnFile { rel, hash: None, is_format: false });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Removes `path_rel` (and, for a dedicated calibre-style
    /// `Author/Title` folder, an author folder left empty) only if
    /// empty. Uses `fs::remove_dir`, which refuses a non-empty
    /// directory, so no content can be lost here even by a race.
    fn remove_book_folder_if_empty(&self, path_rel: &str, dedicated: bool) -> Option<String> {
        if path_rel.is_empty() || !path_is_plain(path_rel) {
            return None;
        }
        let library = &self.backend.library_path;
        let mut removed = None;
        let mut current = PathBuf::from(path_rel);
        loop {
            if current.as_os_str().is_empty() {
                break;
            }
            let absolute = library.join(&current);
            if !absolute.is_dir() || fs::remove_dir(&absolute).is_err() {
                break;
            }
            if removed.is_none() {
                removed = Some(path_rel.to_string());
            }
            if !dedicated {
                break;
            }
            match current.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => current = parent.to_path_buf(),
                _ => break,
            }
        }
        removed
    }

    /// Every format this book has, as `(FORMAT, filename stem)` --
    /// `data.name` being calibre's own record of what each format is
    /// actually called on disk.
    pub fn format_file_names(&self, book_id: i32) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT format, name FROM data WHERE book = ?1 ORDER BY format")?;
        let rows = stmt.query_map([book_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// The file name (`stem.ext`, extension lowercase) of each book's
    /// primary format, for the table's File name column and its sort.
    ///
    /// "Primary" is the first format in alphabetical order, the same one the
    /// JSON API calls `main_format`. `ids` of `None` means every book; it is
    /// one query either way (an `IN` list in chunks for a given set), never one
    /// per book and never a stat of the disk, so it stays cheap on 100k books.
    /// A format whose `data.name` is empty falls back to the sanitized title,
    /// as [`Cache::get_data_as_dict`] does.
    pub fn primary_file_names(&self, ids: Option<&[i32]>) -> Result<HashMap<i32, String>> {
        let mut best: HashMap<i32, (String, String)> = HashMap::new();
        let mut take = |book: i32, fmt: String, name: String| {
            let better = best.get(&book).is_none_or(|(f, _)| fmt < *f);
            if better {
                best.insert(book, (fmt, name));
            }
        };
        {
            let conn = self.backend.conn.lock().unwrap();
            match ids {
                None => {
                    let mut stmt = conn.prepare("SELECT book, lower(format), name FROM data")?;
                    for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
                        let (b, f, n) = row?;
                        take(b, f, n);
                    }
                }
                Some(ids) => {
                    for chunk in ids.chunks(500) {
                        let marks = vec!["?"; chunk.len()].join(",");
                        let mut stmt = conn.prepare(&format!("SELECT book, lower(format), name FROM data WHERE book IN ({marks})"))?;
                        for row in stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
                            let (b, f, n) = row?;
                            take(b, f, n);
                        }
                    }
                }
            }
        }
        let mut out = HashMap::with_capacity(best.len());
        for (book, (fmt, name)) in best {
            let stem = if name.is_empty() { self.field_for(book, "title")?.map(|t| sanitize_file_name(&t)).unwrap_or_default() } else { name };
            out.insert(book, format!("{stem}.{fmt}"));
        }
        Ok(out)
    }

    /// The stem this book's format files share, if they share one.
    ///
    /// `None` when the book has no formats, or when two formats
    /// disagree -- which is possible for a book whose formats were
    /// added under different titles, since [`Cache::add_format`] names
    /// a file after the title at the moment it is added.
    pub fn format_file_stem(&self, book_id: i32) -> anyhow::Result<Option<String>> {
        let names = self.format_file_names(book_id)?;
        let mut iter = names.into_iter().map(|(_, name)| name);
        let Some(first) = iter.next() else { return Ok(None) };
        Ok(if iter.all(|n| n == first) { Some(first) } else { None })
    }

    /// A stem this book can rename its files to without colliding
    /// with anything already in its folder.
    ///
    /// Returns `preferred` (sanitized) when it is free, and
    /// `preferred (1)`, `preferred (2)`... when it is not. Files this
    /// book already owns do not count as collisions -- renaming a book
    /// to the name it already has must not yield `Title (1)`.
    ///
    /// Callers use this *before* [`Cache::rename_format_files`], which
    /// refuses a collision rather than resolving one: a rename cannot
    /// be undone, so the name a preview shows has to be the name that
    /// is actually used, which means resolving collisions up front
    /// rather than at write time.
    pub fn available_format_stem(&self, book_id: i32, preferred: &str) -> anyhow::Result<String> {
        let preferred = sanitize_file_name(preferred);
        if preferred.trim().is_empty() {
            anyhow::bail!("a filename cannot be empty");
        }

        let Some(book_dir) = self.book_dir(book_id)? else {
            return Ok(preferred);
        };
        let formats = self.format_file_names(book_id)?;

        let mut candidate = preferred.clone();
        for suffix in 1..1000 {
            let free = formats.iter().all(|(format, own_name)| {
                let ext = format.to_lowercase();
                *own_name == candidate || !book_dir.join(format!("{candidate}.{ext}")).exists()
            });
            if free {
                return Ok(candidate);
            }
            candidate = format!("{preferred} ({suffix})");
        }
        anyhow::bail!("could not find an unused filename based on {preferred:?} in {}", book_dir.display())
    }

    /// Renames every one of a book's format files to `new_stem`,
    /// keeping each extension, and records the new name in `data`.
    ///
    /// This is the deliberate half of the title/filename split: it
    /// changes what the files are called and nothing else -- not the
    /// title, not the author, not the folder they sit in, all of
    /// which are metadata-derived and have their own operations.
    ///
    /// Driven by the `data` table rather than by listing the
    /// directory (the removed `rename_book_files` swept up every file in
    /// a folder, which is wrong for a folder shared by several books).
    /// The rows and the files have to end up
    /// agreeing, and iterating the rows means a file this does not
    /// know about (`cover.jpg`, `metadata.opf`, anything a user
    /// dropped in the folder) is left alone rather than swept up by a
    /// wildcard.
    ///
    /// Returns the formats it actually renamed. A format whose file is
    /// already called `new_stem` is not one of them.
    ///
    /// Refuses rather than overwrites if the destination exists and is
    /// not the file being renamed -- two books can legitimately share
    /// a folder (same author, same title), so a collision here is a
    /// real possibility and silently destroying the other book's file
    /// is not an acceptable outcome of a rename.
    pub fn rename_format_files(&self, book_id: i32, new_stem: &str) -> anyhow::Result<Vec<String>> {
        let new_stem = sanitize_file_name(new_stem);
        if new_stem.trim().is_empty() {
            anyhow::bail!("a filename cannot be empty");
        }

        let Some(book_dir) = self.book_dir(book_id)? else {
            return Ok(Vec::new());
        };

        let handle = self.backend.write_handle()?;
        let mut renamed = Vec::new();

        for (format, old_stem) in self.format_file_names(book_id)? {
            if old_stem == new_stem {
                continue;
            }
            let ext = format.to_lowercase();
            let old_path = book_dir.join(format!("{old_stem}.{ext}"));
            let new_path = book_dir.join(format!("{new_stem}.{ext}"));

            // On a case-insensitive volume `the hobbit.pdf` and
            // `The Hobbit.pdf` are one file, so `new_path` "existing" is
            // not a collision when it is the file being renamed.
            let same_file = new_path.exists() && lexmechanic_utils::filenames::samefile(&old_path, &new_path);
            if new_path.exists() && !same_file {
                anyhow::bail!("{} already exists", new_path.display());
            }
            // A `data` row whose file is missing is not a reason to
            // abandon the rename: the row still has to be corrected,
            // or it keeps pointing at a name nothing will ever have.
            if old_path.exists() {
                if same_file {
                    handle.rename_atomic(&old_path, &new_path)?;
                } else {
                    // The check above and the move are one step here, so a
                    // file created in between is refused, not replaced.
                    handle.rename_atomic_no_replace(&old_path, &new_path)?;
                }
            }

            {
                let conn = self.backend.conn.lock().unwrap();
                conn.execute("UPDATE data SET name = ?1 WHERE book = ?2 AND format = ?3", (&new_stem, book_id, &format))?;
            }

            // A rename is "this format is now this file", which is the
            // same statement `FormatSet` already makes -- one op for
            // both is what keeps `data.name` and the file on disk from
            // drifting the way they did before #885.
            let size = std::fs::metadata(&new_path).map(|m| m.len() as i64).unwrap_or(0);
            // A rename does not change the content: carry the recorded hash
            // forward (as `set_format_name` does) rather than logging a
            // `FormatSet` without one, which a rebuild would read as "no hash".
            let (recorded_hash, _) = self.checksums().recorded_identity(book_id, "format", &format).unwrap_or((None, None));
            let (format_name, stem) = (format.clone(), new_stem.clone());
            self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FormatSet { book, format: format_name, name: stem, size, hash: recorded_hash });
            renamed.push(format);
        }

        Ok(renamed)
    }

    /// Updates `title`/`author_sort` and the `authors`/
    /// `books_authors_link` rows to match.
    pub fn update_book_metadata(
        &self,
        book_id: i32,
        title: &str,
        author: &str,
    ) -> anyhow::Result<()> {
        // Deliberately no longer renames the book's files (#893).
        //
        // It used to rename the book's files first (the since-removed
        // `rename_book_files`), which moved the
        // book's folder to `<new author>/<new title>` and renamed every
        // format inside it. That made the two inseparable in the other
        // direction from #885: correcting a typo in a title silently
        // rearranged files the user had put where they wanted them, and
        // on a library synced or backed up by path it rewrote history
        // that other tools were tracking.
        //
        // Moving a book's files is its own operation (organize for the
        // folder, `rename_format_files` for the filenames) -- never a
        // side effect of editing metadata.
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        tx.execute(
            "UPDATE books SET title = ?1, author_sort = ?2 WHERE id = ?3",
            (title, author, book_id),
        )?;
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
        drop(conn);

        // This is the path `calibredb set_metadata title` takes, and it
        // appended nothing: a rebuild from the log restored the book's
        // old title. Found by the change-log audit, not by a user -- yet.
        //
        // The three fields it writes, as the same `FieldSet`s `set_field`
        // would record, so replay needs no special case. `authors` is
        // written as the single name it links, and `author_sort` as the
        // same string, exactly as the statements above do.
        let (title, author) = (title.to_string(), author.to_string());
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "title".to_string(), value: Some(title) });
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "author_sort".to_string(), value: Some(author.clone()) });
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field: "authors".to_string(), value: Some(author) });
        Ok(())
    }

    /// Recursively copies the entire library directory to `dest`.
    pub fn clone_to(&self, dest: &Path) -> anyhow::Result<()> {
        if !dest.exists() {
            fs::create_dir_all(dest)?;
        }
        copy_dir_recursive(&self.backend.library_path, dest)?;
        Ok(())
    }

    /// Port of `old_src/src/calibre/db/__init__.py`'s `get_data_as_dict`
    /// (issue #218): bulk metadata export as a list of JSON objects,
    /// including custom columns and resolved cover/format paths.
    ///
    /// `prefix` defaults to the library path (matching upstream);
    /// format/cover paths are rewritten relative to it when it's a
    /// different directory (e.g. `calibredb catalog`-style export
    /// consumers that want portable relative paths).
    ///
    /// # Disclosed simplifications
    ///
    /// - Custom-column values are included in the standard
    ///   `label -> value` shape, but the `{label}_index` companion
    ///   field upstream adds for `series`-datatype custom columns is
    ///   not -- this crate's custom-column model (#214) has no
    ///   dedicated index sub-column for any datatype, series included.
    /// - Format paths are resolved by re-deriving the filename
    ///   `add_format` (#216) would have used
    ///   (`sanitize_file_name(title).<fmt>`) and checking it exists,
    ///   not upstream's real `format_abspath` (which resolves via the
    ///   `data` table's own `name` column -- the two agree today since
    ///   `add_format`/`add_book` always write that same filename, but
    ///   a format added some other way with a different on-disk name
    ///   wouldn't be found by this).
    pub fn get_data_as_dict(
        &self,
        prefix: Option<&Path>,
        authors_as_string: bool,
        ids: Option<&std::collections::HashSet<i32>>,
        convert_to_local_tz: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let library_path = self.backend.library_path.clone();
        let prefix = prefix
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| library_path.clone());
        let custom_columns = self.custom_column_label_map()?;

        let book_ids: Vec<i32> = match ids {
            Some(set) => {
                let mut v: Vec<i32> = set.iter().copied().collect();
                v.sort_unstable();
                v
            }
            None => self.all_book_ids()?,
        };

        let mut out = Vec::with_capacity(book_ids.len());
        for book_id in book_ids {
            let Some(title) = self.field_for(book_id, "title")? else {
                continue;
            };
            let mut x = serde_json::Map::new();
            x.insert("id".into(), serde_json::json!(book_id));
            x.insert("title".into(), serde_json::json!(title));
            x.insert(
                "sort".into(),
                serde_json::json!(self.field_for(book_id, "sort")?),
            );
            x.insert(
                "author_sort".into(),
                serde_json::json!(self.field_for(book_id, "author_sort")?),
            );
            x.insert(
                "publisher".into(),
                serde_json::json!(self.field_for(book_id, "publisher")?),
            );
            x.insert(
                "rating".into(),
                serde_json::json!(self
                    .field_for(book_id, "rating")?
                    .and_then(|s| s.parse::<f64>().ok())),
            );
            x.insert(
                "size".into(),
                serde_json::json!(self
                    .field_for(book_id, "size")?
                    .and_then(|s| s.parse::<i64>().ok())),
            );
            x.insert(
                "series".into(),
                serde_json::json!(self.field_for(book_id, "series")?),
            );
            x.insert(
                "series_index".into(),
                serde_json::json!(self
                    .field_for(book_id, "series_index")?
                    .and_then(|s| s.parse::<f64>().ok())),
            );
            x.insert(
                "uuid".into(),
                serde_json::json!(self.field_for(book_id, "uuid")?),
            );
            x.insert(
                "comments".into(),
                serde_json::json!(self.field_for(book_id, "comments")?),
            );
            x.insert(
                "isbn".into(),
                serde_json::json!(self.field_for(book_id, "isbn")?.unwrap_or_default()),
            );
            x.insert(
                "identifiers".into(),
                serde_json::json!(self.field_for(book_id, "identifiers")?.unwrap_or_default()),
            );

            let authors: Vec<String> = match self.field_for(book_id, "authors")? {
                Some(s) if !s.is_empty() => s.split(" & ").map(|a| a.to_string()).collect(),
                _ => vec!["Unknown".to_string()],
            };
            x.insert(
                "authors".into(),
                if authors_as_string {
                    serde_json::json!(authors.join(" & "))
                } else {
                    serde_json::json!(authors)
                },
            );

            let tags: Vec<String> = self
                .field_for(book_id, "tags")?
                .map(|s| s.split(", ").map(|t| t.trim().to_string()).collect())
                .unwrap_or_default();
            x.insert("tags".into(), serde_json::json!(tags));

            let languages: Vec<String> = self
                .field_for(book_id, "languages")?
                .map(|s| s.split(", ").map(|t| t.to_string()).collect())
                .unwrap_or_default();
            x.insert("languages".into(), serde_json::json!(languages));

            for field in ["timestamp", "pubdate", "last_modified"] {
                let raw = self.field_for(book_id, field)?;
                let value = if convert_to_local_tz {
                    raw.as_deref()
                        .and_then(|s| lexmechanic_utils::date::parse_date(s, true))
                        .map(|dt| dt.with_timezone(&chrono::Local).to_rfc3339())
                } else {
                    raw
                };
                x.insert(field.into(), serde_json::json!(value));
            }

            let path_rel = self.field_for(book_id, "path")?.unwrap_or_default();
            let has_cover: bool = {
                let conn = self.backend.conn.lock().unwrap();
                conn.query_row(
                    "SELECT has_cover FROM books WHERE id = ?1",
                    [book_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0)
                    != 0
            };
            x.insert(
                "cover".into(),
                serde_json::json!(if has_cover {
                    // Where the cover really is: `.lexmechanic/covers/<uuid>.jpg`,
                    // or a legacy `<folder>/cover.jpg` (see `covers::cover_path`).
                    // Hard-coding `<folder>/cover.jpg` 404'd every cover in a
                    // scanned library. Re-rooted under `prefix` like the rest.
                    let real = crate::covers::cover_path(self, book_id)
                        .unwrap_or_else(|_| library_path.join(&path_rel).join("cover.jpg"));
                    let rel = real.strip_prefix(&library_path).map(Path::to_path_buf).unwrap_or(real);
                    Some(prefix.join(rel).to_string_lossy().to_string())
                } else {
                    None
                }),
            );

            let mut formats = Vec::new();
            let mut available_formats = Vec::new();
            // `name` is the file's own stem as recorded when the format
            // was added. Reading it -- rather than recomputing
            // `sanitize_file_name(title)` -- is what keeps a book
            // openable after its title is edited: the file on disk does
            // not change name, so recomputing produced a path that did
            // not exist and the format silently vanished from the row.
            // Every route that serves a book (`/get/{fmt}`, convert,
            // open-externally, download) reads these keys, so all of
            // them 404'd at once.
            let mut data_formats: Vec<(String, String)> = Vec::new();
            {
                let conn = self.backend.conn.lock().unwrap();
                let mut stmt = conn.prepare("SELECT format, name FROM data WHERE book = ?1")?;
                let rows = stmt.query_map([book_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
                for row in rows {
                    data_formats.push(row?);
                }
            }
            for (fmt, stem) in &data_formats {
                available_formats.push(fmt.to_uppercase());
                // An older row, or one written before `name` was
                // populated, falls back to the title -- which is what
                // the filename would have been at that point anyway.
                let stem = if stem.is_empty() { sanitize_file_name(&title) } else { stem.clone() };
                let file_name = format!("{}.{}", stem, fmt.to_lowercase());
                let abs_path = library_path.join(&path_rel).join(&file_name);
                if abs_path.exists() {
                    let out_path = if prefix != library_path {
                        match abs_path.strip_prefix(&library_path) {
                            Ok(rel) => prefix.join(rel),
                            Err(_) => abs_path.clone(),
                        }
                    } else {
                        abs_path.clone()
                    };
                    let out_path_str = out_path.to_string_lossy().to_string();
                    x.insert(
                        format!("fmt_{}", fmt.to_lowercase()),
                        serde_json::json!(out_path_str),
                    );
                    formats.push(out_path_str);
                }
            }
            x.insert("formats".into(), serde_json::json!(formats));
            x.insert(
                "available_formats".into(),
                serde_json::json!(available_formats),
            );

            for label in custom_columns.keys() {
                let val = self.get_custom_column_value(book_id, label)?;
                x.insert(label.clone(), serde_json::json!(val));
                if custom_columns[label]["datatype"] == "series" {
                    let index = self.get_custom_series_index(book_id, label)?;
                    x.insert(format!("{label}_index"), serde_json::json!(index));
                }
            }

            out.push(serde_json::Value::Object(x));
        }

        Ok(out)
    }

    /// Real custom-column support (issue #212 follow-up), moved here
    /// from `library.rs`'s previously Cache-side-only duplicate of the
    /// same logic -- `Library::add_custom_column`/etc. now delegate to
    /// these instead of hand-rolling their own SQL, now that both
    /// share the real schema (#212). NOT a port of upstream's real
    /// `tables.py`/`fields.py` custom-column architecture (per-column
    /// `Table` subclasses bulk-loaded into memory on library open,
    /// `CustomColumns`/`initialize_custom_columns` in `backend.py`) --
    /// that's a much larger, separate rearchitecture of how this
    /// crate accesses fields at all (`Cache::field_for` already hits
    /// the DB directly per call rather than an in-memory table model,
    /// its own disclosed simplification -- see this module's docs).
    /// This is a narrower, same-shape extension of that existing
    /// per-call SQL strategy to custom columns: a `custom_columns`
    /// metadata row plus a dynamic `custom_column_N` value table per
    /// column, same as `Library`'s original implementation, just
    /// hosted on `Cache`'s connection instead of a second one.
    pub fn custom_column_label_map(&self) -> Result<HashMap<String, serde_json::Value>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized FROM custom_columns",
        )?;
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

        let mut out = HashMap::new();
        for row in rows {
            let (label, data) = row?;
            out.insert(label, data);
        }
        Ok(out)
    }

    /// Port of `LibraryDatabase2.all_tags`: every row in the `tags`
    /// table (not narrowed to tags actually attached to a book, unlike
    /// most of this crate's tag-reading paths), trimmed, with blanks
    /// dropped.
    pub fn all_tags(&self) -> Result<Vec<String>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM tags")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let name = row?.trim().to_string();
            if !name.is_empty() {
                out.push(name);
            }
        }
        Ok(out)
    }

    /// Port of `LibraryDatabase2.last_modified`: the most recent
    /// `last_modified` timestamp across every book, or the Unix epoch
    /// for an empty library (matching upstream's own `UNDEFINED_DATE`-style
    /// "nothing to report" fallback rather than `None`/an error).
    pub fn last_modified(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        let raw: Option<String> = {
            let conn = self.backend.conn.lock().unwrap();
            conn.query_row("SELECT MAX(last_modified) FROM books", [], |row| row.get(0)).optional()?.flatten()
        };
        Ok(raw.as_deref().and_then(|s| lexmechanic_utils::date::parse_date(s, true)).unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH))
    }

    /// Creates a custom column with calibre's exact schema for its
    /// datatype (see [`crate::custom_columns`]): a one-table column for
    /// `datetime`/`comments`/`int`/`bool`/`float`/`composite`, a value
    /// table plus link table (with indices, triggers and tag-browser
    /// views) for `text`/`series`/`enumeration`/`rating`. `is_multiple`
    /// only applies to `text` and `composite`, as in calibre.
    pub fn add_custom_column(&self, label: &str, name: &str, datatype: &str, is_multiple: bool) -> anyhow::Result<i32> {
        self.add_custom_column_with_display(label, name, datatype, is_multiple, &serde_json::json!({}))
    }

    /// [`Cache::add_custom_column`] with the column's `display` settings
    /// (an enumeration's `enum_values`, a text column's `is_names`, ...).
    pub fn add_custom_column_with_display(
        &self,
        label: &str,
        name: &str,
        datatype: &str,
        is_multiple: bool,
        display: &serde_json::Value,
    ) -> anyhow::Result<i32> {
        crate::custom_columns::validate_new_column(label, datatype)?;
        let normalized = crate::custom_columns::is_normalized(datatype);
        let is_multiple = is_multiple && matches!(datatype, "text" | "composite");
        let display_json = serde_json::to_string(display)?;
        let mut conn = self.backend.conn.lock().unwrap();

        let count: i32 = conn.query_row("SELECT COUNT(*) FROM custom_columns WHERE label = ?1", [label], |row| row.get(0))?;
        if count > 0 {
            anyhow::bail!("Column with label '{}' already exists", label);
        }

        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO custom_columns(label, name, datatype, is_multiple, editable, display, normalized) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
            (label, name, datatype, is_multiple, &display_json, normalized),
        )?;
        let col_id = tx.last_insert_rowid();
        crate::custom_columns::create_schema(&tx, col_id, datatype, normalized)?;
        tx.commit()?;
        // Before recording: appending fsyncs, and #901 warns against
        // doing that with the connection mutex held.
        drop(conn);
        self.record(crate::change_log::ChangeOp::CustomColumnAdded {
            label: label.to_string(),
            name: name.to_string(),
            datatype: datatype.to_string(),
            is_multiple,
            display: (display_json != "{}").then_some(display_json),
        });
        Ok(col_id as i32)
    }

    /// A custom column's definition (storage shape, separators, enum
    /// values), by label (a leading `#` is accepted).
    pub fn custom_column_def(&self, label: &str) -> Result<Option<crate::custom_columns::ColumnDef>> {
        let conn = self.backend.conn.lock().unwrap();
        crate::custom_columns::column(&conn, label)
    }

    fn custom_column_lookup(&self, label: &str) -> Result<Option<(i32, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(crate::custom_columns::column(&conn, label)?.map(|c| (c.num, c.datatype)))
    }

    /// A book's value in a custom column, as text. A multiple-value column
    /// comes back joined with its separator (`", "`, or `" & "` for a
    /// names column) in the order it was written; a rating is calibre's
    /// 0-10 scale; a series is just the series name -- read the index with
    /// [`Cache::get_custom_series_index`], or ask for `<label>_index`.
    pub fn get_custom_column_value(&self, book_id: i32, label: &str) -> Result<Option<String>> {
        use crate::custom_columns::{self as cc, Target};
        let conn = self.backend.conn.lock().unwrap();
        match cc::resolve(&conn, label)? {
            None => Ok(None),
            Some(Target::Value(def)) => cc::read_value(&conn, &def, book_id),
            Some(Target::SeriesIndex(def)) => Ok(cc::read_series_index(&conn, &def, book_id)?.map(|n| n.to_string())),
        }
    }

    /// A book's index in a series custom column (`None` without a series).
    pub fn get_custom_series_index(&self, book_id: i32, label: &str) -> Result<Option<f64>> {
        let conn = self.backend.conn.lock().unwrap();
        match crate::custom_columns::column(&conn, label)? {
            Some(def) => crate::custom_columns::read_series_index(&conn, &def, book_id),
            None => Ok(None),
        }
    }

    /// Sets a custom column's value for a book, the way calibre does for
    /// the column's datatype (see [`crate::custom_columns`]). An empty
    /// value clears it. `<label>_index` sets a series column's index.
    /// Errors for an enumeration value outside the column's list and for
    /// text that is not a number/date/yes-no where the column needs one.
    pub fn set_custom_column_value(&self, book_id: i32, label: &str, value: &str) -> anyhow::Result<()> {
        use crate::custom_columns::{self as cc, Target};
        let label = label.strip_prefix('#').unwrap_or(label);
        let conn = self.backend.conn.lock().unwrap();
        let Some(target) = cc::resolve(&conn, label)? else {
            anyhow::bail!("Custom column with label '{}' not found", label);
        };
        // What goes in the log: the column's value, and for a series the
        // index it ended up with, so a replay (or a compacted log) does not
        // depend on what index the book happened to hold before.
        let mut logged = Vec::new();
        match target {
            Target::Value(def) => {
                let index = cc::write_value(&conn, &def, book_id, value)?;
                let value = if def.datatype == "series" { cc::split_series_value(value).0 } else { value.to_string() };
                logged.push((def.label.clone(), value));
                if let Some(idx) = index {
                    logged.push((format!("{}_index", def.label), idx.to_string()));
                }
            }
            Target::SeriesIndex(def) => {
                if let Some(idx) = cc::write_series_index(&conn, &def, book_id, value)? {
                    logged.push((format!("{}_index", def.label), idx.to_string()));
                }
            }
        }
        drop(conn);

        // Named `#label`, the way calibre names a custom column's field,
        // so replay can tell it from a built-in one.
        for (field, value) in logged {
            let field = format!("#{field}");
            self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field, value: Some(value) });
        }
        Ok(())
    }

    /// A custom column's id by label, or `None` if there is no such
    /// column. Lets replay tell "already applied" from "needs applying"
    /// without provoking the duplicate-label error.
    pub fn custom_column_id(&self, label: &str) -> anyhow::Result<Option<i32>> {
        Ok(self.custom_column_lookup(label)?.map(|(id, _)| id))
    }

    pub fn remove_custom_column(&self, label: &str) -> anyhow::Result<()> {
        {
            let mut conn = self.backend.conn.lock().unwrap();
            let Some(def) = crate::custom_columns::column(&conn, label)? else {
                anyhow::bail!("Column '{}' not found", label);
            };
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM custom_columns WHERE id = ?1", [def.num])?;
            crate::custom_columns::drop_schema(&tx, def.num)?;
            tx.commit()?;
        }
        self.record(crate::change_log::ChangeOp::CustomColumnRemoved { label: label.to_string() });
        Ok(())
    }

    /// Renames a category item (author/tag/publisher) and, if that
    /// collides with an existing name (the `name` column's `UNIQUE`
    /// constraint), merges by re-pointing the old item's links at the
    /// existing one and deleting the now-orphaned old row -- ported
    /// from `legacy::LegacyDb::rename_item`'s identical logic
    /// (issues #223/#749), moved here (and `legacy.rs`'s own
    /// `rename_author`/`rename_tag`/`rename_publisher` now delegate to
    /// this) so a real caller with only a `Cache` in scope (like
    /// `lexmechanic_srv`, which never opens a second `LegacyDb`/`Cache`
    /// connection onto the same library file) can reach it directly.
    /// A real, simplified stand-in for upstream's fuller
    /// `rename_items` (which also updates every affected book's
    /// composite/sort fields), matching `legacy.rs`'s own already-
    /// disclosed narrowing.
    /// An author's, tag's or publisher's id by name, for replay: ids are
    /// local to one machine, so a log can only refer to an item by name.
    pub fn item_id_by_name(&self, table: &str, name: &str) -> anyhow::Result<Option<i32>> {
        anyhow::ensure!(matches!(table, "authors" | "tags" | "publishers"), "not a renameable item table: {table}");
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row(&format!("SELECT id FROM {table} WHERE name = ?1"), [name], |row| row.get(0)).optional()?)
    }

    fn rename_item(&self, table: &str, link_table: &str, link_col: &str, old_id: i32, new_name: &str) -> anyhow::Result<()> {
        // The old name has to be read before the rename, since afterwards
        // there is nothing left to read it from.
        let old_name: Option<String> = {
            let conn = self.backend.conn.lock().unwrap();
            conn.query_row(&format!("SELECT name FROM {table} WHERE id = ?1"), [old_id], |row| row.get(0)).optional()?
        };
        self.rename_item_in_db(table, link_table, link_col, old_id, new_name)?;
        // Outside the connection lock: appending fsyncs.
        if let Some(from) = old_name {
            self.record(crate::change_log::ChangeOp::ItemRenamed { kind: table.to_string(), from, to: new_name.to_string() });
        }
        Ok(())
    }

    fn rename_item_in_db(&self, table: &str, link_table: &str, link_col: &str, old_id: i32, new_name: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<i32> = tx.query_row(&format!("SELECT id FROM {table} WHERE name = ?1 AND id != ?2"), (new_name, old_id), |row| row.get(0)).optional()?;
        match existing {
            Some(target_id) => {
                tx.execute(&format!("UPDATE OR IGNORE {link_table} SET {link_col} = ?1 WHERE {link_col} = ?2"), (target_id, old_id))?;
                tx.execute(&format!("DELETE FROM {link_table} WHERE {link_col} = ?1"), [old_id])?;
                tx.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [old_id])?;
            }
            None => {
                tx.execute(&format!("UPDATE {table} SET name = ?1 WHERE id = ?2"), (new_name, old_id))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Writes the stored sort key of the author called `name`, or does nothing
    /// when there is no such author. Replay of [`crate::change_log::ChangeOp::AuthorSortSet`];
    /// not logged itself.
    pub(crate) fn set_author_sort_key(&self, name: &str, sort: &str) -> anyhow::Result<()> {
        let conn = self.backend.conn.lock().unwrap();
        conn.execute("UPDATE authors SET sort = ?1 WHERE name = ?2", (sort, name))?;
        Ok(())
    }

    pub fn rename_author(&self, old_id: i32, new_name: &str) -> anyhow::Result<()> {
        self.rename_item("authors", "books_authors_link", "author", old_id, new_name)
    }

    pub fn rename_tag(&self, old_id: i32, new_name: &str) -> anyhow::Result<()> {
        self.rename_item("tags", "books_tags_link", "tag", old_id, new_name)
    }

    pub fn rename_publisher(&self, old_id: i32, new_name: &str) -> anyhow::Result<()> {
        self.rename_item("publishers", "books_publishers_link", "publisher", old_id, new_name)
    }

    pub fn has_cover(&self, book_id: i32) -> anyhow::Result<bool> {
        let conn = self.backend.conn.lock().unwrap();
        let has_cover: Option<i64> = conn
            .query_row(
                "SELECT has_cover FROM books WHERE id = ?1",
                [book_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(has_cover.unwrap_or(0) != 0)
    }

    /// Ported from `Library::get_book`'s identical SQL (issue #748:
    /// `check_library::CheckLibrary` was written against `Library`,
    /// the pre-`Cache` API -- `Library` and `Cache` are sibling
    /// structs, each independently owning their own `Backend`/
    /// connection, not wrapper/wrapped like `legacy::LegacyDb` turned
    /// out to be for #749, so this couldn't be a one-line delegation;
    /// the real SQL had to move here too).
    pub fn get_book(&self, id: i32) -> anyhow::Result<Option<Book>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, sort, timestamp, pubdate, series_index, author_sort, isbn, lccn, path, has_cover, uuid FROM books WHERE id = ?1")?;
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

    /// Ported from `Library::all_authors`'s identical SQL (issue #748,
    /// see [`Cache::get_book`]'s own doc for why this moved here).
    pub fn all_authors(&self) -> anyhow::Result<Vec<(i32, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM authors")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut authors = Vec::new();
        for row in rows {
            authors.push(row?);
        }
        Ok(authors)
    }

    /// Ported from `Library::format_files`'s identical SQL (issue
    /// #748, see [`Cache::get_book`]'s own doc for why this moved
    /// here).
    pub fn format_files(&self, book_id: i32) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name, format FROM data WHERE book = ?1")?;
        let rows = stmt.query_map([book_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut formats = Vec::new();
        for row in rows {
            formats.push(row?);
        }
        Ok(formats)
    }

    /// Ported from `Library::is_case_sensitive`'s identical stub
    /// (issue #748, see [`Cache::get_book`]'s own doc for why this
    /// moved here) -- real upstream detects the *filesystem's* real
    /// case-sensitivity; neither this port's `Library` nor `Cache` has
    /// ever done that real detection, both always report `false`.
    pub fn is_case_sensitive(&self) -> bool {
        false
    }

    /// Real generic field writer backing `legacy.rs`'s (#223) setter
    /// API. Upstream's `Cache.set_field` is ~250 lines handling every
    /// field (including composites), `allow_case_change`, dirtying,
    /// and change notification; this ports the write path for each
    /// *standard* field [`Cache::field_for`] already reads (the same
    /// set, minus `id`/`path`/`last_modified`/`size`, none of which
    /// upstream's own legacy setter API exposes either), using the
    /// same get-or-create-by-name pattern [`Cache::add_book_db_entry`]
    /// already uses for authors.
    ///
    /// # Disclosed simplifications
    ///
    /// - Many-to-many fields (`tags`/`languages`/`authors`) take
    ///   `value` pre-joined the same way [`Cache::field_for`] returns
    ///   them (`", "` for tags/languages, `" & "` for authors) and
    ///   replace the entire set -- no single-item add/remove, no
    ///   `allow_case_change` merge-on-rename behavior.
    /// - `identifiers` takes the same `"type:val,type:val"` string
    ///   [`Cache::field_for`] returns and replaces the entire set.
    /// - No dirtying/notification, no composite-field recalculation
    ///   (e.g. setting `authors` does not recompute `author_sort`).
    pub fn set_field(&self, book_id: i32, field: &str, value: &str) -> anyhow::Result<()> {
        self.set_field_inner(book_id, field, value)?;
        // Recorded after the write, not before: a change that was
        // logged and then failed to apply would come back on the next
        // rebuild as an edit the user never successfully made.
        let (field, value) = (field.to_string(), value.to_string());
        self.record_for_book(book_id, |book| crate::change_log::ChangeOp::FieldSet { book, field, value: Some(value) });
        Ok(())
    }

    fn set_field_inner(&self, book_id: i32, field: &str, value: &str) -> anyhow::Result<()> {
        match field {
            "title" | "sort" | "uuid" if value.is_empty() => {
                // Matches upstream: these three fields silently no-op
                // on an empty value rather than clearing it.
            }
            // `isbn` and `lccn` are legacy `books` columns calibre keeps for
            // compatibility. `Library::set_metadata` wrote them with raw SQL
            // because `set_field` had no arm for them, which is also how
            // those writes escaped the change log.
            "title" | "sort" | "author_sort" | "uuid" | "isbn" | "lccn" => {
                let conn = self.backend.conn.lock().unwrap();
                conn.execute(
                    &format!("UPDATE books SET {field} = ?1 WHERE id = ?2"),
                    (value, book_id),
                )?;
            }
            "series_index" => {
                let val: f64 = value.parse().unwrap_or(1.0);
                let conn = self.backend.conn.lock().unwrap();
                conn.execute(
                    "UPDATE books SET series_index = ?1 WHERE id = ?2",
                    (val, book_id),
                )?;
            }
            "timestamp" | "pubdate" => {
                let conn = self.backend.conn.lock().unwrap();
                conn.execute(
                    &format!("UPDATE books SET {field} = ?1 WHERE id = ?2"),
                    (value, book_id),
                )?;
            }
            "comments" => {
                let conn = self.backend.conn.lock().unwrap();
                conn.execute("DELETE FROM comments WHERE book = ?1", (book_id,))?;
                if !value.is_empty() {
                    conn.execute(
                        "INSERT INTO comments (book, text) VALUES (?1, ?2)",
                        (book_id, value),
                    )?;
                }
            }
            "has_cover" | "cover" => {
                let flag = matches!(value, "1" | "true" | "True");
                let conn = self.backend.conn.lock().unwrap();
                conn.execute(
                    "UPDATE books SET has_cover = ?1 WHERE id = ?2",
                    (flag as i32, book_id),
                )?;
            }
            "series" => {
                self.set_many_to_one_field(book_id, "series", "books_series_link", "series", value)?
            }
            "publisher" => self.set_many_to_one_field(
                book_id,
                "publishers",
                "books_publishers_link",
                "publisher",
                value,
            )?,
            "rating" => self.set_rating_field(book_id, value)?,
            "tags" => self.set_many_to_many_field(
                book_id,
                "tags",
                "name",
                "books_tags_link",
                "tag",
                ", ",
                value,
            )?,
            "languages" => self.set_many_to_many_field(
                book_id,
                "languages",
                "lang_code",
                "books_languages_link",
                "lang_code",
                ", ",
                value,
            )?,
            "authors" => self.set_many_to_many_field(
                book_id,
                "authors",
                "name",
                "books_authors_link",
                "author",
                " & ",
                value,
            )?,
            "identifiers" => self.set_identifiers_field(book_id, value)?,
            // Any other name is checked against the real
            // `custom_columns` table before giving up -- issue #720
            // found this fallback used to unconditionally reject
            // every custom-column label, contradicting that issue's
            // own filing claim that `set_field` "already" wrote
            // custom columns. It didn't; this is the fix.
            _ if {
                // A column (`label` or `#label`) or a series column's
                // `<label>_index`.
                let conn = self.backend.conn.lock().unwrap();
                crate::custom_columns::resolve(&conn, field)?.is_some()
            } =>
            {
                self.set_custom_column_value(book_id, field, value)?
            }
            _ => anyhow::bail!("Field '{}' is not writable by this port's set_field", field),
        }
        Ok(())
    }

    fn set_many_to_one_field(
        &self,
        book_id: i32,
        table: &str,
        link_table: &str,
        link_col: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::link_many_to_one_in(&tx, book_id, table, link_table, link_col, value)?;
        tx.commit()?;
        Ok(())
    }

    /// The body of [`Cache::set_many_to_one_field`] on a transaction the
    /// caller owns, so a batch of books can share one commit.
    pub(crate) fn link_many_to_one_in(tx: &rusqlite::Connection, book_id: i32, table: &str, link_table: &str, link_col: &str, value: &str) -> anyhow::Result<()> {
        tx.execute(
            &format!("DELETE FROM {link_table} WHERE book = ?1"),
            (book_id,),
        )?;
        let value = value.trim();
        if !value.is_empty() {
            let item_id: i32 = {
                let mut stmt = tx.prepare(&format!("SELECT id FROM {table} WHERE name = ?1"))?;
                let mut rows = stmt.query([value])?;
                if let Some(row) = rows.next()? {
                    row.get(0)?
                } else {
                    tx.execute(&format!("INSERT INTO {table} (name) VALUES (?1)"), [value])?;
                    tx.last_insert_rowid() as i32
                }
            };
            tx.execute(
                &format!("INSERT INTO {link_table} (book, {link_col}) VALUES (?1, ?2)"),
                (book_id, item_id),
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn set_many_to_many_field(
        &self,
        book_id: i32,
        table: &str,
        name_col: &str,
        link_table: &str,
        link_col: &str,
        sep: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::link_many_to_many_in(&tx, book_id, table, name_col, link_table, link_col, sep, value)?;
        tx.commit()?;
        Ok(())
    }

    /// The body of [`Cache::set_many_to_many_field`] on a transaction the
    /// caller owns, so a batch of books can share one commit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn link_many_to_many_in(
        tx: &rusqlite::Connection,
        book_id: i32,
        table: &str,
        name_col: &str,
        link_table: &str,
        link_col: &str,
        sep: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        // Link tables are UNIQUE(book, item) over NOCASE names: a repeated
        // item ("a & A") must be linked once, not abort the write.
        let items: Vec<&str> = dedupe_names(value.split(sep));
        tx.execute(
            &format!("DELETE FROM {link_table} WHERE book = ?1"),
            (book_id,),
        )?;
        for (idx, item) in items.iter().enumerate() {
            let item_id: i32 = {
                let mut stmt =
                    tx.prepare(&format!("SELECT id FROM {table} WHERE {name_col} = ?1"))?;
                let mut rows = stmt.query([*item])?;
                if let Some(row) = rows.next()? {
                    row.get(0)?
                } else {
                    tx.execute(
                        &format!("INSERT INTO {table} ({name_col}) VALUES (?1)"),
                        [*item],
                    )?;
                    tx.last_insert_rowid() as i32
                }
            };
            if link_table == "books_languages_link" {
                tx.execute(
                    &format!(
                        "INSERT INTO {link_table} (book, {link_col}, item_order) VALUES (?1, ?2, ?3)"
                    ),
                    (book_id, item_id, idx as i32),
                )?;
            } else {
                tx.execute(
                    &format!("INSERT INTO {link_table} (book, {link_col}) VALUES (?1, ?2)"),
                    (book_id, item_id),
                )?;
            }
        }
        Ok(())
    }

    fn set_rating_field(&self, book_id: i32, value: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::set_rating_in(&tx, book_id, value)?;
        tx.commit()?;
        Ok(())
    }

    /// The body of [`Cache::set_rating_field`] on a transaction the caller owns.
    pub(crate) fn set_rating_in(tx: &rusqlite::Connection, book_id: i32, value: &str) -> anyhow::Result<()> {
        let rating: i32 = value.parse().unwrap_or(0);
        tx.execute("DELETE FROM books_ratings_link WHERE book = ?1", (book_id,))?;
        if rating > 0 {
            let rating_id: i32 = {
                let mut stmt = tx.prepare("SELECT id FROM ratings WHERE rating = ?1")?;
                let mut rows = stmt.query([rating])?;
                if let Some(row) = rows.next()? {
                    row.get(0)?
                } else {
                    tx.execute("INSERT INTO ratings (rating) VALUES (?1)", [rating])?;
                    tx.last_insert_rowid() as i32
                }
            };
            tx.execute(
                "INSERT INTO books_ratings_link (book, rating) VALUES (?1, ?2)",
                (book_id, rating_id),
            )?;
        }
        Ok(())
    }

    fn set_identifiers_field(&self, book_id: i32, value: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::set_identifiers_in(&tx, book_id, value)?;
        tx.commit()?;
        Ok(())
    }

    /// The body of [`Cache::set_identifiers_field`] on a transaction the caller owns.
    pub(crate) fn set_identifiers_in(tx: &rusqlite::Connection, book_id: i32, value: &str) -> anyhow::Result<()> {
        tx.execute("DELETE FROM identifiers WHERE book = ?1", (book_id,))?;
        for pair in value.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once(':') {
                if !k.is_empty() && !v.is_empty() {
                    tx.execute(
                        "INSERT INTO identifiers (book, type, val) VALUES (?1, ?2, ?3)",
                        (book_id, k, v),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The fields [`Cache::set_fields_for_books`] writes inside its one
    /// transaction. Anything else (custom columns, whose writes run their own
    /// transaction) goes through [`Cache::set_field`] per book.
    fn is_batchable_field(field: &str) -> bool {
        matches!(
            field,
            "title" | "sort" | "author_sort" | "isbn" | "series_index" | "timestamp" | "pubdate" | "comments" | "has_cover" | "series" | "publisher" | "rating" | "tags" | "languages" | "authors" | "identifiers"
        )
    }

    /// [`Cache::set_field_inner`] for the batchable fields, on a transaction the
    /// caller owns. Same writes, same value rules.
    fn set_field_in_tx(tx: &rusqlite::Connection, book_id: i32, field: &str, value: &str) -> anyhow::Result<()> {
        match field {
            "title" | "sort" if value.is_empty() => {}
            "title" | "sort" | "author_sort" | "isbn" => {
                tx.execute(&format!("UPDATE books SET {field} = ?1 WHERE id = ?2"), (value, book_id))?;
            }
            "series_index" => {
                let val: f64 = value.parse().unwrap_or(1.0);
                tx.execute("UPDATE books SET series_index = ?1 WHERE id = ?2", (val, book_id))?;
            }
            "timestamp" | "pubdate" => {
                tx.execute(&format!("UPDATE books SET {field} = ?1 WHERE id = ?2"), (value, book_id))?;
            }
            "comments" => {
                tx.execute("DELETE FROM comments WHERE book = ?1", (book_id,))?;
                if !value.is_empty() {
                    tx.execute("INSERT INTO comments (book, text) VALUES (?1, ?2)", (book_id, value))?;
                }
            }
            "has_cover" => {
                let flag = matches!(value, "1" | "true" | "True");
                tx.execute("UPDATE books SET has_cover = ?1 WHERE id = ?2", (flag as i32, book_id))?;
            }
            "series" => Self::link_many_to_one_in(tx, book_id, "series", "books_series_link", "series", value)?,
            "publisher" => Self::link_many_to_one_in(tx, book_id, "publishers", "books_publishers_link", "publisher", value)?,
            "rating" => Self::set_rating_in(tx, book_id, value)?,
            "tags" => Self::link_many_to_many_in(tx, book_id, "tags", "name", "books_tags_link", "tag", ", ", value)?,
            "languages" => Self::link_many_to_many_in(tx, book_id, "languages", "lang_code", "books_languages_link", "lang_code", ", ", value)?,
            "authors" => Self::link_many_to_many_in(tx, book_id, "authors", "name", "books_authors_link", "author", " & ", value)?,
            "identifiers" => Self::set_identifiers_in(tx, book_id, value)?,
            other => anyhow::bail!("Field '{other}' is not writable by this port's set_field"),
        }
        Ok(())
    }

    /// Applies the same `changes` (`(field, value)` as [`Cache::set_field`]
    /// takes them) to many books, for the bulk-edit batch route.
    ///
    /// The standard fields of every book are written in **one** `BEGIN
    /// IMMEDIATE` transaction with a savepoint per book, so a book that fails is
    /// rolled back on its own and the rest commit together -- one journal sync
    /// per call instead of one per field per book, which is what makes editing
    /// 30,000 books on a network share tolerable. The change log gets one batch
    /// entry for the lot (database first, log second, as every write here).
    /// Fields the transaction cannot take (custom columns) are then applied per
    /// book through [`Cache::set_field`].
    ///
    /// The result has one entry per id, in order: `Ok` when every field of that
    /// book was written.
    pub fn set_fields_for_books(&self, book_ids: &[i32], changes: &[(String, String)]) -> Vec<anyhow::Result<()>> {
        let (batchable, other): (Vec<&(String, String)>, Vec<&(String, String)>) = changes.iter().partition(|(f, _)| Self::is_batchable_field(f));
        let mut results: Vec<anyhow::Result<()>> = Vec::with_capacity(book_ids.len());
        let mut uuids: Vec<Option<String>> = Vec::with_capacity(book_ids.len());
        for id in book_ids {
            uuids.push(self.book_uuid(*id).ok().flatten());
        }

        if !batchable.is_empty() {
            let mut conn = self.backend.conn.lock().unwrap();
            let mut tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(tx) => tx,
                Err(e) => {
                    let msg = format!("{e}");
                    return book_ids.iter().map(|_| Err(anyhow::anyhow!("{msg}"))).collect();
                }
            };
            for (idx, id) in book_ids.iter().enumerate() {
                let one = (|| -> anyhow::Result<()> {
                    if uuids[idx].is_none() {
                        anyhow::bail!("Book {id} not found");
                    }
                    let sp = tx.savepoint()?;
                    for (field, value) in &batchable {
                        Self::set_field_in_tx(&sp, *id, field, value)?;
                    }
                    sp.commit()?;
                    Ok(())
                })();
                results.push(one);
            }
            if let Err(e) = tx.commit() {
                let msg = format!("{e}");
                return book_ids.iter().map(|_| Err(anyhow::anyhow!("{msg}"))).collect();
            }
            drop(conn);

            let mut ops = Vec::new();
            for (idx, result) in results.iter().enumerate() {
                if let (Ok(()), Some(uuid)) = (result, &uuids[idx]) {
                    for (field, value) in &batchable {
                        ops.push(crate::change_log::ChangeOp::FieldSet { book: uuid.clone(), field: field.clone(), value: Some(value.clone()) });
                    }
                }
            }
            self.record_many(ops);
        } else {
            results.extend(book_ids.iter().map(|_| Ok(())));
        }

        for (idx, id) in book_ids.iter().enumerate() {
            if results[idx].is_err() {
                continue;
            }
            for (field, value) in &other {
                if let Err(e) = self.set_field(*id, field, value) {
                    results[idx] = Err(e);
                    break;
                }
            }
        }
        results
    }

    /// Port of `db.get_last_read_positions` -- every recorded reading
    /// position for one book/format/user triple, one per device (the
    /// in-browser EPUB reader syncs a CFI position per device so a
    /// reader who last read on their phone can resume in the desktop
    /// browser).
    pub fn get_last_read_positions(&self, book_id: i32, fmt: &str, user: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        let fmt = fmt.to_uppercase();
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT device, cfi, epoch, pos_frac FROM last_read_positions WHERE book = ?1 AND format = ?2 AND user = ?3")?;
        let rows = stmt.query_map((book_id, &fmt, user), |row| {
            Ok(serde_json::json!({
                "device": row.get::<_, String>(0)?,
                "cfi": row.get::<_, String>(1)?,
                "epoch": row.get::<_, f64>(2)?,
                "pos_frac": row.get::<_, f64>(3)?,
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Port of `db.set_last_read_position`. An empty/absent `cfi`
    /// deletes the stored position for this book/format/user/device
    /// instead of recording one, matching upstream (the reader clears
    /// its own position once a book is finished). `user`/`device`
    /// default to `"_"` when empty, matching upstream's own fallback
    /// for anonymous/unspecified callers.
    pub fn set_last_read_position(&self, book_id: i32, fmt: &str, user: &str, device: &str, cfi: Option<&str>, epoch: Option<f64>, pos_frac: f64) -> anyhow::Result<()> {
        let fmt = fmt.to_uppercase();
        let user = if user.is_empty() { "_" } else { user };
        let device = if device.is_empty() { "_" } else { device };
        let conn = self.backend.conn.lock().unwrap();
        match cfi.filter(|c| !c.is_empty()) {
            None => {
                conn.execute("DELETE FROM last_read_positions WHERE book = ?1 AND format = ?2 AND user = ?3 AND device = ?4", (book_id, &fmt, user, device))?;
            }
            Some(cfi) => {
                let epoch = epoch.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0));
                conn.execute(
                    "INSERT OR REPLACE INTO last_read_positions (book, format, user, device, cfi, epoch, pos_frac) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    (book_id, &fmt, user, device, cfi, epoch, pos_frac),
                )?;
            }
        }
        Ok(())
    }

    /// A real [`crate::fts::connection::FtsConnection`] over this
    /// library's `full-text-search.db`, same pattern as
    /// [`crate::library::Library::fts`] -- this crate's `Cache` and
    /// `Library` both wrap the same underlying [`Backend`], so this is
    /// the same connection either type would build (needed here so
    /// `lexmechanic_srv`, which only holds an `Arc<Cache>`, can reach it
    /// too).
    pub fn fts(&self) -> crate::fts::connection::FtsConnection {
        crate::fts::connection::FtsConnection::new(self.backend.conn.clone(), &self.backend.db_path)
    }

    /// A real [`crate::notes::connection::NotesConnection`] over this
    /// library's `.calnotes/notes.db`, same pattern as
    /// [`crate::library::Library::notes`] -- see [`Cache::fts`]'s doc
    /// for why this is duplicated onto `Cache` rather than routed
    /// through `Library`.
    pub fn notes(&self) -> crate::notes::connection::NotesConnection {
        crate::notes::connection::NotesConnection::new(self.backend.clone(), &self.backend.library_path)
    }

    /// Port of `Library::get_preference`/`set_preference`'s read half
    /// -- the plain string-flag `preferences` table (distinct from
    /// `Cache`'s own separate JSON-preference storage, if any).
    pub fn get_preference(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT val FROM preferences WHERE key = ?1")?;
        let val: Option<String> = stmt.query_row([key], |row| row.get(0)).optional()?;
        Ok(val)
    }

    /// Port of `Library::set_preference`.
    pub fn set_preference(&self, key: &str, val: &str) -> anyhow::Result<()> {
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("INSERT OR REPLACE INTO preferences (key, val) VALUES (?1, ?2)", (key, val))?;
        }
        // Outside the lock: appending fsyncs, and holding the connection
        // mutex across that would serialise every reader behind disk I/O
        // (#901's own warning).
        self.record(crate::change_log::ChangeOp::PrefSet { key: key.to_string(), value: Some(val.to_string()) });
        Ok(())
    }

    /// The preference name `key` had before the project was renamed
    /// (`lexmechanic.naming` was `lexmechanic.naming`), or `None` for a key that is
    /// not one of ours.
    pub fn legacy_preference_key(key: &str) -> Option<String> {
        key.strip_prefix("lexmechanic.").map(|rest| format!("oxide.{rest}"))
    }

    /// [`Cache::get_preference`] for one of our own `lexmechanic.*` keys: the
    /// value under the new name, else the one a library written before the
    /// rename still holds under the old name.
    pub fn get_preference_compat(&self, key: &str) -> anyhow::Result<Option<String>> {
        if let Some(v) = self.get_preference(key)? {
            return Ok(Some(v));
        }
        match Self::legacy_preference_key(key) {
            Some(old) => self.get_preference(&old),
            None => Ok(None),
        }
    }

    /// [`Cache::set_preference`] for one of our own keys: writes the new name
    /// and drops the old one, so the value lives in one place from here on.
    pub fn set_preference_compat(&self, key: &str, val: &str) -> anyhow::Result<()> {
        self.set_preference(key, val)?;
        self.drop_legacy_preference(key)
    }

    /// [`Cache::clear_preference`] for one of our own keys: clears both names,
    /// so an old value cannot come back through the fallback.
    pub fn clear_preference_compat(&self, key: &str) -> anyhow::Result<()> {
        self.clear_preference(key)?;
        self.drop_legacy_preference(key)
    }

    fn drop_legacy_preference(&self, key: &str) -> anyhow::Result<()> {
        if let Some(old) = Self::legacy_preference_key(key) {
            if self.get_preference(&old)?.is_some() {
                self.clear_preference(&old)?;
            }
        }
        Ok(())
    }

    /// Removes a preference, so replay can express "this was unset"
    /// rather than only "this was set to something".
    pub fn clear_preference(&self, key: &str) -> anyhow::Result<()> {
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("DELETE FROM preferences WHERE key = ?1", [key])?;
        }
        self.record(crate::change_log::ChangeOp::PrefSet { key: key.to_string(), value: None });
        Ok(())
    }

    /// Port of `is_fts_enabled` -- same as [`crate::library::Library::is_fts_enabled`].
    pub fn is_fts_enabled(&self) -> anyhow::Result<bool> {
        Ok(self.get_preference("fts.enabled")?.as_deref() == Some("true"))
    }

    // --- Saved searches (issue #422) -- port of db.cache.Cache's
    // saved_search_* methods, a thin layer over a {name: query} map
    // stored under the "saved_searches" preference key (matching
    // upstream's own `Search(self, 'saved_searches', ...)`). {{{

    const SAVED_SEARCHES_PREF: &'static str = "saved_searches";

    fn saved_search_map(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let json = self.get_preference(Self::SAVED_SEARCHES_PREF)?.unwrap_or_else(|| "{}".to_string());
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    fn set_saved_search_map(&self, map: &std::collections::HashMap<String, String>) -> anyhow::Result<()> {
        self.set_preference(Self::SAVED_SEARCHES_PREF, &serde_json::to_string(map)?)
    }

    /// Port of `saved_search_names`: every saved search's name,
    /// sorted. Narrowed to a plain case-insensitive sort rather than
    /// upstream's ICU `sort_key` -- see `lexmechanic_utils::icu`'s own
    /// doc for why real ICU collation isn't available yet (issue #459).
    pub fn saved_search_names(&self) -> anyhow::Result<Vec<String>> {
        let mut names: Vec<String> = self.saved_search_map()?.into_keys().collect();
        names.sort_by_key(|n| n.to_lowercase());
        Ok(names)
    }

    /// Port of `saved_search_lookup`: case-insensitive name match.
    pub fn saved_search_lookup(&self, name: &str) -> anyhow::Result<Option<String>> {
        let name_lower = name.to_lowercase();
        Ok(self.saved_search_map()?.into_iter().find(|(n, _)| n.to_lowercase() == name_lower).map(|(_, v)| v))
    }

    /// Port of `saved_search_add`.
    pub fn saved_search_add(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let mut map = self.saved_search_map()?;
        map.insert(name.to_string(), value.trim().to_string());
        self.set_saved_search_map(&map)
    }

    /// Port of `saved_search_delete`.
    pub fn saved_search_delete(&self, name: &str) -> anyhow::Result<()> {
        let mut map = self.saved_search_map()?;
        map.remove(name);
        self.set_saved_search_map(&map)
    }

    /// Port of `saved_search_rename`. Narrowed to a no-op when
    /// `old_name` doesn't exist -- upstream's own
    /// `queries[new] = queries.get(old, None)` would store a JSON
    /// `null` under `new_name` in that case, which no real caller
    /// relies on and this map's `HashMap<String, String>` can't
    /// represent anyway.
    pub fn saved_search_rename(&self, old_name: &str, new_name: &str) -> anyhow::Result<()> {
        let mut map = self.saved_search_map()?;
        if let Some(v) = map.remove(old_name) {
            map.insert(new_name.to_string(), v);
            self.set_saved_search_map(&map)?;
        }
        Ok(())
    }

    /// Port of `saved_search_set_all`: replaces the entire map.
    pub fn saved_search_set_all(&self, map: std::collections::HashMap<String, String>) -> anyhow::Result<()> {
        self.set_saved_search_map(&map)
    }
    // }}}

    // --- Virtual libraries (issue #500, part of the #432 browser-UI
    // epic) -- upstream's own `books_in_virtual_library` resolves a
    // named virtual library through `db._pref('virtual_libraries', {})`,
    // the *exact* same `{name: query}`-map-under-a-preference-key
    // shape as saved searches (`db.cache.py:1605-1622`) -- just a
    // second, separate preference key, not a distinct subsystem. This
    // mirrors `saved_search_*` above 1:1, plus a public
    // [`Cache::virtual_library_map`] (saved searches never needed a
    // public "give me the whole map" accessor; the library browser
    // does, to list every virtual library by name). {{{

    const VIRTUAL_LIBRARIES_PREF: &'static str = "virtual_libraries";

    fn virtual_library_map_raw(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let json = self.get_preference(Self::VIRTUAL_LIBRARIES_PREF)?.unwrap_or_else(|| "{}".to_string());
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    fn set_virtual_library_map(&self, map: &std::collections::HashMap<String, String>) -> anyhow::Result<()> {
        self.set_preference(Self::VIRTUAL_LIBRARIES_PREF, &serde_json::to_string(map)?)
    }

    /// The whole `{name: query}` map, unsorted -- for a caller (the
    /// library browser) that wants to list every virtual library by
    /// name, not just check for/resolve one.
    pub fn virtual_library_map(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        self.virtual_library_map_raw()
    }

    /// Port of `virtual_libraries` (the read-only name-listing half --
    /// upstream's own property of the same name): every virtual
    /// library's name, sorted. Same case-insensitive-sort narrowing as
    /// [`Cache::saved_search_names`] (no real ICU collation yet).
    pub fn virtual_library_names(&self) -> anyhow::Result<Vec<String>> {
        let mut names: Vec<String> = self.virtual_library_map_raw()?.into_keys().collect();
        names.sort_by_key(|n| n.to_lowercase());
        Ok(names)
    }

    /// Port of the lookup half of `books_in_virtual_library`'s own
    /// `db._pref('virtual_libraries', {}).get(vl)` -- case-insensitive
    /// name match, matching [`Cache::saved_search_lookup`].
    pub fn virtual_library_lookup(&self, name: &str) -> anyhow::Result<Option<String>> {
        let name_lower = name.to_lowercase();
        Ok(self.virtual_library_map_raw()?.into_iter().find(|(n, _)| n.to_lowercase() == name_lower).map(|(_, v)| v))
    }

    pub fn virtual_library_add(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let mut map = self.virtual_library_map_raw()?;
        map.insert(name.to_string(), value.trim().to_string());
        self.set_virtual_library_map(&map)
    }

    pub fn virtual_library_delete(&self, name: &str) -> anyhow::Result<()> {
        let mut map = self.virtual_library_map_raw()?;
        map.remove(name);
        self.set_virtual_library_map(&map)
    }

    pub fn virtual_library_set_all(&self, map: std::collections::HashMap<String, String>) -> anyhow::Result<()> {
        self.set_virtual_library_map(&map)
    }
    // }}}

    /// Port of `enable_fts` -- same as [`crate::library::Library::set_fts_enabled`].
    pub fn set_fts_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        self.set_preference("fts.enabled", if enabled { "true" } else { "false" })?;
        if enabled {
            self.fts().initialize()?;
            self.fts().dirty_existing()?;
        }
        Ok(())
    }

    /// Port of `fts_indexing_progress`'s `(left, total)` -- same as
    /// [`crate::library::Library::fts_indexing_progress`].
    pub fn fts_indexing_progress(&self) -> anyhow::Result<(i64, i64)> {
        let fts = self.fts();
        fts.initialize()?;
        let left = fts.number_dirtied()?;
        let indexed = fts.number_indexed()?;
        Ok((left, left + indexed))
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

/// Trims each name, drops empty ones and drops repeats, keeping the first
/// occurrence. "Repeat" is case-insensitive, matching the NOCASE collation of
/// the `authors`/`tags`/... name columns, so the result can be linked to a
/// book without tripping the `UNIQUE(book, <item>)` constraint of its link
/// table. The one shared helper for every place that links a list of names.
pub(crate) fn dedupe_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if !name.is_empty() && seen.insert(name.to_lowercase()) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    fn insert_book(cache: &Cache, title: &str) -> i32 {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES (?1)", [title])
            .unwrap();
        conn.last_insert_rowid() as i32
    }

    fn author_id(cache: &Cache, name: &str) -> i32 {
        let conn = cache.backend.conn.lock().unwrap();
        conn.query_row("SELECT id FROM authors WHERE name = ?1", [name], |row| row.get(0)).unwrap()
    }

    #[test]
    fn rename_author_renames_when_the_new_name_is_not_taken() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "authors", "Old Name").unwrap();
        let old_id = author_id(&cache, "Old Name");

        cache.rename_author(old_id, "New Name").unwrap();

        assert_eq!(cache.field_for(id, "authors").unwrap(), Some("New Name".to_string()));
        assert_eq!(author_id(&cache, "New Name"), old_id, "same row, just renamed -- not a new author");
    }

    #[test]
    fn rename_author_merges_into_an_existing_author_on_collision() {
        // Real merge behavior: renaming "Old Name" to an author that
        // already exists re-points every book linked to "Old Name"
        // onto the existing author and drops the now-orphaned row,
        // rather than violating the `name` column's UNIQUE constraint.
        let (_dir, cache) = open_test_cache();
        let book_a = insert_book(&cache, "A");
        let book_b = insert_book(&cache, "B");
        cache.set_field(book_a, "authors", "Alice").unwrap();
        cache.set_field(book_b, "authors", "Bob").unwrap();
        let alice_id = author_id(&cache, "Alice");
        let bob_id = author_id(&cache, "Bob");

        cache.rename_author(alice_id, "Bob").unwrap();

        assert_eq!(cache.field_for(book_a, "authors").unwrap(), Some("Bob".to_string()), "book_a's author link should now point at the surviving Bob row");
        assert_eq!(cache.field_for(book_b, "authors").unwrap(), Some("Bob".to_string()));
        assert_eq!(author_id(&cache, "Bob"), bob_id, "Bob's own id should be unchanged (Alice's row was the one dropped)");
        let conn = cache.backend.conn.lock().unwrap();
        let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM authors WHERE id = ?1", [alice_id], |row| row.get(0)).unwrap();
        assert_eq!(remaining, 0, "the orphaned Alice row should really be gone");
    }

    #[test]
    fn rename_tag_and_rename_publisher_work_the_same_way() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "tags", "scifi").unwrap();
        cache.set_field(id, "publisher", "Old Press").unwrap();
        let tag_id = {
            let conn = cache.backend.conn.lock().unwrap();
            conn.query_row("SELECT id FROM tags WHERE name = ?1", ["scifi"], |row| row.get::<_, i32>(0)).unwrap()
        };
        let publisher_id = {
            let conn = cache.backend.conn.lock().unwrap();
            conn.query_row("SELECT id FROM publishers WHERE name = ?1", ["Old Press"], |row| row.get::<_, i32>(0)).unwrap()
        };

        cache.rename_tag(tag_id, "classic-scifi").unwrap();
        cache.rename_publisher(publisher_id, "New Press").unwrap();

        assert_eq!(cache.field_for(id, "tags").unwrap(), Some("classic-scifi".to_string()));
        assert_eq!(cache.field_for(id, "publisher").unwrap(), Some("New Press".to_string()));
    }

    #[test]
    fn saved_search_add_lookup_and_names_round_trip() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.saved_search_names().unwrap(), Vec::<String>::new());

        cache.saved_search_add("Recent SF", "tags:scifi and date:>30daysago").unwrap();
        cache.saved_search_add("Anthologies", "tags:anthology").unwrap();

        assert_eq!(cache.saved_search_names().unwrap(), vec!["Anthologies".to_string(), "Recent SF".to_string()]);
        assert_eq!(cache.saved_search_lookup("Recent SF").unwrap().as_deref(), Some("tags:scifi and date:>30daysago"));
        // Case-insensitive, matching upstream's own lookup().
        assert_eq!(cache.saved_search_lookup("recent sf").unwrap().as_deref(), Some("tags:scifi and date:>30daysago"));
        assert_eq!(cache.saved_search_lookup("nonexistent").unwrap(), None);
    }

    #[test]
    fn saved_search_delete_removes_it() {
        let (_dir, cache) = open_test_cache();
        cache.saved_search_add("temp", "tags:x").unwrap();
        assert!(cache.saved_search_lookup("temp").unwrap().is_some());
        cache.saved_search_delete("temp").unwrap();
        assert_eq!(cache.saved_search_lookup("temp").unwrap(), None);
    }

    #[test]
    fn saved_search_rename_moves_the_query_to_the_new_name() {
        let (_dir, cache) = open_test_cache();
        cache.saved_search_add("old", "tags:x").unwrap();
        cache.saved_search_rename("old", "new").unwrap();
        assert_eq!(cache.saved_search_lookup("old").unwrap(), None);
        assert_eq!(cache.saved_search_lookup("new").unwrap().as_deref(), Some("tags:x"));
    }

    #[test]
    fn saved_search_rename_of_a_nonexistent_name_is_a_no_op() {
        let (_dir, cache) = open_test_cache();
        cache.saved_search_rename("nonexistent", "new").unwrap();
        assert_eq!(cache.saved_search_names().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn saved_search_set_all_replaces_the_whole_map() {
        let (_dir, cache) = open_test_cache();
        cache.saved_search_add("keep-me-out", "tags:x").unwrap();
        let mut replacement = std::collections::HashMap::new();
        replacement.insert("only".to_string(), "tags:y".to_string());
        cache.saved_search_set_all(replacement).unwrap();
        assert_eq!(cache.saved_search_names().unwrap(), vec!["only".to_string()]);
    }

    #[test]
    fn virtual_library_add_lookup_and_names_round_trip() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.virtual_library_names().unwrap(), Vec::<String>::new());

        cache.virtual_library_add("Unread", "not read:true").unwrap();
        cache.virtual_library_add("Fiction", "tags:fiction").unwrap();

        assert_eq!(cache.virtual_library_names().unwrap(), vec!["Fiction".to_string(), "Unread".to_string()]);
        assert_eq!(cache.virtual_library_lookup("Unread").unwrap().as_deref(), Some("not read:true"));
        assert_eq!(cache.virtual_library_lookup("unread").unwrap().as_deref(), Some("not read:true"), "lookup should be case-insensitive, matching saved searches");
        assert_eq!(cache.virtual_library_lookup("nonexistent").unwrap(), None);
    }

    #[test]
    fn virtual_library_map_lists_every_entry() {
        let (_dir, cache) = open_test_cache();
        cache.virtual_library_add("Unread", "not read:true").unwrap();
        cache.virtual_library_add("Fiction", "tags:fiction").unwrap();
        let map = cache.virtual_library_map().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("Unread").map(String::as_str), Some("not read:true"));
    }

    #[test]
    fn virtual_library_delete_removes_it() {
        let (_dir, cache) = open_test_cache();
        cache.virtual_library_add("temp", "tags:x").unwrap();
        assert!(cache.virtual_library_lookup("temp").unwrap().is_some());
        cache.virtual_library_delete("temp").unwrap();
        assert_eq!(cache.virtual_library_lookup("temp").unwrap(), None);
    }

    #[test]
    fn virtual_library_set_all_replaces_the_whole_map() {
        let (_dir, cache) = open_test_cache();
        cache.virtual_library_add("keep-me-out", "tags:x").unwrap();
        let mut replacement = std::collections::HashMap::new();
        replacement.insert("only".to_string(), "tags:y".to_string());
        cache.virtual_library_set_all(replacement).unwrap();
        assert_eq!(cache.virtual_library_names().unwrap(), vec!["only".to_string()]);
    }

    #[test]
    fn virtual_libraries_and_saved_searches_are_stored_under_separate_preference_keys() {
        let (_dir, cache) = open_test_cache();
        cache.saved_search_add("same-name", "tags:from-saved-search").unwrap();
        cache.virtual_library_add("same-name", "tags:from-virtual-library").unwrap();
        assert_eq!(cache.saved_search_lookup("same-name").unwrap().as_deref(), Some("tags:from-saved-search"));
        assert_eq!(cache.virtual_library_lookup("same-name").unwrap().as_deref(), Some("tags:from-virtual-library"));
    }

    #[test]
    fn all_book_ids_returns_every_book_in_id_order() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.all_book_ids().unwrap(), Vec::<i32>::new());
        let id1 = insert_book(&cache, "One");
        let id2 = insert_book(&cache, "Two");
        assert_eq!(cache.all_book_ids().unwrap(), vec![id1, id2]);
    }

    fn add_pdf_book(dir: &Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.pdf"));
        fs::write(&source, title.as_bytes()).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["An Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    #[test]
    fn set_fields_for_books_applies_to_every_book_in_one_log_entry_and_fails_a_bad_id_alone() {
        let (dir, cache) = open_test_cache();
        let a = add_pdf_book(dir.path(), &cache, "A");
        let b = add_pdf_book(dir.path(), &cache, "B");
        let log = cache.backend.change_log().unwrap();
        let entries_before = log.entry_count();

        let changes = vec![("tags".to_string(), "red, blue".to_string()), ("publisher".to_string(), "Acme".to_string()), ("rating".to_string(), "8".to_string())];
        let results = cache.set_fields_for_books(&[a, 9999, b], &changes);
        assert!(results[0].is_ok() && results[2].is_ok());
        assert!(results[1].is_err(), "an unknown book fails on its own");
        for id in [a, b] {
            assert_eq!(cache.field_for(id, "tags").unwrap().as_deref(), Some("red, blue"));
            assert_eq!(cache.field_for(id, "publisher").unwrap().as_deref(), Some("Acme"));
            assert_eq!(cache.field_for(id, "rating").unwrap().as_deref(), Some("8"));
        }
        assert_eq!(log.entry_count(), entries_before + 1, "one batch entry for every book and field");
    }

    #[test]
    fn set_fields_for_books_sends_custom_columns_through_the_per_book_path() {
        let (dir, cache) = open_test_cache();
        let a = add_pdf_book(dir.path(), &cache, "A");
        cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();
        let results = cache.set_fields_for_books(&[a], &[("#shelf".to_string(), "top".to_string()), ("series".to_string(), "Saga".to_string())]);
        assert!(results[0].is_ok(), "{:?}", results[0]);
        assert_eq!(cache.field_for(a, "series").unwrap().as_deref(), Some("Saga"));
        assert_eq!(cache.get_custom_column_value(a, "#shelf").unwrap().as_deref(), Some("top"));
    }

    #[test]
    fn field_for_reads_scalar_book_columns() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "My Title");
        assert_eq!(
            cache.field_for(id, "title").unwrap(),
            Some("My Title".to_string())
        );
    }

    #[test]
    fn field_for_reads_the_integer_id_column_without_a_type_mismatch() {
        // Regression test: `id` is INTEGER, not TEXT like most of the
        // `books` columns `field_for` handles in the same match arm --
        // fetching it via `row.get::<_, String>(0)` (rusqlite's
        // `FromSql` for `String` only accepts SQLite TEXT) used to
        // return an `InvalidColumnType` error that `view.rs::sort`'s
        // `.ok().flatten()` silently swallowed into "no value",
        // breaking `sort("id", ...)` without ever surfacing an error.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "id").unwrap(), Some(id.to_string()));
    }

    #[test]
    fn field_for_reads_comments_from_the_comments_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "comments").unwrap(), None);
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO comments (book, text) VALUES (?1, ?2)",
                (id, "Some comment text"),
            )
            .unwrap();
        }
        assert_eq!(
            cache.field_for(id, "comments").unwrap(),
            Some("Some comment text".to_string())
        );
    }

    #[test]
    fn field_for_reads_authors_joined_with_ampersand_in_link_order() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO authors (name) VALUES ('Bob')", [])
            .unwrap();
        let bob_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO authors (name) VALUES ('Alice')", [])
            .unwrap();
        let alice_id = conn.last_insert_rowid();
        // Link Bob first, then Alice -- field order must follow link
        // (insertion) order, not alphabetical.
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (id, bob_id),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (id, alice_id),
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "authors").unwrap(),
            Some("Bob & Alice".to_string())
        );
    }

    #[test]
    fn field_for_reads_tags_joined_with_comma() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "tags").unwrap(), None);
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO tags (name) VALUES ('fiction')", [])
            .unwrap();
        let t1 = conn.last_insert_rowid();
        conn.execute("INSERT INTO tags (name) VALUES ('drama')", [])
            .unwrap();
        let t2 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
            (id, t1),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
            (id, t2),
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "tags").unwrap(),
            Some("fiction, drama".to_string())
        );
    }

    #[test]
    fn field_for_reads_series_and_series_index() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO series (name) VALUES ('The Trilogy')", [])
            .unwrap();
        let series_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO books_series_link (book, series) VALUES (?1, ?2)",
            (id, series_id),
        )
        .unwrap();
        conn.execute("UPDATE books SET series_index = 2.0 WHERE id = ?1", [id])
            .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "series").unwrap(),
            Some("The Trilogy".to_string())
        );
        assert_eq!(
            cache.field_for(id, "series_index").unwrap(),
            Some("2".to_string())
        );
    }

    #[test]
    fn field_for_reads_identifiers_as_type_val_pairs() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO identifiers (book, type, val) VALUES (?1, 'isbn', '12345')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO identifiers (book, type, val) VALUES (?1, 'doi', 'abc')",
            [id],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "identifiers").unwrap(),
            Some("isbn:12345,doi:abc".to_string())
        );
    }

    #[test]
    fn field_for_reads_formats_from_the_data_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 100, 'book')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'MOBI', 200, 'book')",
            [id],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            cache.field_for(id, "formats").unwrap(),
            Some("EPUB, MOBI".to_string())
        );
        assert_eq!(
            cache.field_for(id, "size").unwrap(),
            Some("200".to_string())
        );
    }

    #[test]
    fn pref_and_set_pref_round_trip_including_namespaced_keys() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.pref("k", None), None);
        cache.set_pref("k", &serde_json::json!("v"), None).unwrap();
        assert_eq!(cache.pref("k", None), Some(serde_json::json!("v")));

        assert_eq!(cache.pref("k", Some("ns")), None);
        cache
            .set_pref("k", &serde_json::json!(42), Some("ns"))
            .unwrap();
        assert_eq!(cache.pref("k", Some("ns")), Some(serde_json::json!(42)));
        // Namespaced and un-namespaced keys of the same name are distinct.
        assert_eq!(cache.pref("k", None), Some(serde_json::json!("v")));
    }

    #[test]
    fn field_for_returns_none_for_an_unrecognized_field_without_deadlocking() {
        // Regression test: the fallback arm used to call
        // `self.backend.field_for(...)`, which tries to lock
        // `self.backend.conn` a second time -- but this function
        // already holds that lock for its whole body, and
        // `std::sync::Mutex` isn't reentrant, so that was a guaranteed
        // self-deadlock on any name not handled by an earlier arm
        // (which is every name, since `Backend::field_for`'s whitelist
        // is a strict subset of this function's). If this test hangs
        // instead of returning, the deadlock is back.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert_eq!(cache.field_for(id, "not_a_real_field").unwrap(), None);
    }

    #[test]
    fn custom_column_round_trips_a_text_value() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache
            .add_custom_column("mycol", "My Column", "text", false)
            .unwrap();
        assert_eq!(cache.get_custom_column_value(id, "mycol").unwrap(), None);
        cache.set_custom_column_value(id, "mycol", "hello").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "mycol").unwrap(),
            Some("hello".to_string())
        );
    }

    #[test]
    fn custom_column_round_trips_numeric_datatypes() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.add_custom_column("n", "N", "int", false).unwrap();
        cache.set_custom_column_value(id, "n", "42").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "n").unwrap(),
            Some("42".to_string())
        );

        cache.add_custom_column("f", "F", "float", false).unwrap();
        cache.set_custom_column_value(id, "f", "3.5").unwrap();
        assert_eq!(
            cache.get_custom_column_value(id, "f").unwrap(),
            Some("3.5".to_string())
        );
    }

    #[test]
    fn set_field_dispatches_an_unrecognized_name_to_a_real_custom_column() {
        // Regression test for issue #720: `set_field`'s fallback used
        // to unconditionally `bail!` for any name it didn't
        // special-case, which meant a real custom column's value
        // could never be set through this generic entry point (the
        // one `lexmechanic_srv::cdb::set_fields` actually calls) even
        // though `Cache::set_custom_column_value` itself worked fine
        // called directly.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.add_custom_column("mycol", "My Column", "text", false).unwrap();
        cache.set_field(id, "mycol", "hello").unwrap();
        assert_eq!(cache.get_custom_column_value(id, "mycol").unwrap(), Some("hello".to_string()));
    }

    #[test]
    fn set_field_still_rejects_a_name_that_is_neither_builtin_nor_a_real_custom_column() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert!(cache.set_field(id, "not_a_real_field_or_column", "x").is_err());
    }

    #[test]
    fn add_custom_column_rejects_a_duplicate_label() {
        let (_dir, cache) = open_test_cache();
        cache
            .add_custom_column("dup", "Dup", "text", false)
            .unwrap();
        assert!(cache
            .add_custom_column("dup", "Dup 2", "text", false)
            .is_err());
    }

    #[test]
    fn set_custom_column_value_errors_for_an_unknown_label() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert!(cache.set_custom_column_value(id, "nope", "x").is_err());
    }

    #[test]
    fn remove_custom_column_drops_the_column_and_its_value_table() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache
            .add_custom_column("temp", "Temp", "text", false)
            .unwrap();
        cache.set_custom_column_value(id, "temp", "x").unwrap();

        cache.remove_custom_column("temp").unwrap();

        assert!(cache.get_custom_column_value(id, "temp").unwrap().is_none());
        assert!(!cache
            .custom_column_label_map()
            .unwrap()
            .contains_key("temp"));
        // Re-adding the same label should work again now that it's gone.
        assert!(cache
            .add_custom_column("temp", "Temp", "text", false)
            .is_ok());
    }

    #[test]
    fn custom_column_label_map_reports_metadata_for_every_column() {
        let (_dir, cache) = open_test_cache();
        cache
            .add_custom_column("mycol", "My Column", "int", true)
            .unwrap();
        cache.add_custom_column("tags2", "Tags 2", "text", true).unwrap();
        let map = cache.custom_column_label_map().unwrap();
        let entry = map.get("mycol").unwrap();
        assert_eq!(entry["name"], "My Column");
        assert_eq!(entry["datatype"], "int");
        // calibre keeps is_multiple only for text and composite columns.
        assert_eq!(entry["is_multiple"], false);
        assert_eq!(entry["normalized"], false);
        assert_eq!(map["tags2"]["is_multiple"], true);
        assert_eq!(map["tags2"]["normalized"], true);
    }

    #[test]
    fn all_tags_returns_every_row_trimmed_with_blanks_dropped() {
        let (_dir, cache) = open_test_cache();
        let conn = cache.backend.conn.lock().unwrap();
        for name in ["Fiction", " Non-Fiction ", "  "] {
            conn.execute("INSERT INTO tags (name) VALUES (?1)", [name]).unwrap();
        }
        drop(conn);

        let mut tags = cache.all_tags().unwrap();
        tags.sort();
        assert_eq!(tags, vec!["Fiction".to_string(), "Non-Fiction".to_string()]);
    }

    // --- filesystem book/format/cover management ---

    fn write_temp_file(dir: &Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    /// Counts real journaled entries under `library_path` whose op tag
    /// matches `tag` (`"DeleteFile"`/`"WriteFile"`/`"RenameFile"`) --
    /// `OperationDescriptor`/`JournalEntry` are private to
    /// `library_handle.rs`, so this checks for the serde tag as a
    /// substring rather than deserializing the real type.
    fn journaled_op_count(library_path: &Path, tag: &str) -> usize {
        let journal_dir = library_path.join(".lexmechanic").join("journal");
        // A missing journal directory means zero journaled operations,
        // not a broken fixture. It is absent whenever nothing has needed
        // a durable write yet -- which since #893 includes adding a book
        // whose file is already where it is going, since there is no copy
        // to journal.
        fs::read_dir(&journal_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .filter(|e| {
                fs::read_to_string(e.path())
                    .map(|content| content.contains(tag))
                    .unwrap_or(false)
            })
            .count()
    }

    fn journaled_delete_count(library_path: &Path) -> usize {
        journaled_op_count(library_path, "DeleteFile")
    }

    #[test]
    fn add_book_stores_the_cover_the_format_carried() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");

        let mut meta = MetaInformation::default();
        meta.title = "Covered".to_string();
        meta.authors = vec!["An Author".to_string()];
        meta.cover_data = (Some("jpg".to_string()), b"pretend jpeg bytes".to_vec());

        let book_id = cache.add_book(&source, &meta).unwrap();

        // Both halves matter: the file on disk is what gets served,
        // and `has_cover` is what the grid consults before asking for
        // it. Before this, neither happened for any format.
        assert!(cache.has_cover(book_id).unwrap(), "has_cover was not set");
        let cover = crate::covers::cover_path(&cache, book_id).unwrap();
        assert_eq!(fs::read(&cover).unwrap(), b"pretend jpeg bytes");
    }

    #[test]
    fn add_book_without_a_cover_leaves_has_cover_off() {
        let (_dir, cache) = open_test_cache();
        let source = write_temp_file(_dir.path(), "src.epub", b"epub bytes");

        let mut meta = MetaInformation::default();
        meta.title = "Bare".to_string();
        meta.authors = vec!["An Author".to_string()];

        let book_id = cache.add_book(&source, &meta).unwrap();
        assert!(!cache.has_cover(book_id).unwrap());
        assert!(!crate::covers::cover_path(&cache, book_id).unwrap().exists());
    }

    #[test]
    fn add_book_places_the_file_in_the_library_root() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");

        let mut meta = MetaInformation::default();
        meta.title = "My Title".to_string();
        meta.authors = vec!["My Author".to_string()];

        let book_id = cache.add_book(&source, &meta).unwrap();

        // The library root (#893): `add_book` brings a file in, and "in"
        // is the folder itself rather than a structure imposed on it.
        assert_eq!(
            cache.field_for(book_id, "path").unwrap(),
            Some(String::new())
        );
        // `src.epub`, not `My Title.epub`: the folder is still derived
        // from the metadata, the filename no longer is (#893).
        let dest = dir.path().join("src.epub");
        assert_eq!(fs::read(dest).unwrap(), b"epub bytes");
    }

    #[test]
    fn add_book_db_entry_preserves_a_caller_supplied_uuid_past_the_insert_trigger() {
        let (_dir, cache) = open_test_cache();
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        meta.uuid = Some("caller-uuid-123".to_string());

        let book_id = cache.add_book_db_entry(&meta, "A/T").unwrap();

        assert_eq!(
            cache.field_for(book_id, "uuid").unwrap(),
            Some("caller-uuid-123".to_string())
        );
    }

    #[test]
    fn add_format_records_the_format_in_the_data_table() {
        let (dir, cache) = open_test_cache();
        // Deliberately from *outside* the library: this test asserts a
        // journaled copy, and adding a file that is already at its
        // destination performs no copy to journal (#893).
        let outside = tempfile::tempdir().unwrap();
        let source = write_temp_file(outside.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache.add_format(book_id, &source, "epub", true).unwrap();

        // This is the real bug fix: `formats`/`size` read from the
        // `data` table via `field_for`, which `add_format` never
        // populated before this pass.
        assert_eq!(
            cache.field_for(book_id, "formats").unwrap(),
            Some("EPUB".to_string())
        );
        assert_eq!(
            cache.field_for(book_id, "size").unwrap(),
            Some(b"epub bytes".len().to_string())
        );

        // add_format now goes through the real LibraryHandle (issue
        // #93's crate-wide write-path retrofit) via `copy_atomic`, not
        // a raw `fs::copy` -- prove it by checking real journaled
        // `WriteFile` entries landed: one from `add_book`'s own
        // initial `add_format` call above, one from this explicit
        // `add_format` call.
        assert_eq!(journaled_op_count(dir.path(), "WriteFile"), 2);
    }

    #[test]
    fn add_format_does_not_replace_an_existing_file_when_replace_is_false() {
        let (dir, cache) = open_test_cache();
        // `add_book` itself already adds the source file as its first
        // format (via `add_format(..., replace=true)` internally).
        let source = write_temp_file(dir.path(), "src.epub", b"first");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        let source2 = write_temp_file(dir.path(), "src2.epub", b"second");
        // `replace` is about the book's *format*, not a filename: the
        // book already has an EPUB, so a second one is refused even
        // though it would land under a different name.
        assert!(!cache.add_format(book_id, &source2, "epub", false).unwrap());

        let first_dest = dir.path().join("src.epub");
        assert_eq!(fs::read(&first_dest).unwrap(), b"first");

        // With `replace`, the new file lands under its own name and the
        // one it replaced is removed rather than orphaned.
        assert!(cache.add_format(book_id, &source2, "epub", true).unwrap());
        assert_eq!(fs::read(dir.path().join("src2.epub")).unwrap(), b"second");
        assert!(!first_dest.exists(), "the replaced file was left behind");
    }

    #[test]
    fn add_format_sanitizes_a_path_traversal_extension() {
        // `format` can come from untrusted input (e.g. an HTTP request
        // body -- see `lexmechanic_srv::cdb::set_fields`'s `added_formats`);
        // it must not be able to embed a path separator and write
        // outside the book's own directory.
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.bin", b"payload");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        let evil_ext = "../../../../../../../../tmp/cache-add-format-traversal-poc";
        cache.add_format(book_id, &source, evil_ext, true).unwrap();

        assert!(!std::path::Path::new("/tmp/cache-add-format-traversal-poc").exists(), "path traversal payload escaped the book's own directory");
    }

    #[test]
    fn remove_format_moves_the_file_to_the_library_trash_and_drops_its_data_row() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        cache.add_format(book_id, &source, "epub", true).unwrap();
        let renames_before = journaled_op_count(dir.path(), "RenameFile");

        cache.remove_format(book_id, "epub").unwrap();

        // Gone from the folder, in the trash (calibre's `f/<id>/<ext>`), and
        // restorable -- not erased.
        assert!(!dir.path().join("A/T/T.epub").exists());
        assert_eq!(cache.field_for(book_id, "formats").unwrap(), None);
        assert_eq!(fs::read(cache.trash_dir().join("f").join(book_id.to_string()).join("epub")).unwrap(), b"epub bytes");
        // Through the real LibraryHandle: a journaled rename, not a delete.
        assert_eq!(journaled_op_count(dir.path(), "RenameFile"), renames_before + 1);
        assert_eq!(journaled_delete_count(dir.path()), 0);
    }

    #[test]
    fn remove_format_permanently_deletes_the_file_and_its_data_row() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        cache.add_format(book_id, &source, "epub", true).unwrap();

        cache.remove_format_permanently(book_id, "epub").unwrap();

        assert!(!dir.path().join("A/T/T.epub").exists());
        assert_eq!(cache.field_for(book_id, "formats").unwrap(), None);
        assert!(cache.list_trash().unwrap().is_empty(), "nothing is kept");

        // remove_format now goes through the real LibraryHandle (issue
        // #93's crate-wide write-path retrofit), not a raw
        // `fs::remove_file` -- prove it by checking a real journaled
        // `DeleteFile` entry landed (not just any entry: `add_book`/
        // `add_format` above also journal their own `WriteFile`
        // entries via `copy_atomic`, so this specifically looks for
        // the delete rather than asserting a total count).
        assert_eq!(journaled_delete_count(dir.path()), 1);
    }

    #[test]
    fn set_last_read_position_records_and_deletes_a_position() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache.set_last_read_position(book_id, "epub", "alice", "phone", Some("/6/4[chap01]"), Some(1000.0), 0.5).unwrap();
        let positions = cache.get_last_read_positions(book_id, "epub", "alice").unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0]["device"], "phone");
        assert_eq!(positions[0]["cfi"], "/6/4[chap01]");
        assert_eq!(positions[0]["pos_frac"], 0.5);

        // A second device gets its own row, not an overwrite.
        cache.set_last_read_position(book_id, "epub", "alice", "desktop", Some("/6/8[chap02]"), Some(2000.0), 0.75).unwrap();
        assert_eq!(cache.get_last_read_positions(book_id, "epub", "alice").unwrap().len(), 2);

        // A different user's positions are isolated.
        assert!(cache.get_last_read_positions(book_id, "epub", "bob").unwrap().is_empty());

        // An empty cfi clears that device's position, matching upstream.
        cache.set_last_read_position(book_id, "epub", "alice", "phone", None, None, 0.0).unwrap();
        let positions = cache.get_last_read_positions(book_id, "epub", "alice").unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0]["device"], "desktop");
    }

    #[test]
    fn set_last_read_position_defaults_empty_user_and_device_to_underscore() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache.set_last_read_position(book_id, "epub", "", "", Some("/2"), Some(1.0), 0.1).unwrap();
        let positions = cache.get_last_read_positions(book_id, "epub", "_").unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0]["device"], "_");
    }

    #[test]
    fn fts_enable_disable_and_search_round_trip_through_cache() {
        let (_dir, cache) = open_test_cache();
        assert!(!cache.is_fts_enabled().unwrap());

        cache.set_fts_enabled(true).unwrap();
        assert!(cache.is_fts_enabled().unwrap());

        cache.fts().add_text(1, "EPUB", 0.0, Some("This is a book about Rust programming."), "", 0, "", None).unwrap();
        cache.fts().add_text(2, "MOBI", 0.0, Some("Python is a fine language too."), "", 0, "", None).unwrap();

        let results = cache.fts().search("Rust", false, None, None, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].book_id, 1);

        cache.set_fts_enabled(false).unwrap();
        assert!(!cache.is_fts_enabled().unwrap());
    }

    #[test]
    fn fts_indexing_progress_reports_dirty_and_indexed_counts() {
        let (_dir, cache) = open_test_cache();
        cache.set_fts_enabled(true).unwrap();
        // `set_fts_enabled(true)` dirties every existing format (none
        // yet in a fresh library), so start from a known state.
        let (left_before, total_before) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left_before, 0);
        assert_eq!(total_before, 0);

        cache.fts().dirty_book(1, &["EPUB"]).unwrap();
        let (left, total) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left, 1);
        assert_eq!(total, 1);

        cache.fts().add_text(1, "EPUB", 0.0, Some("indexed now"), "", 0, "", None).unwrap();
        let (left, total) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left, 0);
        assert_eq!(total, 1);
    }

    #[test]
    fn notes_shares_the_same_connection_cache_would_get_via_library() {
        let (_dir, cache) = open_test_cache();
        cache.notes().initialize().unwrap();
        let hash = std::collections::HashSet::new();
        cache.notes().set_note("authors", 1, "Jane Doe", "<p>A note</p>", &hash).unwrap();
        assert_eq!(cache.notes().get_note("authors", 1).unwrap(), Some("<p>A note</p>".to_string()));
    }

    #[test]
    fn add_book_records_a_real_blake3_checksum_for_the_initial_format() {
        // Port of docs/FAULT_TOLERANCE.md §8: "every book file's
        // BLAKE3 is stored... at add time". `add_book` delegates its
        // initial format to `add_format`, so this exercises the same
        // recording path `add_format_records_the_format_in_the_data_table`
        // covers for the `data` row.
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        let dest = dir.path().join("src.epub");
        assert_eq!(
            cache
                .checksums()
                .verify_file(book_id, "format", "EPUB", &dest)
                .unwrap(),
            crate::checksums::VerifyOutcome::Match
        );

        // Corrupt the file on disk directly, bypassing every write
        // path -- the stored checksum must catch it.
        fs::write(&dest, b"corrupted!").unwrap();
        assert!(matches!(
            cache
                .checksums()
                .verify_file(book_id, "format", "EPUB", &dest),
            Err(crate::checksums::ChecksumError::Mismatch { .. })
        ));
    }

    #[test]
    fn remove_format_deletes_its_checksum_record_too() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache.remove_format(book_id, "epub").unwrap();

        assert_eq!(
            cache
                .checksums()
                .verify_bytes(book_id, "format", "EPUB", b"epub bytes")
                .unwrap(),
            crate::checksums::VerifyOutcome::NoRecord
        );
    }

    #[test]
    fn delete_book_removes_the_row_and_the_file() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        assert!(dir.path().join("src.epub").exists());

        cache.delete_book(book_id).unwrap();

        assert_eq!(cache.field_for(book_id, "title").unwrap(), None);
        // The book's own file goes to the wastebasket -- by name, not by
        // removing a folder (a root-level book has none; its "folder"
        // is the library itself).
        assert!(!dir.path().join("src.epub").exists());
        assert_eq!(journaled_delete_count(dir.path()), 0);
    }

    /// A dedicated per-book folder is emptied (its own files only) and
    /// then removed, along with the now-empty author folder.
    #[test]
    fn delete_book_empties_and_removes_a_dedicated_folder() {
        let (dir, cache) = open_test_cache();
        let book_id = add_pdf_in_its_own_folder(dir.path(), &cache, "T", "A", "src");
        fs::write(dir.path().join("A/T/cover.jpg"), b"jpg").unwrap();
        fs::write(dir.path().join("A/T/metadata.opf"), b"opf").unwrap();
        assert!(dir.path().join("A/T").exists());

        cache.delete_book(book_id).unwrap();

        assert!(!dir.path().join("A/T").exists());
        assert!(!dir.path().join("A").exists());
        assert!(dir.path().exists());
    }

    /// The bug: books scanned from one folder share `books.path`, and
    /// deleting one removed the folder with its siblings and every
    /// non-book file in it.
    #[test]
    fn delete_book_leaves_siblings_and_non_book_files_in_a_shared_folder() {
        let (dir, cache) = open_test_cache();
        let folder = dir.path().join("Receipts");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.pdf"), b"%PDF a").unwrap();
        fs::write(folder.join("b.pdf"), b"%PDF b").unwrap();
        fs::write(folder.join("notes.txt"), b"not a book").unwrap();
        fs::write(folder.join("cover.jpg"), b"someone else's picture").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "A".to_string();
        let a = cache.register_book_in_place("Receipts/a.pdf", &meta).unwrap();
        meta.title = "B".to_string();
        let b = cache.register_book_in_place("Receipts/b.pdf", &meta).unwrap();

        let outcome = cache.delete_book_with(a, BookFiles::Trash).unwrap();

        assert_eq!(outcome.trashed, vec!["Receipts/a.pdf".to_string()]);
        assert!(!folder.join("a.pdf").exists());
        assert!(folder.join("b.pdf").exists(), "a sibling book was deleted");
        assert!(folder.join("notes.txt").exists(), "a non-book file was deleted");
        assert!(folder.join("cover.jpg").exists(), "a shared folder's cover.jpg is not this book's");
        assert!(cache.field_for(b, "title").unwrap().is_some());
        assert_eq!(outcome.folder_removed, None);
    }

    /// Deleting the last book of a folder that still holds a non-book file
    /// must not remove the folder.
    #[test]
    fn delete_book_never_removes_a_folder_that_is_not_empty() {
        let (dir, cache) = open_test_cache();
        let folder = dir.path().join("Docs");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.pdf"), b"%PDF a").unwrap();
        fs::write(folder.join("keep.txt"), b"mine").unwrap();
        let id = cache.register_book_in_place("Docs/a.pdf", &MetaInformation::default()).unwrap();

        cache.delete_book(id).unwrap();

        assert!(!folder.join("a.pdf").exists());
        assert!(folder.join("keep.txt").exists());
    }

    /// Without a wastebasket the file stays: no silent hard delete.
    #[test]
    fn delete_book_keeps_the_file_and_reports_when_trashing_fails() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        let book_id = cache.add_book(&source, &meta).unwrap();

        FORCE_TRASH_FAILURE.with(|f| f.set(true));
        let outcome = cache.delete_book_with(book_id, BookFiles::Trash);
        FORCE_TRASH_FAILURE.with(|f| f.set(false));
        let outcome = outcome.unwrap();

        assert_eq!(outcome.failed.len(), 1, "{outcome:?}");
        assert!(outcome.trashed.is_empty());
        assert!(dir.path().join("src.epub").exists(), "the file must not be hard-deleted");
        // Ignored, so the next scan does not resurrect the book.
        assert_eq!(cache.ignored().list().unwrap().len(), 1);
    }

    /// The library root is never removed, however empty it ends up.
    #[test]
    fn delete_book_never_removes_the_library_root() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let book_id = cache.add_book(&source, &MetaInformation::default()).unwrap();
        let outcome = cache.delete_book_with(book_id, BookFiles::Trash).unwrap();
        assert_eq!(outcome.folder_removed, None);
        assert!(dir.path().exists());
        assert!(dir.path().join("metadata.db").exists());
    }

    #[test]
    fn path_is_plain_rejects_escapes_and_the_state_directory() {
        assert!(path_is_plain("A/B"));
        assert!(!path_is_plain(""));
        assert!(!path_is_plain("../x"));
        assert!(!path_is_plain("A/../x"));
        assert!(!path_is_plain("/abs"));
        assert!(!path_is_plain(".lexmechanic/covers"));
    }

    #[test]
    fn delete_book_clears_its_checksum_records() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache.delete_book(book_id).unwrap();

        assert_eq!(
            cache
                .checksums()
                .verify_bytes(book_id, "format", "EPUB", b"x")
                .unwrap(),
            crate::checksums::VerifyOutcome::NoRecord
        );
    }

    #[test]
    fn clone_to_recursively_copies_the_library_directory() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        cache.add_book(&source, &meta).unwrap();

        let dest_dir = tempdir().unwrap();
        cache.clone_to(dest_dir.path()).unwrap();

        assert!(dest_dir.path().join("src.epub").exists());
    }

    // --- get_data_as_dict ---

    #[test]
    fn dedupe_names_trims_drops_empty_and_repeats_case_insensitively() {
        let got = dedupe_names(["Ann Lee", "  ann lee ", "", "  ", "Bob", "BOB", "Cy"]);
        assert_eq!(got, vec!["Ann Lee", "Bob", "Cy"]);
    }

    /// A book whose author list repeats a name (or differs only in case or
    /// whitespace) used to hit UNIQUE(book, author) and abort the insert.
    #[test]
    fn a_book_with_duplicate_authors_is_added_with_each_author_once() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "dup.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "Dup".to_string();
        meta.authors = vec!["Alice".into(), "alice".into(), "  ALICE ".into(), "Bob".into(), "".into()];
        let id = cache.add_book(&source, &meta).unwrap();
        assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some("Alice & Bob"));
        // Only blanks: falls back to "Unknown" rather than no author.
        let mut meta = MetaInformation::default();
        meta.title = "Blank".to_string();
        meta.authors = vec![" ".into()];
        let id = cache.add_book(&source, &meta).unwrap();
        assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some("Unknown"));
    }

    #[test]
    fn set_field_with_repeated_items_links_each_once() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "authors", "Ann & ann & Bob").unwrap();
        assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some("Ann & Bob"));
        cache.set_field(id, "tags", "x, X, y").unwrap();
        assert_eq!(cache.field_for(id, "tags").unwrap().as_deref(), Some("x, y"));
    }

    #[test]
    fn get_data_as_dict_includes_core_fields_authors_and_tags() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "My Book".to_string();
        meta.authors = vec!["Alice".to_string(), "Bob".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        {
            let conn = cache.backend.conn.lock().unwrap();
            for tag in ["fiction", "classic"] {
                conn.execute("INSERT INTO tags (name) VALUES (?1)", [tag])
                    .unwrap();
                let tag_id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
                    (book_id, tag_id),
                )
                .unwrap();
            }
        }

        let data = cache.get_data_as_dict(None, false, None, false).unwrap();
        assert_eq!(data.len(), 1);
        let rec = &data[0];
        assert_eq!(rec["id"], book_id);
        assert_eq!(rec["title"], "My Book");
        assert_eq!(rec["authors"], serde_json::json!(["Alice", "Bob"]));
        assert_eq!(rec["tags"], serde_json::json!(["fiction", "classic"]));
    }

    #[test]
    fn get_data_as_dict_joins_authors_as_a_string_when_asked() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["Alice".to_string(), "Bob".to_string()];
        cache.add_book(&source, &meta).unwrap();

        let data = cache.get_data_as_dict(None, true, None, false).unwrap();
        assert_eq!(data[0]["authors"], "Alice & Bob");
    }

    #[test]
    fn get_data_as_dict_resolves_a_real_format_path_and_available_formats() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        cache.add_book(&source, &meta).unwrap();

        let data = cache.get_data_as_dict(None, false, None, false).unwrap();
        let rec = &data[0];
        assert_eq!(rec["available_formats"], serde_json::json!(["EPUB"]));
        let formats = rec["formats"].as_array().unwrap();
        assert_eq!(formats.len(), 1);
        assert!(formats[0].as_str().unwrap().ends_with("src.epub"), "got {:?}", formats[0]);
        assert_eq!(rec["fmt_epub"], formats[0]);
    }

    #[test]
    fn get_data_as_dict_filters_by_the_given_ids() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "Keep Me".to_string();
        meta.authors = vec!["A".to_string()];
        let keep_id = cache.add_book(&source, &meta).unwrap();
        meta.title = "Skip Me".to_string();
        cache.add_book(&source, &meta).unwrap();

        let ids: std::collections::HashSet<i32> = [keep_id].into_iter().collect();
        let data = cache
            .get_data_as_dict(None, false, Some(&ids), false)
            .unwrap();

        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["title"], "Keep Me");
    }

    #[test]
    fn get_data_as_dict_includes_custom_column_values() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        cache
            .add_custom_column("mycol", "My Column", "text", false)
            .unwrap();
        cache
            .set_custom_column_value(book_id, "mycol", "hello")
            .unwrap();

        let data = cache.get_data_as_dict(None, false, None, false).unwrap();
        assert_eq!(data[0]["mycol"], "hello");
    }

    #[test]
    fn set_field_writes_scalar_book_columns() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "Old Title");
        cache.set_field(id, "title", "New Title").unwrap();
        cache.set_field(id, "series_index", "3.5").unwrap();
        assert_eq!(
            cache.field_for(id, "title").unwrap(),
            Some("New Title".to_string())
        );
        assert_eq!(
            cache.field_for(id, "series_index").unwrap(),
            Some("3.5".to_string())
        );
    }

    /// Renaming a book must not orphan its files.
    ///
    /// The row builder used to recompute each format's filename from
    /// the *current* title, while the file on disk keeps the name it
    /// was given when added. Editing a title therefore produced a path
    /// that did not exist, the `fmt_*` key vanished, and every route
    /// that serves a book -- read, open externally, download, convert
    /// -- returned 404 for a book that was sitting right there.
    #[test]
    fn a_renamed_book_can_still_be_opened() {
        let (dir, cache) = open_test_cache();

        let source = dir.path().join("original.pdf");
        std::fs::write(&source, b"%PDF-1.4 pretend").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "Original Title".to_string();
        meta.authors = vec!["An Author".to_string()];
        let id = cache.add_book(&source, &meta).unwrap();

        let row = |cache: &Cache| cache.get_data_as_dict(None, false, None, false).unwrap()[0].clone();

        let before = row(&cache);
        let path_before = before["fmt_pdf"].as_str().expect("the format resolves before the rename").to_string();
        assert!(std::path::Path::new(&path_before).exists());

        cache.set_field(id, "title", "A Completely Different Title").unwrap();

        let after = row(&cache);
        let path_after = after["fmt_pdf"].as_str().expect("the format must still resolve after a rename");
        assert!(
            std::path::Path::new(path_after).exists(),
            "the recorded path {path_after} does not exist, so every route serving this book will 404"
        );
        assert_eq!(after["title"], "A Completely Different Title");
    }

    /// Adds a book with one PDF whose source filename is `stem`.
    /// Adds a book and then gives it a folder of its own, the way a
    /// calibre-style library is laid out.
    ///
    /// `add_book` no longer creates one (#893), so a test whose *subject*
    /// is per-book folders -- the legacy cover
    /// location -- has to construct that layout explicitly rather than
    /// relying on it as a side effect.
    fn add_pdf_in_its_own_folder(dir: &Path, cache: &Cache, title: &str, author: &str, stem: &str) -> i32 {
        let id = add_pdf(dir, cache, title, author, stem);
        let folder = format!("{}/{}", sanitize_file_name(author), sanitize_file_name(title));
        let book_dir = dir.join(&folder);
        fs::create_dir_all(&book_dir).unwrap();
        for (format, name) in cache.format_file_names(id).unwrap() {
            let file = format!("{name}.{}", format.to_lowercase());
            fs::rename(dir.join(&file), book_dir.join(&file)).unwrap();
        }
        cache.set_book_path(id, &folder).unwrap();
        id
    }

    fn add_pdf(dir: &Path, cache: &Cache, title: &str, author: &str, stem: &str) -> i32 {
        let source = dir.join(format!("{stem}.pdf"));
        fs::write(&source, b"%PDF-1.4 pretend").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    #[test]
    fn renaming_files_moves_them_and_records_the_new_name() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "whatever");

        let before = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert!(Path::new(&before).exists());

        let renamed = cache.rename_format_files(id, "Nineteen Eighty-Four").unwrap();
        assert_eq!(renamed, vec!["PDF".to_string()]);

        // The row and the disk have to agree -- that invariant is the
        // whole reason this function updates `data` rather than only
        // moving files.
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("Nineteen Eighty-Four"));
        let after = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert!(after.ends_with("Nineteen Eighty-Four.pdf"), "got {after}");
        assert!(Path::new(&after).exists(), "the recorded path does not exist, so every route serving this book will 404");
        assert!(!Path::new(&before).exists(), "the old file is still there");
    }

    /// The other direction of #885's split, and the whole point of
    /// #893: editing metadata must not move anything on disk. It used
    /// to, so correcting a typo in a title rearranged files the user had
    /// put where they wanted them.
    #[test]
    fn editing_metadata_leaves_the_files_where_they_are() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "Old Title", "Old Author", "scan0001");
        let original = dir.path().join("scan0001.pdf");
        assert!(original.exists());

        cache.update_book_metadata(id, "A Completely New Title", "A Different Author").unwrap();

        // Metadata changed...
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("A Completely New Title"));
        // ...and not one byte moved.
        assert!(original.exists(), "editing the title moved the file");
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some(""));
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("scan0001"));
        assert!(!dir.path().join("A Different Author").exists());

        // And the book is still reachable, which is the failure mode the
        // old coupling produced when `data.name` fell out of step.
        let path = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert!(Path::new(&path).exists(), "{path} does not exist");
    }

    /// Same, through `set_field` -- the path the server's metadata editor
    /// actually takes.
    #[test]
    fn setting_the_title_field_leaves_the_files_where_they_are() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "Old Title", "An Author", "scan0001");
        let original = dir.path().join("scan0001.pdf");

        cache.set_field(id, "title", "Renamed In The UI").unwrap();

        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Renamed In The UI"));
        assert!(original.exists());
    }

    #[test]
    fn renaming_files_leaves_the_title_and_folder_alone() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        cache.rename_format_files(id, "Something Else").unwrap();

        // The point of the whole issue: naming the file is not
        // retitling the book.
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("A Title"));
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some(""));
    }

    #[test]
    fn renaming_files_leaves_the_cover_alone() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");
        crate::covers::set_cover(&cache, id, b"pretend jpeg").unwrap();

        cache.rename_format_files(id, "Something Else").unwrap();

        // `cover.jpg` is a fixed name every reader of a library
        // expects; sweeping it up in a filename rename would orphan
        // it. Driving off `data` rather than the directory is what
        // makes that automatic.
        assert!(crate::covers::cover_path(&cache, id).unwrap().exists());
    }

    /// A book's folder is `<author>/<title>`, and its files are named
    /// after the title -- so two books by the same author with the
    /// same title wanted the same path. `add_book` passes
    /// `replace = true`, so the second copy simply overwrote the
    /// first, and both `data` rows then named one file holding the
    /// second book's contents. Opening the first book handed you the
    /// second. Verified before it was fixed.
    #[test]
    fn adding_a_second_book_with_the_same_title_does_not_overwrite_the_first() {
        let (dir, cache) = open_test_cache();
        let first_src = dir.path().join("one.pdf");
        fs::write(&first_src, b"BOOK ONE").unwrap();
        let second_src = dir.path().join("two.pdf");
        fs::write(&second_src, b"BOOK TWO").unwrap();

        let mut meta = MetaInformation::default();
        meta.title = "Same Title".to_string();
        meta.authors = vec!["Same Author".to_string()];
        let first = cache.add_book(&first_src, &meta).unwrap();
        let second = cache.add_book(&second_src, &meta).unwrap();

        // They do share a folder -- that part is by design.
        assert_eq!(cache.field_for(first, "path").unwrap(), cache.field_for(second, "path").unwrap());

        let path_of = |id: i32| {
            cache.get_data_as_dict(None, false, None, false).unwrap().into_iter().find(|r| r["id"] == id).unwrap()["fmt_pdf"].as_str().unwrap().to_string()
        };
        let (first_path, second_path) = (path_of(first), path_of(second));

        assert_ne!(first_path, second_path, "both books still point at one file");
        assert_eq!(fs::read(&first_path).unwrap(), b"BOOK ONE");
        assert_eq!(fs::read(&second_path).unwrap(), b"BOOK TWO");
    }

    /// Since #893 a file keeps its source name, so two books with the
    /// same title no longer want the same filename -- the collision case
    /// is now two *sources* with the same name, which is just as easy to
    /// hit (two folders of `scan0001.pdf`).
    #[test]
    fn two_sources_with_the_same_name_do_not_overwrite_each_other() {
        let (dir, cache) = open_test_cache();
        let first_src = dir.path().join("a").join("scan0001.pdf");
        let second_src = dir.path().join("b").join("scan0001.pdf");
        for (path, bytes) in [(&first_src, &b"BOOK ONE"[..]), (&second_src, &b"BOOK TWO"[..])] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        let mut meta = MetaInformation::default();
        meta.title = "Same Title".to_string();
        meta.authors = vec!["Same Author".to_string()];
        let first = cache.add_book(&first_src, &meta).unwrap();
        let second = cache.add_book(&second_src, &meta).unwrap();

        let path_of = |id: i32| {
            cache.get_data_as_dict(None, false, None, false).unwrap().into_iter().find(|r| r["id"] == id).unwrap()["fmt_pdf"].as_str().unwrap().to_string()
        };
        let (first_path, second_path) = (path_of(first), path_of(second));

        assert_ne!(first_path, second_path);
        assert_eq!(fs::read(&first_path).unwrap(), b"BOOK ONE");
        assert_eq!(fs::read(&second_path).unwrap(), b"BOOK TWO");
        assert!(second_path.ends_with("scan0001 (1).pdf"), "got {second_path}");
    }

    #[test]
    fn re_adding_the_same_format_to_one_book_still_replaces_its_own_file() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        let replacement = dir.path().join("newer.pdf");
        fs::write(&replacement, b"NEWER CONTENT").unwrap();
        assert!(cache.add_format(id, &replacement, "pdf", true).unwrap());

        // The replacement brings its own name with it (#893)...
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("newer"));
        let path = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert_eq!(fs::read(&path).unwrap(), b"NEWER CONTENT");
        // ...and the file it replaced is gone, not left behind with
        // nothing pointing at it.
        assert!(!dir.path().join("A Title.pdf").exists(), "the replaced file was orphaned on disk");
    }

    /// Re-adding a format under the *same* name overwrites in place, and
    /// does not walk the name to `(1)`.
    /// An empty `books.path` means the library root, which is where a
    /// book in a tracked folder normally lives (#889). A guard that
    /// refused it was correct only while every book got its own
    /// `<author>/<title>/` folder.
    #[test]
    fn a_format_can_be_added_to_a_book_whose_files_are_in_the_library_root() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "book");
        cache.set_book_path(id, "").unwrap();

        let epub = dir.path().join("elsewhere").join("book.epub");
        fs::create_dir_all(epub.parent().unwrap()).unwrap();
        fs::write(&epub, b"epub bytes").unwrap();

        assert!(cache.add_format(id, &epub, "epub", true).unwrap());
        assert!(dir.path().join("book.epub").exists(), "the format should land in the library root");
    }

    #[test]
    fn re_adding_a_format_under_the_same_name_overwrites_it() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "book");

        let again = dir.path().join("again").join("book.pdf");
        fs::create_dir_all(again.parent().unwrap()).unwrap();
        fs::write(&again, b"SECOND VERSION").unwrap();
        assert!(cache.add_format(id, &again, "pdf", true).unwrap());

        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("book"));
        assert!(!dir.path().join("book (1).pdf").exists());
        let path = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert_eq!(fs::read(&path).unwrap(), b"SECOND VERSION");
    }

    /// `remove_format` used to delete the first file in the folder
    /// with a matching extension. In a folder shared by two books that
    /// is a coin flip, and losing it deletes a book nobody asked to
    /// touch.
    #[test]
    fn removing_a_format_deletes_this_books_file_and_not_the_other_ones() {
        let (dir, cache) = open_test_cache();
        let first_src = dir.path().join("one.pdf");
        fs::write(&first_src, b"BOOK ONE").unwrap();
        let second_src = dir.path().join("two.pdf");
        fs::write(&second_src, b"BOOK TWO").unwrap();

        let mut meta = MetaInformation::default();
        meta.title = "Same Title".to_string();
        meta.authors = vec!["Same Author".to_string()];
        let first = cache.add_book(&first_src, &meta).unwrap();
        let second = cache.add_book(&second_src, &meta).unwrap();

        let second_path = cache.get_data_as_dict(None, false, None, false).unwrap().into_iter().find(|r| r["id"] == second).unwrap()["fmt_pdf"].as_str().unwrap().to_string();

        cache.remove_format(first, "pdf").unwrap();

        assert!(Path::new(&second_path).exists(), "removing one book's format deleted the other book's file");
        assert_eq!(fs::read(&second_path).unwrap(), b"BOOK TWO");
        assert!(!dir.path().join("Same Title.pdf").exists(), "the format that was asked for is still there");
    }

    #[test]
    fn an_available_stem_steps_around_a_name_that_is_taken() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");
        let book_dir = dir.path().to_path_buf();
        fs::write(book_dir.join("Taken.pdf"), b"someone else's").unwrap();

        assert_eq!(cache.available_format_stem(id, "Free").unwrap(), "Free");
        assert_eq!(cache.available_format_stem(id, "Taken").unwrap(), "Taken (1)");
        // Its own name is not a collision -- otherwise renaming a book
        // to what it is already called would walk it to `(1)`.
        assert_eq!(cache.available_format_stem(id, "A Title").unwrap(), "A Title");
    }

    #[test]
    fn an_available_stem_must_be_free_for_every_format_the_book_has() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");
        let epub = dir.path().join("x.epub");
        fs::write(&epub, b"epub bytes").unwrap();
        cache.add_format(id, &epub, "epub", true).unwrap();

        // Only the EPUB name is taken, but a rename moves both files,
        // so the stem is unusable for the book as a whole.
        let book_dir = dir.path().to_path_buf();
        fs::write(book_dir.join("Wanted.epub"), b"someone else's").unwrap();
        assert_eq!(cache.available_format_stem(id, "Wanted").unwrap(), "Wanted (1)");
    }

    #[test]
    fn renaming_files_refuses_to_overwrite_an_existing_file() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        // Something else already occupies the name being asked for.
        // Two books with the same author and title really do share a
        // folder, so this is a reachable state, not a contrived one.
        let book_dir = dir.path().to_path_buf();
        fs::write(book_dir.join("Taken.pdf"), b"someone else's book").unwrap();

        let err = cache.rename_format_files(id, "Taken").unwrap_err().to_string();
        assert!(err.contains("already exists"), "got: {err}");

        // Nothing moved, and the file that was in the way is intact.
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("A Title"));
        assert_eq!(fs::read(book_dir.join("Taken.pdf")).unwrap(), b"someone else's book");
        assert!(book_dir.join("A Title.pdf").exists());
    }

    #[test]
    fn renaming_files_rejects_a_name_that_sanitizes_away_to_nothing() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");
        assert!(cache.rename_format_files(id, "   ").is_err());
        assert!(cache.rename_format_files(id, "..").is_err());
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("A Title"));
    }

    #[test]
    fn renaming_files_to_the_name_they_already_have_is_a_no_op() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");
        assert!(cache.rename_format_files(id, "A Title").unwrap().is_empty());
    }

    #[test]
    fn renaming_files_sanitizes_a_name_that_would_escape_the_folder() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        cache.rename_format_files(id, "../../etc/passwd").unwrap();

        let stem = cache.format_file_stem(id).unwrap().unwrap();
        assert!(!stem.contains('/') && !stem.contains('\\'), "got {stem}");
        let path = cache.get_data_as_dict(None, false, None, false).unwrap()[0]["fmt_pdf"].as_str().unwrap().to_string();
        assert!(Path::new(&path).starts_with(dir.path()), "{path} escaped the library");
    }

    #[test]
    fn a_book_whose_formats_were_added_under_different_titles_has_no_single_stem() {
        let (dir, cache) = open_test_cache();
        let id = add_pdf(dir.path(), &cache, "First Title", "An Author", "First Title");

        // `add_format` names a file after the title at the moment it
        // runs, so retitling between two adds leaves the two formats
        // with different stems.
        cache.set_field(id, "title", "Second Title").unwrap();
        let epub = dir.path().join("x.epub");
        fs::write(&epub, b"epub bytes").unwrap();
        cache.add_format(id, &epub, "epub", true).unwrap();

        assert_eq!(cache.format_file_stem(id).unwrap(), None);
        // ...and renaming brings them back into line.
        cache.rename_format_files(id, "One Name").unwrap();
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("One Name"));
    }

    #[test]
    fn set_field_replaces_the_entire_many_to_many_set() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "tags", "fiction, adventure").unwrap();
        assert_eq!(
            cache.field_for(id, "tags").unwrap(),
            Some("fiction, adventure".to_string())
        );

        // Replacing drops tags not in the new set.
        cache.set_field(id, "tags", "adventure, thriller").unwrap();
        assert_eq!(
            cache.field_for(id, "tags").unwrap(),
            Some("adventure, thriller".to_string())
        );
    }

    #[test]
    fn set_field_reuses_an_existing_item_row_by_name() {
        let (_dir, cache) = open_test_cache();
        let a = insert_book(&cache, "A");
        let b = insert_book(&cache, "B");
        cache.set_field(a, "tags", "fiction").unwrap();
        cache.set_field(b, "tags", "fiction").unwrap();

        let conn = cache.backend.conn.lock().unwrap();
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM tags WHERE name = 'fiction'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn set_field_replaces_a_many_to_one_link() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "series", "First Series").unwrap();
        assert_eq!(
            cache.field_for(id, "series").unwrap(),
            Some("First Series".to_string())
        );
        cache.set_field(id, "series", "Second Series").unwrap();
        assert_eq!(
            cache.field_for(id, "series").unwrap(),
            Some("Second Series".to_string())
        );
    }

    #[test]
    fn set_field_rating_zero_clears_the_rating() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache.set_field(id, "rating", "8").unwrap();
        assert_eq!(
            cache.field_for(id, "rating").unwrap(),
            Some("8".to_string())
        );
        cache.set_field(id, "rating", "0").unwrap();
        assert_eq!(cache.field_for(id, "rating").unwrap(), None);
    }

    #[test]
    fn set_field_identifiers_replaces_the_whole_set() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        cache
            .set_field(id, "identifiers", "isbn:123,doi:abc")
            .unwrap();
        assert_eq!(
            cache.field_for(id, "identifiers").unwrap(),
            Some("isbn:123,doi:abc".to_string())
        );
        cache.set_field(id, "identifiers", "isbn:456").unwrap();
        assert_eq!(
            cache.field_for(id, "identifiers").unwrap(),
            Some("isbn:456".to_string())
        );
    }

    #[test]
    fn set_field_rejects_unwritable_fields() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert!(cache.set_field(id, "size", "1").is_err());
    }

    #[test]
    fn has_cover_reflects_the_books_column() {
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "T");
        assert!(!cache.has_cover(id).unwrap());
        cache.set_field(id, "has_cover", "1").unwrap();
        assert!(cache.has_cover(id).unwrap());
    }

    #[test]
    fn field_for_reloads_after_a_raw_sql_write_bypassing_every_cache_method() {
        // Issue #222 phase 3: `field_for` now reads from a lazily
        // loaded in-memory snapshot, not per-call SQL -- this proves
        // the automatic `total_changes()`-based staleness check
        // catches a write that goes through *neither* `Cache::set_field`
        // nor an explicit `invalidate_field_cache()` call, just a raw
        // `conn.execute` on the same connection.
        let (_dir, cache) = open_test_cache();
        let id = insert_book(&cache, "Original Title");

        // Prime the cache -- after this, `field_for` has a loaded
        // snapshot that does NOT reflect the write below yet.
        assert_eq!(
            cache.field_for(id, "title").unwrap(),
            Some("Original Title".to_string())
        );

        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute(
                "UPDATE books SET title = 'Changed By Raw SQL' WHERE id = ?1",
                (id,),
            )
            .unwrap();
        }

        assert_eq!(
            cache.field_for(id, "title").unwrap(),
            Some("Changed By Raw SQL".to_string())
        );
    }

    #[test]
    fn two_cache_values_over_the_same_backend_each_see_current_data() {
        // A fresh `Cache` (e.g. every `Library::as_cache()` call) has
        // its own empty snapshot -- it must not somehow inherit a
        // stale one from a different `Cache` value sharing the same
        // underlying connection.
        let dir = tempdir().unwrap();
        let backend = Backend::new(dir.path()).unwrap();
        let cache_a = Cache::from_backend(backend.clone());
        let id = insert_book(&cache_a, "First");
        assert_eq!(
            cache_a.field_for(id, "title").unwrap(),
            Some("First".to_string())
        );

        cache_a.set_field(id, "title", "Second").unwrap();

        let cache_b = Cache::from_backend(backend);
        assert_eq!(
            cache_b.field_for(id, "title").unwrap(),
            Some("Second".to_string())
        );
    }

    /// #901: library-scoped writes reach the log too, so a rebuild can
    /// restore preferences and not just books.
    #[test]
    fn setting_and_clearing_a_preference_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();

        cache.set_preference("sort", "title").unwrap();
        cache.clear_preference("sort").unwrap();

        let log = cache.backend.change_log().unwrap();
        let replay = log.replay().unwrap();
        let ops: Vec<&crate::change_log::ChangeOp> = replay.changes().map(|c| &c.op).collect();
        assert!(
            ops.iter().any(|op| matches!(op, crate::change_log::ChangeOp::PrefSet { key, value: Some(v) } if key == "sort" && v == "title")),
            "{ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, crate::change_log::ChangeOp::PrefSet { key, value: None } if key == "sort")),
            "clearing a preference has to be expressible, or a replay cannot unset it: {ops:?}"
        );
    }

    /// The schema half of #901: the column's *definition* is logged, not
    /// just the values in it.
    #[test]
    fn creating_and_removing_a_custom_column_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();

        cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();
        cache.remove_custom_column("shelf").unwrap();

        let log = cache.backend.change_log().unwrap();
        let replay = log.replay().unwrap();
        let ops: Vec<&crate::change_log::ChangeOp> = replay.changes().map(|c| &c.op).collect();
        assert!(
            ops.iter().any(|op| matches!(op, crate::change_log::ChangeOp::CustomColumnAdded { label, datatype, .. } if label == "shelf" && datatype == "text")),
            "{ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, crate::change_log::ChangeOp::CustomColumnRemoved { label } if label == "shelf")),
            "{ops:?}"
        );
    }
}
