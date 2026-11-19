//! Port of `docs/FAULT_TOLERANCE.md` §8's other half (issue #93): "Every
//! book file's BLAKE3 is stored in metadata.db at add time and
//! re-verified on any operation that touches the file... Cover images
//! and sidecar files: same rule."
//!
//! # Where the checksums actually live
//!
//! Not literally inside `metadata.db`'s own schema -- that schema is
//! kept byte-for-byte compatible with real calibre's `schema.py` (see
//! `schema_upgrades.rs`; every `CREATE TABLE` there matches upstream
//! exactly) so a library stays fully openable by the real application.
//! Adding an oxide-only table there would break that. Instead this
//! follows the same convention `notes/connection.rs` (#227) and
//! `fts/connection.rs` (#226) already established for oxide-only
//! sidecar state: a separate SQLite file, `ATTACH`ed to the same
//! shared connection, living under `<library>/.calibre-oxide/` -- the
//! same directory [`crate::library_handle::LibraryHandle`]'s writer
//! lock and journal use (#93 phases 1-2), since this is the same
//! fault-tolerance story.
//!
//! # What's real
//!
//! [`ChecksumStore`]: BLAKE3-hash storage and verification keyed by
//! `(book_id, kind, key)` -- `kind` is `"format"`/`"cover"`/`"opf"`,
//! `key` is the format extension (empty for cover/opf, there's only
//! ever one of each per book). Keying by book id + kind + key instead
//! of by on-disk path means a book's checksums survive
//! [`crate::cache::Cache`]'s title/author-driven folder and filename
//! renames for free -- no extra bookkeeping needed at rename time,
//! since the identity that matters (which book, which format) doesn't
//! change even though the path does.
//!
//! Wired into every place in this crate that actually writes or
//! removes one of these files: [`crate::cache::Cache::add_format`]
//! (covers [`crate::cache::Cache::add_book`] too, which delegates to
//! it) records; [`crate::cache::Cache::remove_format`] and
//! [`crate::cache::Cache::delete_book`] remove the now-stale record(s)
//! so the sidecar db doesn't accumulate orphaned rows forever;
//! [`crate::covers::set_cover`] records the cover; `backup.rs`'s
//! `backup_metadata` records the `metadata.opf` sidecar. Re-
//! verification is wired into `check_library.rs`'s existing per-book
//! scan (`CheckLibrary::corrupted_formats`/`corrupted_covers`) --
//! that module already reads every book's directory to compare
//! against the DB, so checking file content there is the natural,
//! self-contained home for it rather than a crate-wide retrofit of
//! every place that ever reads a book file's bytes (still deferred,
//! same as phases 1-3's module docs already disclose).
//!
//! # Disclosed simplifications
//!
//! - **[`VerifyOutcome::NoRecord`] is not corruption.** A file that
//!   predates this feature (added by an older calibre-oxide build, or
//!   by real calibre, which never writes this sidecar at all) has no
//!   stored checksum to compare against -- reported as "unknown", not
//!   silently treated as verified and not flagged as corrupt either.
//!   Honest about what was never recorded rather than pretending
//!   coverage that doesn't exist.
//! - **Not wired into every read path.** Verification lives in
//!   `check_library.rs`'s scan and, as of the export pass, in
//!   `cli/cmd_export.rs`'s per-book copy (a real BLAKE3 mismatch
//!   there skips exporting that book rather than copying corrupted
//!   bytes out of the library -- matching §8's "the operation aborts
//!   before mutating anything", scoped to the one book, not the whole
//!   export run, consistent with this loop's existing "skip and
//!   continue" handling for a missing file). Any other function that
//!   opens a book file directly still reads it unverified; a
//!   corrupted file there is caught the next time a library check
//!   runs, not at the moment of use.
//!
//!   There is deliberately **no** "convert" call site wired in: this
//!   repo has no `cmd_convert` command that resolves a book_id/format
//!   through `Library`/`Cache` at all yet (`calibre_conversion`'s
//!   `ebook_convert` binary is a standalone file-to-file converter
//!   with no concept of a library or book id) -- there is nothing to
//!   wire re-verification into until that command exists.
//!
//!   There is also deliberately **no** "catalog generation" call
//!   site: checked directly (issue #93 follow-up) rather than assumed
//!   -- `cli/cmd_catalog.rs` (its only implemented format is CSV;
//!   anything else is a hard "unsupported format" error) writes only
//!   DB metadata fields (title/author/pubdate/isbn/`path`-the-string)
//!   into the CSV. It never opens a format file, a cover, or any
//!   other on-disk book file at all, so there is no book-file byte
//!   read here to verify against. (An earlier draft of this doc
//!   claimed catalog generation "still reads a book file directly" --
//!   that was an unverified assumption, not a checked fact; corrected
//!   here after actually reading `cmd_catalog.rs`.) Real upstream
//!   `catalog.py` -- unported, tracked separately in
//!   `docs/modules_to_port.md` -- generates richer catalog formats
//!   that *do* embed cover thumbnails; if that ever gets ported, its
//!   cover-reading code would be a real site to wire in.
//!
//!   Extending re-verification to more call sites (convert, once it
//!   exists; anywhere else that reads a book file directly) is part
//!   of the same still-open "crate-wide retrofit" item phases 1-3
//!   already track under #93.
//! - **No re-verification on `metadata.db` itself** -- this covers
//!   book/cover/OPF *files*, not the SQLite database's own integrity
//!   (SQLite's own WAL/page-checksum machinery is the relevant
//!   mechanism there, out of scope for this file).

