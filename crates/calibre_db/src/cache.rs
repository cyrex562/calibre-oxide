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
use crate::fields::FieldStore;
use calibre_ebooks::metadata::MetaInformation;
use calibre_utils::filenames::sanitize_file_name;
use rusqlite::{OptionalExtension, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

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
    field_cache: Mutex<Option<(i64, FieldStore)>>,
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
    /// `.calibre-oxide/checksums.db`, sharing this `Cache`'s connection
    /// -- issue #93 §8.
    pub fn checksums(&self) -> crate::checksums::ChecksumStore {
        crate::checksums::ChecksumStore::new(self.backend.conn.clone(), &self.backend.library_path)
    }

    /// `SELECT total_changes()` -- SQLite's built-in running count of
    /// every row inserted/updated/deleted on this connection since it
    /// was opened (a real SQL function, not just a C API -- no extra
    /// rusqlite wrapper needed). Used as a cheap, robust staleness
    /// check: *any* write through this connection, by *any* code path
    /// (this file's own methods, raw SQL elsewhere in the crate, test
    /// fixtures), moves this number, so there is no missed-invalidation
    /// class of bug to chase down call site by call site.
    fn total_changes(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap_or(0)
    }

    fn with_field_store<T>(&self, f: impl FnOnce(&FieldStore) -> T) -> Result<T> {
        let conn = self.backend.conn.lock().unwrap();
        let current = Self::total_changes(&conn);
        let mut guard = self.field_cache.lock().unwrap();
        let stale = match &*guard {
            Some((loaded_at, _)) => *loaded_at != current,
            None => true,
        };
        if stale {
            let store = FieldStore::load(&conn)?;
            *guard = Some((current, store));
        }
        Ok(f(&guard.as_ref().unwrap().1))
    }

    pub fn library_id(&self) -> String {
        self.backend.library_id().unwrap_or_default()
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
        match namespace {
            Some(ns) => self
                .backend
                .set_pref(&format!("namespaced:{ns}:{name}"), val),
            None => self.backend.set_pref(name, val),
        }
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
        self.with_field_store(|store| store.field_for(book_id, field_name))
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
        let author_name = metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        let author_folder = sanitize_file_name(author_name);
        let title_folder = sanitize_file_name(&metadata.title);
        let rel_path = Path::new(&author_folder).join(&title_folder);
        let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");

        let book_id = self.add_book_db_entry(metadata, &rel_path_str)?;

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

        Ok(book_id)
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
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction()?;

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
                        metadata.timestamp.unwrap_or_else(chrono::Utc::now).to_rfc3339(),
                        metadata.pubdate.unwrap_or_else(chrono::Utc::now).to_rfc3339(),
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
                        metadata.timestamp.unwrap_or_else(chrono::Utc::now).to_rfc3339(),
                        metadata.pubdate.unwrap_or_else(chrono::Utc::now).to_rfc3339(),
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
        let link_names: Vec<&str> = if metadata.authors.is_empty() {
            vec!["Unknown"]
        } else {
            metadata.authors.iter().map(|s| s.as_str()).collect()
        };
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

        tx.commit()?;
        Ok(book_id)
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
        if path_rel.is_empty() {
            anyhow::bail!("Book has no path");
        }
        let title = self.field_for(book_id, "title")?.unwrap_or_default();

        let book_dir = self.backend.library_path.join(&path_rel);

        // `format` is sanitized the same as `title` -- a caller that
        // derives it from untrusted input (e.g. an HTTP request body,
        // see `calibre_srv::cdb::set_fields`) must not be able to
        // embed a path separator here and write outside `book_dir`.
        let file_name = format!("{}.{}", sanitize_file_name(&title), sanitize_file_name(&format.to_lowercase()));
        let dest_path = book_dir.join(&file_name);
        if dest_path.exists() && !replace {
            return Ok(false);
        }

        // Port of issue #93's crate-wide write-path retrofit: real,
        // journaled, crash-safe, large-file-safe copy-in through
        // `LibraryHandle` instead of a raw `fs::copy` (creates
        // `book_dir` itself, no separate `create_dir_all` needed).
        // `copy_atomic` streams both its hashing and copying passes,
        // so this never buffers a whole book file in memory even for
        // a large audiobook/PDF.
        let hash = self
            .backend
            .write_handle()?
            .copy_atomic(source_path, &dest_path)?;
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
        conn.execute(
            "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
            (book_id, format.to_uppercase(), size, sanitize_file_name(&title)),
        )?;
        drop(conn);
        Ok(true)
    }

    /// Removes the first on-disk file matching `fmt`'s extension from
    /// the book's folder and its `data` table row, if any.
    pub fn remove_format(&self, book_id: i32, fmt: &str) -> anyhow::Result<()> {
        let path_rel = match self.field_for(book_id, "path")? {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(()),
        };
        let book_dir = self.backend.library_path.join(&path_rel);
        let target_ext = fmt.to_lowercase();

        if let Ok(entries) = fs::read_dir(&book_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext.to_lowercase() == target_ext {
                            // Port of issue #93's crate-wide write-path
                            // retrofit: real, journaled, crash-safe
                            // removal through `LibraryHandle` instead of
                            // a raw `fs::remove_file`.
                            self.backend.write_handle()?.remove_atomic(&path)?;
                            break;
                        }
                    }
                }
            }
        }

        // The file is gone -- drop its stale checksum record too, so
        // the sidecar store doesn't keep asserting a hash for a format
        // that no longer exists.
        self.checksums()
            .remove(book_id, "format", &fmt.to_uppercase())?;

        let conn = self.backend.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM data WHERE book = ?1 AND format = ?2",
            (book_id, fmt.to_uppercase()),
        )?;
        drop(conn);
        Ok(())
    }

    /// Deletes a book's row (the real schema's `books_delete_trg`
    /// cascades cleanup of every link/data/comments/identifiers row
    /// for it) and its on-disk folder.
    pub fn delete_book(&self, book_id: i32) -> anyhow::Result<()> {
        let path_rel = self.field_for(book_id, "path")?;
        self.checksums().remove_all_for_book(book_id)?;
        {
            let conn = self.backend.conn.lock().unwrap();
            conn.execute("DELETE FROM books WHERE id = ?1", (book_id,))?;
        }
        if let Some(rel_path) = path_rel {
            if !rel_path.is_empty() {
                let dir_path = self.backend.library_path.join(rel_path);
                if dir_path.exists() {
                    // A failed cleanup shouldn't undo the already-committed
                    // DB delete; warn and move on, same as the original
                    // `library.rs` behavior this replaces. Port of issue
                    // #93's crate-wide write-path retrofit: real,
                    // journaled, crash-safe removal through
                    // `LibraryHandle` instead of a raw `fs::remove_dir_all`.
                    match self.backend.write_handle() {
                        Ok(handle) => {
                            if let Err(e) = handle.remove_atomic(&dir_path) {
                                eprintln!("Warning: failed to delete directory {dir_path:?}: {e}");
                            }
                        }
                        Err(e) => {
                            eprintln!("Warning: failed to delete directory {dir_path:?}: {e}");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Moves a book's folder to match a new title/author and renames
    /// its non-special files (`cover.jpg`/`metadata.opf` are left
    /// alone) to match the new title, then updates the `path` column.
    /// A no-op if the book has no path yet, or the computed new folder
    /// is identical to the old one.
    ///
    /// Port of issue #93's crate-wide write-path retrofit: every
    /// individual rename/removal below goes through the real
    /// `LibraryHandle` (journaled, crash-safe, recovery-verified on
    /// the next open) instead of a raw `fs::rename`/`fs::remove_dir`.
    ///
    /// On [`crate::library_handle::StorageTier::Network`] (issue
    /// #257's remaining scope), the directory rename, every file's own
    /// rename, and the empty-old-directory cleanup are staged and
    /// committed as one real two-phase batch via
    /// [`crate::library_handle::LibraryHandle::begin_network_batch`] --
    /// a crash mid-batch is recovered by finishing whichever steps
    /// hadn't completed yet, not left as an inconsistent intermediate
    /// state. On every other tier, each rename/removal below is its
    /// own independently-recoverable `LibraryHandle` call, same as
    /// before -- a crash between two of them can still leave a book
    /// with its directory moved but not every file inside renamed yet,
    /// same window this method always had on local storage (a multi-
    /// operation journal transaction spanning several `LibraryHandle`
    /// calls doesn't exist for the local-tier path, unlike the network
    /// one). On **every** tier, the DB `path` update at the end is
    /// still its own separate step after the file-level work
    /// succeeds -- `LibraryHandle` has no connection to the SQLite
    /// database at all (issue #260's territory), so a crash between a
    /// successful rename/batch and the DB update is a real, smaller,
    /// separate gap not addressed here.
    fn rename_book_files(
        &self,
        book_id: i32,
        new_title: &str,
        new_author: &str,
    ) -> anyhow::Result<()> {
        let old_rel_path = self.field_for(book_id, "path")?.unwrap_or_default();
        if old_rel_path.is_empty() {
            return Ok(());
        }
        let library_path = &self.backend.library_path;
        let old_full_dir = library_path.join(&old_rel_path);
        if !old_full_dir.exists() {
            return Ok(());
        }

        let new_author_folder = sanitize_file_name(new_author);
        let new_title_folder = sanitize_file_name(new_title);
        let new_rel_path = Path::new(&new_author_folder).join(&new_title_folder);
        let new_full_dir = library_path.join(&new_rel_path);

        if old_full_dir == new_full_dir {
            return Ok(());
        }

        let new_author_full_path = library_path.join(&new_author_folder);
        if !new_author_full_path.exists() {
            fs::create_dir_all(&new_author_full_path)?;
        }

        let handle = self.backend.write_handle()?;

        if handle.tier() == crate::library_handle::StorageTier::Network {
            // Issue #257's remaining scope: §6's own named example is
            // exactly this operation (a book move = a directory rename
            // plus each of its files' own rename). Every step has to be
            // discovered and staged *before* anything moves -- listing
            // `new_full_dir`'s contents (the local-tier approach just
            // below) only works because the directory rename has
            // already happened by the time it runs; on network storage
            // that would defeat the whole point of staging the batch
            // atomically up front. See `NetworkBatch`'s doc for the
            // full design.
            let mut batch = handle.begin_network_batch();
            batch.stage_rename(&old_full_dir, &new_full_dir);

            for entry in fs::read_dir(&old_full_dir)? {
                let entry = entry?;
                let old_path = entry.path();
                if old_path.is_file() {
                    if let Some(file_name) = old_path.file_name().and_then(|n| n.to_str()) {
                        if file_name == "cover.jpg" || file_name == "metadata.opf" {
                            continue;
                        }
                        if let Some(extension) = old_path.extension().and_then(|e| e.to_str()) {
                            let new_file_name =
                                format!("{}.{}", sanitize_file_name(new_title), extension);
                            // Where this file will be immediately after
                            // the directory-rename step above (which
                            // runs first within this same batch) -- not
                            // its current path under `old_full_dir`.
                            let post_dir_move_path = new_full_dir.join(file_name);
                            let new_file_path = new_full_dir.join(new_file_name);
                            if post_dir_move_path != new_file_path {
                                batch.stage_rename(&post_dir_move_path, &new_file_path);
                            }
                        }
                    }
                }
            }

            if let Some(parent) = old_full_dir.parent() {
                if parent.exists() && fs::read_dir(parent)?.count() == 1 {
                    // `old_full_dir` is the only entry under `parent`
                    // right now -- it'll be empty once the directory
                    // rename above takes effect, so stage its removal
                    // as the batch's final step.
                    batch.stage_remove(parent);
                }
            }

            batch.commit()?;
        } else {
            handle.rename_atomic(&old_full_dir, &new_full_dir)?;

            for entry in fs::read_dir(&new_full_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                        if file_name == "cover.jpg" || file_name == "metadata.opf" {
                            continue;
                        }
                        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
                            let new_file_name =
                                format!("{}.{}", sanitize_file_name(new_title), extension);
                            let new_file_path = new_full_dir.join(new_file_name);
                            if path != new_file_path {
                                if let Err(e) = handle.rename_atomic(&path, &new_file_path) {
                                    eprintln!(
                                        "Warning: failed to rename {path:?} to {new_file_path:?}: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
            }

            if let Some(parent) = old_full_dir.parent() {
                if parent.exists() && fs::read_dir(parent)?.next().is_none() {
                    if let Err(e) = handle.remove_atomic(parent) {
                        eprintln!("Warning: failed to remove empty directory {parent:?}: {e}");
                    }
                }
            }
        }

        let new_rel_path_str = new_rel_path.to_string_lossy().replace('\\', "/");
        let conn = self.backend.conn.lock().unwrap();
        conn.execute(
            "UPDATE books SET path = ?1 WHERE id = ?2",
            (&new_rel_path_str, book_id),
        )?;
        Ok(())
    }

    /// Renames the book's folder/files (via [`Cache::rename_book_files`])
    /// then updates `title`/`author_sort` and the `authors`/
    /// `books_authors_link` rows to match.
    pub fn update_book_metadata(
        &self,
        book_id: i32,
        title: &str,
        author: &str,
    ) -> anyhow::Result<()> {
        self.rename_book_files(book_id, title, author)?;

        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction()?;

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
                        .and_then(|s| calibre_utils::date::parse_date(s, true))
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
                    Some(
                        prefix
                            .join(&path_rel)
                            .join("cover.jpg")
                            .to_string_lossy()
                            .to_string(),
                    )
                } else {
                    None
                }),
            );

            let mut formats = Vec::new();
            let mut available_formats = Vec::new();
            let mut data_formats: Vec<String> = Vec::new();
            {
                let conn = self.backend.conn.lock().unwrap();
                let mut stmt = conn.prepare("SELECT format FROM data WHERE book = ?1")?;
                let rows = stmt.query_map([book_id], |row| row.get::<_, String>(0))?;
                for row in rows {
                    data_formats.push(row?);
                }
            }
            for fmt in &data_formats {
                available_formats.push(fmt.to_uppercase());
                let file_name = format!("{}.{}", sanitize_file_name(&title), fmt.to_lowercase());
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
        Ok(raw.as_deref().and_then(|s| calibre_utils::date::parse_date(s, true)).unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH))
    }

    /// `datatype` is one of `bool`/`int`/`float`/`rating` (one-to-one
    /// numeric value table) or `text`/`comments`/`series` (one-to-one
    /// text value table, non-`is_multiple` only -- see the error
    /// below); anything else falls back to a generic text value table,
    /// same as `Library`'s original behavior.
    pub fn add_custom_column(
        &self,
        label: &str,
        name: &str,
        datatype: &str,
        is_multiple: bool,
    ) -> anyhow::Result<i32> {
        let mut conn = self.backend.conn.lock().unwrap();

        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM custom_columns WHERE label = ?1",
            [label],
            |row| row.get(0),
        )?;
        if count > 0 {
            anyhow::bail!("Column with label '{}' already exists", label);
        }

        let tx = conn.transaction()?;

        tx.execute(
            "INSERT INTO custom_columns
            (label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized)
            VALUES (?1, ?2, ?3, 0, 1, '{}', ?4, 0)",
            (label, name, datatype, is_multiple),
        )?;
        let col_id = tx.last_insert_rowid() as i32;
        let table_name = format!("custom_column_{}", col_id);

        match datatype {
            "bool" | "int" | "float" | "rating" => {
                let value_type = if datatype == "float" || datatype == "rating" {
                    "REAL"
                } else {
                    "INTEGER"
                };
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value {value_type})"
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{col_id}_book ON {table_name} (book)"),
                    [],
                )?;
            }
            "text" | "comments" | "series" => {
                if is_multiple {
                    anyhow::bail!("Multiple-value text columns not yet supported in this port");
                }
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)"
                    ),
                    [],
                )?;
                tx.execute(
                    &format!("CREATE INDEX idx_{col_id}_book ON {table_name} (book)"),
                    [],
                )?;
            }
            _ => {
                tx.execute(
                    &format!(
                        "CREATE TABLE {table_name} (id INTEGER PRIMARY KEY, book INTEGER, value TEXT)"
                    ),
                    [],
                )?;
            }
        }

        tx.commit()?;
        Ok(col_id)
    }

    fn custom_column_lookup(&self, label: &str) -> Result<Option<(i32, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, datatype FROM custom_columns WHERE label = ?1",
            [label],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    }

    pub fn get_custom_column_value(&self, book_id: i32, label: &str) -> Result<Option<String>> {
        let Some((col_id, datatype)) = self.custom_column_lookup(label)? else {
            return Ok(None);
        };
        let table_name = format!("custom_column_{col_id}");
        let conn = self.backend.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT value FROM {table_name} WHERE book = ?1"),
            [book_id],
            |row| match datatype.as_str() {
                "int" | "bool" => row.get::<_, i32>(0).map(|v| v.to_string()),
                "float" | "rating" => row.get::<_, f64>(0).map(|v| v.to_string()),
                _ => row.get(0),
            },
        )
        .optional()
    }

    pub fn set_custom_column_value(
        &self,
        book_id: i32,
        label: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let Some((col_id, datatype)) = self.custom_column_lookup(label)? else {
            anyhow::bail!("Custom column with label '{}' not found", label);
        };
        let table_name = format!("custom_column_{col_id}");
        let conn = self.backend.conn.lock().unwrap();
        let sql = format!("INSERT OR REPLACE INTO {table_name} (book, value) VALUES (?1, ?2)");
        match datatype.as_str() {
            "bool" => conn.execute(
                &sql,
                (book_id, value.parse::<bool>().unwrap_or(false) as i32),
            ),
            "int" => conn.execute(&sql, (book_id, value.parse::<i32>().unwrap_or(0))),
            "float" | "rating" => {
                conn.execute(&sql, (book_id, value.parse::<f64>().unwrap_or(0.0)))
            }
            _ => conn.execute(&sql, (book_id, value)),
        }?;
        Ok(())
    }

    pub fn remove_custom_column(&self, label: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let col_id: Option<i32> = conn
            .query_row(
                "SELECT id FROM custom_columns WHERE label = ?1",
                [label],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = col_id else {
            anyhow::bail!("Column '{}' not found", label);
        };

        let tx = conn.transaction()?;
        tx.execute("DELETE FROM custom_columns WHERE id = ?1", [id])?;
        // `id` is an integer we control (from the `custom_columns` row
        // just looked up), not user input -- no injection risk from
        // building the table name via `format!`.
        tx.execute(&format!("DROP TABLE IF EXISTS custom_column_{id}"), [])?;
        tx.commit()?;
        Ok(())
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
        match field {
            "title" | "sort" | "uuid" if value.is_empty() => {
                // Matches upstream: these three fields silently no-op
                // on an empty value rather than clearing it.
            }
            "title" | "sort" | "author_sort" | "uuid" => {
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
        let tx = conn.transaction()?;
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
        tx.commit()?;
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
        let items: Vec<&str> = value
            .split(sep)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction()?;
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
        tx.commit()?;
        Ok(())
    }

    fn set_rating_field(&self, book_id: i32, value: &str) -> anyhow::Result<()> {
        let rating: i32 = value.parse().unwrap_or(0);
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction()?;
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
        tx.commit()?;
        Ok(())
    }

    fn set_identifiers_field(&self, book_id: i32, value: &str) -> anyhow::Result<()> {
        let mut conn = self.backend.conn.lock().unwrap();
        let tx = conn.transaction()?;
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
        tx.commit()?;
        Ok(())
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
    /// `calibre_srv`, which only holds an `Arc<Cache>`, can reach it
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
        let conn = self.backend.conn.lock().unwrap();
        conn.execute("INSERT OR REPLACE INTO preferences (key, val) VALUES (?1, ?2)", (key, val))?;
        Ok(())
    }

    /// Port of `is_fts_enabled` -- same as [`crate::library::Library::is_fts_enabled`].
    pub fn is_fts_enabled(&self) -> anyhow::Result<bool> {
        Ok(self.get_preference("fts.enabled")?.as_deref() == Some("true"))
    }

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

    #[test]
    fn all_book_ids_returns_every_book_in_id_order() {
        let (_dir, cache) = open_test_cache();
        assert_eq!(cache.all_book_ids().unwrap(), Vec::<i32>::new());
        let id1 = insert_book(&cache, "One");
        let id2 = insert_book(&cache, "Two");
        assert_eq!(cache.all_book_ids().unwrap(), vec![id1, id2]);
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
        let map = cache.custom_column_label_map().unwrap();
        let entry = map.get("mycol").unwrap();
        assert_eq!(entry["name"], "My Column");
        assert_eq!(entry["datatype"], "int");
        assert_eq!(entry["is_multiple"], true);
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
        let journal_dir = library_path.join(".calibre-oxide").join("journal");
        fs::read_dir(&journal_dir)
            .unwrap()
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
    fn add_book_creates_the_author_title_folder_and_copies_the_file() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");

        let mut meta = MetaInformation::default();
        meta.title = "My Title".to_string();
        meta.authors = vec!["My Author".to_string()];

        let book_id = cache.add_book(&source, &meta).unwrap();

        assert_eq!(
            cache.field_for(book_id, "path").unwrap(),
            Some("My Author/My Title".to_string())
        );
        let dest = dir.path().join("My Author/My Title/My Title.epub");
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
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
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
        assert!(!cache.add_format(book_id, &source2, "epub", false).unwrap());

        let dest = dir.path().join("A/T/T.epub");
        assert_eq!(fs::read(&dest).unwrap(), b"first");

        assert!(cache.add_format(book_id, &source2, "epub", true).unwrap());
        assert_eq!(fs::read(&dest).unwrap(), b"second");
    }

    #[test]
    fn add_format_sanitizes_a_path_traversal_extension() {
        // `format` can come from untrusted input (e.g. an HTTP request
        // body -- see `calibre_srv::cdb::set_fields`'s `added_formats`);
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
    fn remove_format_deletes_the_file_and_its_data_row() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        cache.add_format(book_id, &source, "epub", true).unwrap();

        cache.remove_format(book_id, "epub").unwrap();

        assert!(!dir.path().join("A/T/T.epub").exists());
        assert_eq!(cache.field_for(book_id, "formats").unwrap(), None);

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

        let dest = dir.path().join("A/T/T.epub");
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
    fn delete_book_removes_the_row_and_the_folder() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        assert!(dir.path().join("A/T").exists());

        cache.delete_book(book_id).unwrap();

        assert_eq!(cache.field_for(book_id, "title").unwrap(), None);
        assert!(!dir.path().join("A/T").exists());

        // delete_book now goes through the real LibraryHandle (issue
        // #93's crate-wide write-path retrofit), not a raw
        // `fs::remove_dir_all` -- prove it by checking a real
        // journaled `DeleteFile` entry landed (see the comment in
        // `remove_format_deletes_the_file_and_its_data_row` for why
        // this doesn't just count every `.op` file).
        assert_eq!(journaled_delete_count(dir.path()), 1);
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
    fn update_book_metadata_renames_the_folder_and_updates_the_author_link() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "src.epub", b"x");
        let mut meta = MetaInformation::default();
        meta.title = "Old Title".to_string();
        meta.authors = vec!["Old Author".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache
            .update_book_metadata(book_id, "New Title", "New Author")
            .unwrap();

        assert_eq!(
            cache.field_for(book_id, "title").unwrap(),
            Some("New Title".to_string())
        );
        assert_eq!(
            cache.field_for(book_id, "authors").unwrap(),
            Some("New Author".to_string())
        );
        assert!(dir.path().join("New Author/New Title").exists());
        assert!(!dir.path().join("Old Author").exists());

        // The book's own file was renamed to match the new title too,
        // not just the directory.
        assert!(dir
            .path()
            .join("New Author/New Title/New Title.epub")
            .exists());
        assert!(!dir
            .path()
            .join("New Author/New Title/Old Title.epub")
            .exists());

        // rename_book_files now goes through the real LibraryHandle
        // (issue #93's crate-wide write-path retrofit), not raw
        // `fs::rename`/`fs::remove_dir` calls -- prove it by checking
        // real journaled entries landed: one `RenameFile` for the
        // directory move, one more for the book file's own rename,
        // and one `DeleteFile` for the now-empty old author directory
        // (`add_book`'s own initial `add_format` also journals a
        // `WriteFile`, which is why this doesn't just count every
        // `.op` file -- see `journaled_op_count`'s doc comment).
        assert_eq!(journaled_op_count(dir.path(), "RenameFile"), 2);
        assert_eq!(journaled_delete_count(dir.path()), 1);
    }

    #[test]
    fn update_book_metadata_leaves_the_old_author_directory_when_another_book_still_uses_it() {
        let (dir, cache) = open_test_cache();

        let source_a = write_temp_file(dir.path(), "a.epub", b"a");
        let mut meta_a = MetaInformation::default();
        meta_a.title = "Book A".to_string();
        meta_a.authors = vec!["Shared Author".to_string()];
        let book_a = cache.add_book(&source_a, &meta_a).unwrap();

        let source_b = write_temp_file(dir.path(), "b.epub", b"b");
        let mut meta_b = MetaInformation::default();
        meta_b.title = "Book B".to_string();
        meta_b.authors = vec!["Shared Author".to_string()];
        cache.add_book(&source_b, &meta_b).unwrap();

        cache
            .update_book_metadata(book_a, "Book A", "Solo Author")
            .unwrap();

        // "Shared Author" is still not empty (Book B's directory is
        // still under it) -- the empty-parent cleanup must not remove
        // it.
        assert!(dir.path().join("Shared Author/Book B").exists());
        assert!(dir.path().join("Solo Author/Book A").exists());
    }

    #[test]
    fn rename_book_files_on_network_tier_uses_a_two_phase_batch() {
        let (dir, cache) = open_test_cache();

        // Force the cached write handle to report Network tier
        // *before* any real handle has ever been opened on this
        // `Backend` -- issue #257's remaining scope, the two-phase
        // multi-file batch case. Installing it first (rather than
        // after `add_book` already opened a real one) avoids a
        // release-then-immediately-reacquire sequence on the same
        // lock file, which a heavily parallel `cargo test` run can
        // make `try_lock()` spuriously report `WouldBlock` for (see
        // `flock_test_guard`'s own doc comment for the full
        // investigation this project already did into that).
        cache.backend.install_network_tier_handle_for_test();

        let source = write_temp_file(dir.path(), "src.epub", b"epub bytes");
        let mut meta = MetaInformation::default();
        meta.title = "Old Title".to_string();
        meta.authors = vec!["Old Author".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();

        cache
            .update_book_metadata(book_id, "New Title", "New Author")
            .unwrap();

        assert_eq!(
            cache.field_for(book_id, "title").unwrap(),
            Some("New Title".to_string())
        );
        assert!(dir.path().join("New Author/New Title").exists());
        assert!(!dir.path().join("Old Author").exists());
        assert!(dir
            .path()
            .join("New Author/New Title/New Title.epub")
            .exists());
        assert!(!dir
            .path()
            .join("New Author/New Title/Old Title.epub")
            .exists());

        // A real Batch entry landed (not a series of individual
        // RenameFile entries) -- proves the Network-tier branch
        // actually ran, not just that the end state happens to match.
        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let batch_entries: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .filter(|e| {
                fs::read_to_string(e.path())
                    .map(|content| content.contains("Batch"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(batch_entries.len(), 1, "expected exactly one Batch entry");
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

        assert!(dest_dir.path().join("A/T/T.epub").exists());
    }

    // --- get_data_as_dict ---

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
        assert!(formats[0].as_str().unwrap().ends_with("T.epub"));
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
}