use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::constants::{CHECKSUMS_DB_NAME, LIBRARY_HANDLE_DIR_NAME};

#[derive(Debug, Error)]
pub enum ChecksumError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "checksum mismatch for book {book_id} {kind}{}: expected {expected}, found {actual}",
        key_suffix(key)
    )]
    Mismatch {
        book_id: i32,
        kind: String,
        key: String,
        expected: String,
        actual: String,
    },
}

fn key_suffix(key: &str) -> String {
    if key.is_empty() {
        String::new()
    } else {
        format!(" ({key})")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome {
    Match,
    /// No checksum was ever recorded for this `(book_id, kind, key)` --
    /// see module doc for why this is distinct from `Match`.
    NoRecord,
}

/// One hash to record, for [`ChecksumStore::record_many`].
#[derive(Debug, Clone)]
pub struct HashRecord {
    pub book_id: i32,
    pub kind: String,
    pub key: String,
    pub hash: String,
    pub size: i64,
    pub identity: Option<calibre_utils::filenames::FileIdentity>,
}

/// One row of the folder importer's ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub hash: String,
    pub size: i64,
    /// The source file's modification time, nanoseconds since the epoch.
    pub mtime_ns: i64,
    /// `imported`, `merged`, `duplicate_content` or `duplicate_title`.
    pub outcome: String,
    /// The duplicate policy in force when it was decided.
    pub policy: String,
}

/// See this module's doc comment for the full design.
/// A PDF format owed a fingerprint.
#[derive(Debug, Clone)]
pub struct PendingPdf {
    pub book_id: i32,
    pub format: String,
    pub hash: String,
    pub name: String,
    pub folder: String,
}

pub struct ChecksumStore {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
}

impl ChecksumStore {
    pub fn new(conn: Arc<Mutex<Connection>>, library_path: &Path) -> Self {
        let db_path = library_path
            .join(LIBRARY_HANDLE_DIR_NAME)
            .join(CHECKSUMS_DB_NAME);
        ChecksumStore { conn, db_path }
    }

    /// Attaches `checksums.db` (if not already) and creates the real
    /// schema -- same one-shot-no-upgrade-ladder shape as
    /// `notes/connection.rs`'s `initialize`, since this crate has only
    /// ever had one version of this schema.
    pub fn initialize(&self) -> SqlResult<()> {
        if let Some(dir) = self.db_path.parent() {
            fs::create_dir_all(dir).ok();
        }

        let conn = self.conn.lock().unwrap();
        let attached: i32 = conn
            .query_row(
                "SELECT count(*) FROM pragma_database_list WHERE name='checksums_db'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if attached == 0 {
            conn.execute(
                "ATTACH DATABASE ? AS checksums_db",
                [self.db_path.to_str().unwrap()],
            )?;
            // docs/FAULT_TOLERANCE.md §3 (issue #260): each attached
            // sidecar database needs its own journal_mode/synchronous
            // pragma -- unqualified PRAGMA journal_mode only applies
            // to `main`, not to an ATTACHed schema.
            // Follows `main`'s mode, so a network-tier library never
            // gets a WAL sidecar (#258).
            conn.execute_batch(&crate::storage_safety::sidecar_journal_sql(
                &conn,
                "checksums_db",
            ))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS checksums_db.file_checksums (
                    book_id INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    key TEXT NOT NULL,
                    blake3_hex TEXT NOT NULL,
                    size INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (book_id, kind, key)
                );
                CREATE INDEX IF NOT EXISTS checksums_db.file_checksums_by_hash
                    ON file_checksums (blake3_hex);
                -- Books the in-place scan registered from their filename
                -- alone, still owed their embedded metadata and cover.
                CREATE TABLE IF NOT EXISTS checksums_db.enrichment_queue (
                    book_id INTEGER PRIMARY KEY,
                    format TEXT NOT NULL,
                    queued_at TEXT NOT NULL
                );
                -- What the folder importer has done with each source file,
                -- so a rerun skips it without reading it again.
                CREATE TABLE IF NOT EXISTS checksums_db.import_ledger (
                    source TEXT NOT NULL PRIMARY KEY,
                    blake3_hex TEXT NOT NULL,
                    size INTEGER NOT NULL,
                    mtime_ns INTEGER NOT NULL,
                    outcome TEXT NOT NULL,
                    policy TEXT NOT NULL,
                    book_uuid TEXT,
                    imported_at TEXT NOT NULL
                );
                -- Page and text fingerprints of PDFs (`pdf_similarity`),
                -- keyed by the content hash so identical files share a row.
                CREATE TABLE IF NOT EXISTS checksums_db.pdf_fingerprints (
                    blake3_hex TEXT NOT NULL PRIMARY KEY,
                    version INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    error TEXT,
                    page_count INTEGER NOT NULL DEFAULT 0,
                    has_text INTEGER NOT NULL DEFAULT 0,
                    text_chars INTEGER NOT NULL DEFAULT 0,
                    pages BLOB,
                    minhash BLOB,
                    computed_at TEXT NOT NULL
                );",
            )?;
        }

        // `(volume, file_index)` -- the filesystem's own identity for the
        // file, from `calibre_utils::filenames::identity`. Added after
        // the table existed, so an existing sidecar needs the columns
        // bolted on rather than recreated: dropping and rebuilding would
        // throw away every recorded hash.
        //
        // Nullable because a row written before this, or on a platform
        // that cannot report an identity, simply has none -- and the
        // drift pass falls back to matching on content.
        for column in ["volume", "file_index"] {
            let exists: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('file_checksums') WHERE name = ?1")
                .and_then(|mut stmt| stmt.exists([column]))
                .unwrap_or(false);
            if !exists {
                // A failure here is not fatal: without the column,
                // identity matching is unavailable and content matching
                // still works.
                let _ = conn.execute(&format!("ALTER TABLE checksums_db.file_checksums ADD COLUMN {column} INTEGER"), ());
            }
        }
        Ok(())
    }

    fn record_bytes(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        bytes: &[u8],
    ) -> Result<(), ChecksumError> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        self.record_hash(book_id, kind, key, &hash, bytes.len() as i64)
    }

    /// Records a BLAKE3 already computed elsewhere -- the large-file-
    /// safe counterpart to [`ChecksumStore::record_file`], for a
    /// caller (e.g. [`crate::library_handle::LibraryHandle::copy_atomic`]'s
    /// streaming copy) that already hashed the content without ever
    /// buffering the whole file in memory, so this doesn't need a
    /// second full read of it just to hash it again.
    pub fn record_hash(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        hash: &str,
        size: i64,
    ) -> Result<(), ChecksumError> {
        self.record_hash_with_identity(book_id, kind, key, hash, size, None)
    }

    /// As [`ChecksumStore::record_hash`], also recording the
    /// filesystem's own identity for the file.
    ///
    /// The identity is what makes a rename detectable without reading
    /// anything: it survives a move within a volume, so a file found at
    /// an unexpected path can be recognised from one `stat` instead of a
    /// full hash. It also catches the case content matching cannot --
    /// a file that was both edited *and* moved.
    pub fn record_hash_with_identity(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        hash: &str,
        size: i64,
        identity: Option<calibre_utils::filenames::FileIdentity>,
    ) -> Result<(), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        Self::record_hash_on(&conn, book_id, kind, key, hash, size, identity)?;
        Ok(())
    }

    /// The statement behind [`ChecksumStore::record_hash_with_identity`],
    /// on a connection the caller already holds -- so a batch can record
    /// many hashes inside the transaction that creates the books they
    /// describe, rather than locking and committing once per file.
    ///
    /// The sidecar must already be attached ([`ChecksumStore::initialize`]).
    pub(crate) fn record_hash_on(
        conn: &Connection,
        book_id: i32,
        kind: &str,
        key: &str,
        hash: &str,
        size: i64,
        identity: Option<calibre_utils::filenames::FileIdentity>,
    ) -> SqlResult<()> {
        let (volume, index) = match identity {
            Some(id) => (Some(id.volume as i64), Some(id.index as i64)),
            None => (None, None),
        };
        conn.execute(
            "INSERT INTO checksums_db.file_checksums
                (book_id, kind, key, blake3_hex, size, updated_at, volume, file_index)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), ?6, ?7)
             ON CONFLICT(book_id, kind, key) DO UPDATE SET
                blake3_hex = excluded.blake3_hex,
                size = excluded.size,
                updated_at = excluded.updated_at,
                -- Keep the old identity when the caller has none, rather
                -- than erasing a usable one with a NULL.
                volume = COALESCE(excluded.volume, volume),
                file_index = COALESCE(excluded.file_index, file_index)",
            (book_id, kind, key, hash, size, volume, index),
        )?;
        Ok(())
    }

    /// Records many hashes in one `BEGIN IMMEDIATE` transaction -- the
    /// batch form of [`ChecksumStore::record_hash_with_identity`], for the
    /// hash backfill, where a commit per file would be most of its cost.
    pub fn record_many(&self, records: &[HashRecord]) -> Result<(), ChecksumError> {
        if records.is_empty() {
            return Ok(());
        }
        self.initialize()?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for r in records {
            Self::record_hash_on(&tx, r.book_id, &r.kind, &r.key, &r.hash, r.size, r.identity)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Queues a book for the lazy metadata pass (cover, embedded title
    /// and authors). On a connection the caller holds, for the same
    /// reason as [`ChecksumStore::record_hash_on`].
    pub(crate) fn queue_enrichment_on(conn: &Connection, book_id: i32, format: &str) -> SqlResult<()> {
        conn.execute(
            "INSERT OR REPLACE INTO checksums_db.enrichment_queue (book_id, format, queued_at) VALUES (?1, ?2, datetime('now'))",
            (book_id, format),
        )?;
        Ok(())
    }

    /// Books still waiting for the metadata pass, as
    /// `(book_id, format, file stem, folder)`, lowest id first.
    ///
    /// The queue is the whole of the pass's state, which is what makes it
    /// resumable: a book leaves it only once it has been enriched, so a
    /// rerun after a cancel or a crash sees exactly what is left.
    pub fn pending_enrichment(&self) -> Result<Vec<(i32, String, String, String)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT q.book_id, q.format, d.name, b.path
             FROM checksums_db.enrichment_queue q
             JOIN data d ON d.book = q.book_id AND d.format = q.format
             JOIN books b ON b.id = q.book_id
             ORDER BY q.book_id",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        Ok(rows.collect::<SqlResult<Vec<_>>>()?)
    }

    /// Takes a book off the metadata queue: it was enriched, or there is
    /// nothing more to learn from its file.
    pub fn finish_enrichment(&self, book_id: i32) -> Result<(), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checksums_db.enrichment_queue WHERE book_id = ?1", (book_id,))?;
        Ok(())
    }

    /// Every format file with no recorded content hash, as
    /// `(book_id, format, file stem, folder)`.
    ///
    /// This is every file of a library opened from real calibre, which
    /// never writes this sidecar. One query for the whole library rather
    /// than a lookup per file.
    pub fn formats_without_hash(&self) -> Result<Vec<(i32, String, String, String)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT d.book, d.format, d.name, b.path
             FROM data d JOIN books b ON b.id = d.book
             WHERE NOT EXISTS (
                 SELECT 1 FROM checksums_db.file_checksums c
                 WHERE c.book_id = d.book AND c.kind = 'format' AND c.key = d.format
             )
             ORDER BY d.book, d.format",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        Ok(rows.collect::<SqlResult<Vec<_>>>()?)
    }

    /// What was recorded for one file: its hash, and its filesystem
    /// identity if one was captured.
    ///
    /// Both optional and independently so -- a row written before
    /// identities were stored has a hash and no identity, and a
    /// platform that cannot report one leaves it null for ever.
    #[allow(clippy::type_complexity)]
    pub fn recorded_identity(&self, book_id: i32, kind: &str, key: &str) -> Result<(Option<String>, Option<calibre_utils::filenames::FileIdentity>), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, Option<i64>, Option<i64>)> = conn
            .query_row(
                "SELECT blake3_hex, volume, file_index FROM checksums_db.file_checksums
                 WHERE book_id = ?1 AND kind = ?2 AND key = ?3",
                (book_id, kind, key),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        Ok(match row {
            Some((hash, Some(volume), Some(index))) => (Some(hash), Some(calibre_utils::filenames::FileIdentity { volume: volume as u64, index: index as u64 })),
            Some((hash, _, _)) => (Some(hash), None),
            None => (None, None),
        })
    }

    /// Every recorded format file as `(book_id, FORMAT) -> (hash, size,
    /// identity)`, in one query.
    ///
    /// For passes that look at every file ([`crate::drift`]): calling
    /// [`ChecksumStore::recorded_identity`] per file is a query -- and a
    /// schema check -- each, which on a library of 300,000 files is most
    /// of the pass.
    #[allow(clippy::type_complexity)]
    pub fn all_format_records(&self) -> Result<std::collections::HashMap<(i32, String), (String, i64, Option<calibre_utils::filenames::FileIdentity>)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT book_id, key, blake3_hex, size, volume, file_index FROM checksums_db.file_checksums WHERE kind = 'format'")?;
        let rows = stmt.query_map([], |row| {
            let identity = match (row.get::<_, Option<i64>>(4)?, row.get::<_, Option<i64>>(5)?) {
                (Some(volume), Some(index)) => Some(calibre_utils::filenames::FileIdentity { volume: volume as u64, index: index as u64 }),
                _ => None,
            };
            Ok(((row.get::<_, i32>(0)?, row.get::<_, String>(1)?), (row.get::<_, String>(2)?, row.get::<_, i64>(3)?, identity)))
        })?;
        Ok(rows.collect::<SqlResult<_>>()?)
    }

    /// Which book owns the file with this filesystem identity.
    ///
    /// The cheap half of rename recovery: one `stat` on a file found at
    /// an unexpected path answers "whose is this?" without reading a
    /// byte of it.
    pub fn find_by_identity(&self, identity: calibre_utils::filenames::FileIdentity) -> Result<Vec<(i32, String, String)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT book_id, kind, key FROM checksums_db.file_checksums
             WHERE volume = ?1 AND file_index = ?2 ORDER BY book_id, kind, key",
        )?;
        let rows = stmt.query_map((identity.volume as i64, identity.index as i64), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Reads `path` and records its BLAKE3 -- the "at add time" half
    /// of §8. Call right after the write that created/replaced `path`
    /// succeeds. Buffers the whole file in memory to hash it; prefer
    /// [`ChecksumStore::record_hash`] with an already-streamed hash
    /// when `path` could be large.
    pub fn record_file(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        path: &Path,
    ) -> Result<(), ChecksumError> {
        let bytes = fs::read(path)?;
        self.record_bytes(book_id, kind, key, &bytes)
    }

    fn stored(&self, book_id: i32, kind: &str, key: &str) -> Result<Option<String>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT blake3_hex FROM checksums_db.file_checksums
                 WHERE book_id = ?1 AND kind = ?2 AND key = ?3",
                (book_id, kind, key),
                |row| row.get(0),
            )
            .optional()?)
    }

    /// The "re-verified on any operation that touches the file" half
    /// of §8. `Err(ChecksumError::Mismatch)` is real corruption, not a
    /// missing-record case -- see [`VerifyOutcome::NoRecord`] for that.
    pub fn verify_bytes(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        bytes: &[u8],
    ) -> Result<VerifyOutcome, ChecksumError> {
        let Some(expected) = self.stored(book_id, kind, key)? else {
            return Ok(VerifyOutcome::NoRecord);
        };
        let actual = blake3::hash(bytes).to_hex().to_string();
        if actual == expected {
            Ok(VerifyOutcome::Match)
        } else {
            Err(ChecksumError::Mismatch {
                book_id,
                kind: kind.to_string(),
                key: key.to_string(),
                expected,
                actual,
            })
        }
    }

    pub fn verify_file(
        &self,
        book_id: i32,
        kind: &str,
        key: &str,
        path: &Path,
    ) -> Result<VerifyOutcome, ChecksumError> {
        let bytes = fs::read(path)?;
        self.verify_bytes(book_id, kind, key, &bytes)
    }

    /// Which recorded files have this content (#890).
    ///
    /// The store is keyed `(book_id, kind, key)`, which answers "what is
    /// book 12's hash?" but not "which book is this file?" -- and the
    /// second is the question rename recovery asks. A file found at an
    /// unexpected path is identified by hashing it and looking the hash
    /// up here; a hit means the file moved rather than a book losing its
    /// file, and it can be re-attached without troubling the user.
    ///
    /// Returns `(book_id, kind, key)` per match. More than one match is
    /// normal and meaningful: two books can genuinely hold byte-identical
    /// files, which is a duplicate to resolve rather than an error.
    pub fn find_by_hash(&self, blake3_hex: &str) -> Result<Vec<(i32, String, String)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT book_id, kind, key FROM checksums_db.file_checksums WHERE blake3_hex = ?1 ORDER BY book_id, kind, key")?;
        let rows = stmt.query_map([blake3_hex], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Every set of books holding a byte-identical file, as
    /// `(hash, book ids)`.
    ///
    /// One `GROUP BY` rather than a hash lookup per book: a library-wide
    /// duplicate scan asks this once, and doing it per book would be a
    /// query per book for an answer the database can produce in a single
    /// pass.
    ///
    /// `kind` selects what is compared -- `"format"` for book files,
    /// `"cover"` for cover images. Comparing across kinds would pair a
    /// book whose cover happens to match another book's format file,
    /// which is meaningless.
    pub fn books_sharing_content(&self, kind: &str) -> Result<Vec<(String, Vec<i32>)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT blake3_hex, book_id FROM checksums_db.file_checksums
             WHERE kind = ?1 AND size > 0
               AND blake3_hex IN (
                   SELECT blake3_hex FROM checksums_db.file_checksums
                   WHERE kind = ?1 AND size > 0 GROUP BY blake3_hex HAVING COUNT(DISTINCT book_id) > 1
               )
             ORDER BY blake3_hex, book_id",
        )?;
        let rows = stmt.query_map([kind], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?)))?;

        let mut groups: Vec<(String, Vec<i32>)> = Vec::new();
        for row in rows {
            let (hash, book_id) = row?;
            match groups.last_mut() {
                Some((current, ids)) if *current == hash => {
                    // A book with two formats of identical content
                    // appears twice for one hash; it is one book, not a
                    // duplicate of itself.
                    if !ids.contains(&book_id) {
                        ids.push(book_id);
                    }
                }
                _ => groups.push((hash, vec![book_id])),
            }
        }
        groups.retain(|(_, ids)| ids.len() > 1);
        Ok(groups)
    }

    /// `(book id, format key)` of every recorded file of size 0.
    ///
    /// Empty files all share one hash (the BLAKE3 of nothing), so they are
    /// kept **out** of [`Self::books_sharing_content`]: two empty files are
    /// not the same book. They are reported on their own instead.
    pub fn empty_files(&self, kind: &str) -> Result<Vec<(i32, String)>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT book_id, key FROM checksums_db.file_checksums WHERE kind = ?1 AND size = 0 ORDER BY book_id, key")?;
        let rows = stmt.query_map([kind], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// What the importer recorded for a source file, if anything.
    pub fn import_ledger_lookup(&self, source: &str) -> Result<Option<LedgerRow>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT blake3_hex, size, mtime_ns, outcome, policy FROM checksums_db.import_ledger WHERE source = ?1")?;
        let mut rows = stmt.query([source])?;
        match rows.next()? {
            Some(row) => Ok(Some(LedgerRow { hash: row.get(0)?, size: row.get(1)?, mtime_ns: row.get(2)?, outcome: row.get(3)?, policy: row.get(4)? })),
            None => Ok(None),
        }
    }

    /// Records the importer's decisions for a batch of source files, in one
    /// `BEGIN IMMEDIATE` transaction.
    pub fn import_ledger_record(&self, rows: &[(String, LedgerRow, Option<String>)]) -> Result<(), ChecksumError> {
        if rows.is_empty() {
            return Ok(());
        }
        self.initialize()?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (source, row, uuid) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO checksums_db.import_ledger (source, blake3_hex, size, mtime_ns, outcome, policy, book_uuid, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
                (source, &row.hash, row.size, row.mtime_ns, &row.outcome, &row.policy, uuid),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether any recorded file has this content.
    ///
    /// Cheaper than [`ChecksumStore::find_by_hash`] when the answer is
    /// only being used to decide whether a hash lookup is worth doing at
    /// all.
    pub fn any_with_hash(&self, blake3_hex: &str) -> Result<bool, ChecksumError> {
        Ok(!self.find_by_hash(blake3_hex)?.is_empty())
    }

    /// Drops one file's stored checksum -- call when the file it
    /// describes is removed, so the store doesn't keep asserting a
    /// hash for something that no longer exists.
    pub fn remove(&self, book_id: i32, kind: &str, key: &str) -> Result<(), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM checksums_db.file_checksums
             WHERE book_id = ?1 AND kind = ?2 AND key = ?3",
            (book_id, kind, key),
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // PDF fingerprints (`pdf_similarity`)
    //
    // A cache of something computed from a file's content, kept in the
    // sidecar with the hashes it is keyed by. It is not library metadata, so
    // none of this goes through `Cache` or the change log (the same as the
    // hashes themselves), and a library rebuilt from the log just recomputes
    // it. Rows are keyed by content, so deleting a book leaves its row: the
    // next book with that content reuses it.
    // ------------------------------------------------------------------

    /// PDF formats with a content hash and no current-version fingerprint
    /// (neither a good one nor a recorded failure).
    pub fn pdf_fingerprints_pending(&self) -> Result<Vec<PendingPdf>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.book_id, c.key, c.blake3_hex, d.name, b.path
             FROM checksums_db.file_checksums c
             JOIN data d ON d.book = c.book_id AND d.format = c.key
             JOIN books b ON b.id = c.book_id
             WHERE c.kind = 'format' AND UPPER(c.key) = 'PDF'
               AND NOT EXISTS (SELECT 1 FROM checksums_db.pdf_fingerprints p WHERE p.blake3_hex = c.blake3_hex AND p.version = ?1)
             ORDER BY c.blake3_hex, c.book_id",
        )?;
        let rows = stmt.query_map([crate::pdf_similarity::FINGERPRINT_VERSION], |row| {
            Ok(PendingPdf { book_id: row.get(0)?, format: row.get(1)?, hash: row.get(2)?, name: row.get(3)?, folder: row.get(4)? })
        })?;
        Ok(rows.collect::<SqlResult<Vec<_>>>()?)
    }

    /// Stores a fingerprint, or the reason there is none (so it is not
    /// retried until the version changes).
    pub fn record_pdf_fingerprint(&self, hash: &str, outcome: &Result<crate::pdf_similarity::PdfFingerprint, String>) -> Result<(), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let version = crate::pdf_similarity::FINGERPRINT_VERSION;
        match outcome {
            Ok(fp) => conn.execute(
                "INSERT OR REPLACE INTO checksums_db.pdf_fingerprints
                 (blake3_hex, version, status, error, page_count, has_text, text_chars, pages, minhash, computed_at)
                 VALUES (?1, ?2, 'ok', NULL, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
                rusqlite::params![hash, version, fp.page_count, fp.has_text, fp.text_chars, fp.encode_pages(), fp.encode_minhash()],
            )?,
            Err(why) => conn.execute(
                "INSERT OR REPLACE INTO checksums_db.pdf_fingerprints (blake3_hex, version, status, error, computed_at)
                 VALUES (?1, ?2, 'failed', ?3, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
                rusqlite::params![hash, version, why],
            )?,
        };
        Ok(())
    }

    /// Whether this content has a current-version row (good or failed).
    pub fn has_pdf_fingerprint(&self, hash: &str) -> Result<bool, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT 1 FROM checksums_db.pdf_fingerprints WHERE blake3_hex = ?1 AND version = ?2",
                rusqlite::params![hash, crate::pdf_similarity::FINGERPRINT_VERSION],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The recorded failure reason for this content, if it failed.
    pub fn pdf_fingerprint_error(&self, hash: &str) -> Result<Option<String>, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT error FROM checksums_db.pdf_fingerprints WHERE blake3_hex = ?1 AND version = ?2 AND status = 'failed'",
                rusqlite::params![hash, crate::pdf_similarity::FINGERPRINT_VERSION],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Every good, current fingerprint with the books that hold that content.
    pub fn pdf_fingerprints(&self) -> Result<Vec<crate::pdf_similarity::LibraryFingerprint>, ChecksumError> {
        use crate::pdf_similarity::{LibraryFingerprint, PdfFingerprint};
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut books: std::collections::HashMap<String, (Vec<i32>, i64)> = std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT c.blake3_hex, c.book_id, c.size FROM checksums_db.file_checksums c
                 JOIN data d ON d.book = c.book_id AND d.format = c.key
                 WHERE c.kind = 'format' AND UPPER(c.key) = 'PDF' ORDER BY c.book_id",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?, r.get::<_, i64>(2)?)))? {
                let (hash, book, size) = row?;
                let entry = books.entry(hash).or_default();
                entry.0.push(book);
                entry.1 = size;
            }
        }
        let mut stmt = conn.prepare("SELECT blake3_hex, page_count, has_text, text_chars, pages, minhash FROM checksums_db.pdf_fingerprints WHERE status = 'ok' AND version = ?1 ORDER BY blake3_hex")?;
        let mut out = Vec::new();
        for row in stmt.query_map([crate::pdf_similarity::FINGERPRINT_VERSION], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, bool>(2)?, r.get::<_, u32>(3)?, r.get::<_, Vec<u8>>(4)?, r.get::<_, Vec<u8>>(5)?))
        })? {
            let (hash, page_count, has_text, text_chars, pages, minhash) = row?;
            let Some(pages) = PdfFingerprint::decode_pages(&pages) else { continue };
            let Some((ids, size)) = books.get(&hash) else { continue };
            out.push(LibraryFingerprint { books: ids.clone(), size: *size, fingerprint: PdfFingerprint { page_count, pages, text_chars, has_text, minhash: PdfFingerprint::decode_minhash(&minhash) }, hash });
        }
        Ok(out)
    }

    /// How many of the library's PDF formats are fingerprinted, failed, owed
    /// a fingerprint, or have no hash to key one by.
    pub fn pdf_status(&self) -> Result<crate::pdf_similarity::PdfStatus, ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let version = crate::pdf_similarity::FINGERPRINT_VERSION;
        let count = |sql: &str| -> SqlResult<usize> { conn.query_row(sql, [version], |r| r.get::<_, i64>(0)).map(|n| n as usize) };
        let hashed_join = "FROM data d JOIN checksums_db.file_checksums c ON c.book_id = d.book AND c.kind = 'format' AND c.key = d.format";
        let fp_join = "JOIN checksums_db.pdf_fingerprints p ON p.blake3_hex = c.blake3_hex AND p.version = ?1";
        let fingerprinted = count(&format!("SELECT count(*) {hashed_join} {fp_join} WHERE UPPER(d.format) = 'PDF' AND p.status = 'ok'"))?;
        let failed = count(&format!("SELECT count(*) {hashed_join} {fp_join} WHERE UPPER(d.format) = 'PDF' AND p.status = 'failed'"))?;
        let hashed = count(&format!("SELECT count(*) {hashed_join} WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1"))?;
        let all = count("SELECT count(*) FROM data d WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1")?;
        Ok(crate::pdf_similarity::PdfStatus { fingerprinted, failed, pending: hashed.saturating_sub(fingerprinted + failed), unhashed: all.saturating_sub(hashed) })
    }


    /// Drops every stored checksum for a book -- call when the whole
    /// book (and its on-disk folder) is deleted.
    pub fn remove_all_for_book(&self, book_id: i32) -> Result<(), ChecksumError> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM checksums_db.file_checksums WHERE book_id = ?1",
            (book_id,),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn store(dir: &Path) -> ChecksumStore {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        ChecksumStore::new(conn, dir)
    }

    #[test]
    fn checksums_db_is_attached_in_wal_mode_with_synchronous_full() {
        // docs/FAULT_TOLERANCE.md §3 (issue #260): real, not just
        // documented -- query the pragmas back from the live
        // connection. Unqualified `PRAGMA journal_mode` only reports
        // `main` (here an in-memory test connection, which can't be
        // WAL at all), so this queries the attached schema by name.
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.initialize().unwrap();
        let conn = store.conn.lock().unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA checksums_db.journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
        let synchronous: i64 = conn
            .query_row("PRAGMA checksums_db.synchronous", [], |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 2);
    }

    #[test]
    fn recording_then_verifying_the_same_bytes_matches() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.record_bytes(1, "format", "EPUB", b"hello").unwrap();
        let outcome = store.verify_bytes(1, "format", "EPUB", b"hello").unwrap();
        assert_eq!(outcome, VerifyOutcome::Match);
    }

    #[test]
    fn verifying_changed_bytes_is_a_real_mismatch_error() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.record_bytes(1, "format", "EPUB", b"hello").unwrap();
        let err = store
            .verify_bytes(1, "format", "EPUB", b"goodbye")
            .unwrap_err();
        assert!(matches!(err, ChecksumError::Mismatch { book_id: 1, .. }));
    }

    #[test]
    fn verifying_with_no_recorded_checksum_is_no_record_not_a_pass_or_fail() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let outcome = store.verify_bytes(1, "format", "EPUB", b"hello").unwrap();
        assert_eq!(outcome, VerifyOutcome::NoRecord);
    }

    #[test]
    fn recording_again_overwrites_the_previous_hash() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.record_bytes(1, "cover", "", b"v1").unwrap();
        store.record_bytes(1, "cover", "", b"v2").unwrap();
        assert_eq!(
            store.verify_bytes(1, "cover", "", b"v2").unwrap(),
            VerifyOutcome::Match
        );
        assert!(store.verify_bytes(1, "cover", "", b"v1").is_err());
    }

    #[test]
    fn remove_clears_a_single_record() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.record_bytes(1, "format", "EPUB", b"a").unwrap();
        store.record_bytes(1, "format", "PDF", b"b").unwrap();
        store.remove(1, "format", "EPUB").unwrap();
        assert_eq!(
            store.verify_bytes(1, "format", "EPUB", b"a").unwrap(),
            VerifyOutcome::NoRecord
        );
        assert_eq!(
            store.verify_bytes(1, "format", "PDF", b"b").unwrap(),
            VerifyOutcome::Match
        );
    }

    #[test]
    fn remove_all_for_book_clears_every_kind() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        store.record_bytes(1, "format", "EPUB", b"a").unwrap();
        store.record_bytes(1, "cover", "", b"c").unwrap();
        store
            .record_bytes(2, "format", "EPUB", b"other book")
            .unwrap();

        store.remove_all_for_book(1).unwrap();

        assert_eq!(
            store.verify_bytes(1, "format", "EPUB", b"a").unwrap(),
            VerifyOutcome::NoRecord
        );
        assert_eq!(
            store.verify_bytes(1, "cover", "", b"c").unwrap(),
            VerifyOutcome::NoRecord
        );
        assert_eq!(
            store
                .verify_bytes(2, "format", "EPUB", b"other book")
                .unwrap(),
            VerifyOutcome::Match
        );
    }

    #[test]
    fn record_file_and_verify_file_round_trip_through_real_files() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let path = dir.path().join("book.epub");
        fs::write(&path, b"real file contents").unwrap();

        store.record_file(1, "format", "EPUB", &path).unwrap();
        assert_eq!(
            store.verify_file(1, "format", "EPUB", &path).unwrap(),
            VerifyOutcome::Match
        );

        fs::write(&path, b"corrupted!").unwrap();
        let err = store.verify_file(1, "format", "EPUB", &path).unwrap_err();
        assert!(matches!(err, ChecksumError::Mismatch { .. }));
    }
}

#[cfg(test)]
mod hash_lookup_tests {
    use super::*;
    use crate::cache::Cache;

    /// The `Cache` is returned rather than dropped: the store borrows
    /// its connection, so letting it go would close the database out
    /// from under every query below.
    fn store() -> (tempfile::TempDir, Cache, ChecksumStore) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let store = cache.checksums();
        (dir, cache, store)
    }

    #[test]
    fn a_recorded_hash_can_be_looked_up_by_content() {
        let (_dir, _cache, store) = store();
        store.record_hash(7, "format", "PDF", "abc123", 100).unwrap();

        assert_eq!(store.find_by_hash("abc123").unwrap(), vec![(7, "format".to_string(), "PDF".to_string())]);
        assert!(store.any_with_hash("abc123").unwrap());
    }

    #[test]
    fn an_unknown_hash_finds_nothing() {
        let (_dir, _cache, store) = store();
        store.record_hash(7, "format", "PDF", "abc123", 100).unwrap();
        assert!(store.find_by_hash("nothing-like-it").unwrap().is_empty());
        assert!(!store.any_with_hash("nothing-like-it").unwrap());
    }

    /// Two books genuinely holding byte-identical files is a duplicate
    /// to resolve, not an error -- so every match is returned rather
    /// than the first.
    #[test]
    fn identical_content_in_two_books_returns_both() {
        let (_dir, _cache, store) = store();
        store.record_hash(1, "format", "PDF", "same", 10).unwrap();
        store.record_hash(2, "format", "PDF", "same", 10).unwrap();

        assert_eq!(store.find_by_hash("same").unwrap().len(), 2);
    }

    #[test]
    fn a_covers_hash_is_distinguishable_from_a_formats() {
        let (_dir, _cache, store) = store();
        store.record_hash(1, "format", "PDF", "shared", 10).unwrap();
        store.record_hash(1, "cover", "", "shared", 10).unwrap();

        let found = store.find_by_hash("shared").unwrap();
        assert!(found.contains(&(1, "cover".to_string(), String::new())));
        assert!(found.contains(&(1, "format".to_string(), "PDF".to_string())));
    }

    /// The rename-recovery path: a file turns up at an unexpected place,
    /// gets hashed, and the hash says which book it belongs to.
    #[test]
    fn a_moved_file_is_identified_by_hashing_it() {
        let (dir, _cache, store) = store();
        let path = dir.path().join("wherever.pdf");
        std::fs::write(&path, b"%PDF-1.4 the real bytes").unwrap();
        store.record_file(42, "format", "PDF", &path).unwrap();

        // Simulating the scan: something unexpected is here, so hash it.
        let hash = blake3::hash(&std::fs::read(&path).unwrap()).to_hex().to_string();
        let found = store.find_by_hash(&hash).unwrap();
        assert_eq!(found, vec![(42, "format".to_string(), "PDF".to_string())]);
    }
}
