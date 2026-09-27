//! Books whose files have gone (issue #895, part of #889).
//!
//! An orphan is a database entry with no file behind it — deleted, or
//! moved out of the library. In a folder the user manages by hand that
//! is an expected state, not an error, and it is the only one
//! [`crate::drift`] cannot resolve on its own: a rename or a move inside
//! the library is *proved* and re-attached silently, so anything left is
//! genuinely lost.
//!
//! # Why the flag is durable
//!
//! Recomputing orphan status on every scan would be cheaper and wrong.
//! An orphan is exactly the thing a user needs to still be there
//! tomorrow: they see it, decide to deal with it later, close the app —
//! and a recomputed list is gone on restart, or worse, quietly comes
//! back each time without them ever being able to say "yes, I know".
//!
//! # Where it lives
//!
//! In the sidecar database, not in `books`. Orphan status is this app's
//! own bookkeeping rather than book metadata, and adding a column to
//! `books` would diverge the schema from calibre's for something that is
//! not a property of the book at all.
//!
//! It shares the one sidecar file (attached as `checksums_db`) rather
//! than opening a second: the schema name is narrower than what it now
//! holds, but a second attachment means another file, another set of
//! journal pragmas and another thing to keep in step, for no gain.
//!
//! # Only ever from a conclusive scan
//!
//! [`reconcile`] refuses a [`DriftReport`] that is not
//! [`DriftReport::conclusive`]. A scan that could not read a directory
//! cannot tell "gone" from "not visible right now", and writing the
//! second down as the first is how a dropped network share becomes a
//! library full of orphans the user then has to clear by hand.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::cache::Cache;
use crate::drift::DriftReport;

/// A book entry with no file behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub book_id: i32,
    pub format: String,
    /// Where the file was last known to be, relative to the library
    /// root. The starting point for looking, and worth showing: "it used
    /// to be in Receipts/" is often all somebody needs.
    pub last_known_path: String,
    pub noticed_at: String,
}

/// Whether a file the user picked is the one that was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocateOutcome {
    /// Byte-identical to what was recorded. Certainly the same book.
    ContentMatches,
    /// A different file. **Allowed** — the user may be supplying a
    /// re-downloaded copy, a different edition, or a repaired scan — but
    /// said out loud rather than accepted silently, because the other
    /// possibility is that they picked the wrong file.
    ContentDiffers,
    /// Nothing was recorded to compare against.
    NothingToCompare,
}

pub struct OrphanStore {
    conn: Arc<Mutex<Connection>>,
    library_path: PathBuf,
}

impl OrphanStore {
    pub fn new(conn: Arc<Mutex<Connection>>, library_path: &Path) -> Self {
        OrphanStore { conn, library_path: library_path.to_path_buf() }
    }

    /// Ensures the sidecar is attached and the table exists.
    ///
    /// Delegates the attach to [`crate::checksums::ChecksumStore`] so
    /// there is exactly one place that knows how the sidecar is opened
    /// and what pragmas it needs.
    fn initialize(&self) -> Result<()> {
        crate::checksums::ChecksumStore::new(self.conn.clone(), &self.library_path).initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checksums_db.orphans (
                book_id INTEGER NOT NULL,
                format TEXT NOT NULL,
                last_known_path TEXT NOT NULL,
                noticed_at TEXT NOT NULL,
                PRIMARY KEY (book_id, format)
            );",
        )?;
        Ok(())
    }

    /// Records that a book's file is gone.
    ///
    /// Idempotent, and deliberately keeps the *original* `noticed_at`:
    /// how long something has been missing is more useful than when it
    /// was last looked at, and refreshing the timestamp on every scan
    /// would erase that.
    pub fn mark(&self, book_id: i32, format: &str, last_known_path: &str) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checksums_db.orphans (book_id, format, last_known_path, noticed_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(book_id, format) DO UPDATE SET last_known_path = excluded.last_known_path",
            (book_id, format.to_uppercase(), last_known_path),
        )?;
        Ok(())
    }

    /// Forgets that a book was ever an orphan — its file is back.
    pub fn clear(&self, book_id: i32, format: &str) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checksums_db.orphans WHERE book_id = ?1 AND format = ?2", (book_id, format.to_uppercase()))?;
        Ok(())
    }

    pub fn clear_all_for_book(&self, book_id: i32) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checksums_db.orphans WHERE book_id = ?1", (book_id,))?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Orphan>> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT book_id, format, last_known_path, noticed_at FROM checksums_db.orphans ORDER BY noticed_at, book_id, format")?;
        let rows = stmt.query_map([], |row| {
            Ok(Orphan { book_id: row.get(0)?, format: row.get(1)?, last_known_path: row.get(2)?, noticed_at: row.get(3)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn is_orphan(&self, book_id: i32, format: &str) -> Result<bool> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT 1 FROM checksums_db.orphans WHERE book_id = ?1 AND format = ?2")?;
        Ok(stmt.exists((book_id, format.to_uppercase()))?)
    }

    /// Book ids with at least one orphaned format, for a
    /// "show me what is broken" filter.
    pub fn orphaned_book_ids(&self) -> Result<Vec<i32>> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT book_id FROM checksums_db.orphans ORDER BY book_id")?;
        let rows = stmt.query_map([], |row| row.get::<_, i32>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct OrphanChanges {
    pub newly_orphaned: Vec<(i32, String)>,
    pub resolved: Vec<(i32, String)>,
}

/// Brings the orphan list into line with a drift report.
///
/// Refuses an inconclusive report: see this module's docs. Returning an
/// error rather than doing nothing quietly, because a caller that wanted
/// to reconcile and could not has something to tell the user.
pub fn reconcile(cache: &Cache, report: &DriftReport) -> Result<OrphanChanges> {
    if !report.conclusive {
        anyhow::bail!("refusing to orphan anything from an incomplete scan: a directory could not be read, so a missing file cannot be told from an unreadable one");
    }

    let store = cache.orphans();
    let mut changes = OrphanChanges::default();

    let recorded_paths = missing_paths(cache, &report.missing)?;
    for (book_id, format) in &report.missing {
        let last_known = recorded_paths.get(&(*book_id, format.clone())).cloned().unwrap_or_default();
        if !store.is_orphan(*book_id, format)? {
            changes.newly_orphaned.push((*book_id, format.clone()));
        }
        store.mark(*book_id, format, &last_known)?;
    }

    // Anything the scan *did* account for is no longer an orphan: it was
    // found where it should be, edited in place, or matched somewhere
    // else. All three mean there is a file again.
    let missing: std::collections::HashSet<(i32, String)> = report.missing.iter().cloned().collect();
    for orphan in store.list()? {
        if !missing.contains(&(orphan.book_id, orphan.format.clone())) {
            store.clear(orphan.book_id, &orphan.format)?;
            changes.resolved.push((orphan.book_id, orphan.format));
        }
    }
    Ok(changes)
}

/// Where each missing `(book, format)` was last recorded.
fn missing_paths(cache: &Cache, missing: &[(i32, String)]) -> Result<std::collections::HashMap<(i32, String), String>> {
    let mut out = std::collections::HashMap::new();
    for (book_id, format) in missing {
        let folder = cache.field_for(*book_id, "path")?.unwrap_or_default();
        if let Some((_, name)) = cache.format_file_names(*book_id)?.into_iter().find(|(f, _)| f.eq_ignore_ascii_case(format)) {
            let file = format!("{name}.{}", format.to_lowercase());
            let path = if folder.is_empty() { file } else { format!("{folder}/{file}") };
            out.insert((*book_id, format.clone()), path);
        }
    }
    Ok(out)
}

/// Points an orphaned book at a file the user chose.
///
/// The chosen file must be **inside the library**: a book whose file
/// lives outside it is not something this model can track, and accepting
/// one would record a path that breaks the moment the library is copied
/// anywhere. Rejected with a message rather than silently copying the
/// file in, because copying somebody's file without being asked is worse
/// than refusing.
///
/// Returns whether the content matches what was recorded. A mismatch is
/// **not** an error — the user may be supplying a re-downloaded copy —
/// but the caller is expected to say so.
pub fn relocate(cache: &Cache, book_id: i32, format: &str, chosen: &Path) -> Result<RelocateOutcome> {
    let library = cache.backend.library_path.canonicalize().with_context(|| format!("resolving {}", cache.backend.library_path.display()))?;
    let chosen_real = chosen.canonicalize().with_context(|| format!("resolving {}", chosen.display()))?;
    if !chosen_real.starts_with(&library) {
        anyhow::bail!("{} is outside the library; copy it in first, then point at the copy", chosen.display());
    }
    if !chosen_real.is_file() {
        anyhow::bail!("{} is not a file", chosen.display());
    }

    let relative = chosen_real.strip_prefix(&library)?.to_string_lossy().replace('\\', "/");
    let path = Path::new(&relative);
    let folder = path.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();

    let bytes = std::fs::read(&chosen_real)?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let (recorded_hash, _) = cache.checksums().recorded_identity(book_id, "format", &format.to_uppercase()).unwrap_or((None, None));
    let outcome = match recorded_hash {
        Some(recorded) if recorded == hash => RelocateOutcome::ContentMatches,
        Some(_) => RelocateOutcome::ContentDiffers,
        None => RelocateOutcome::NothingToCompare,
    };

    cache.set_book_path(book_id, &folder)?;
    cache.record_format_row(book_id, format, &stem, bytes.len() as i64, Some(&hash))?;
    // Re-record the identity too, so the next rename of this file is
    // detectable from a `stat` rather than a full read.
    cache
        .checksums()
        .record_hash_with_identity(book_id, "format", &format.to_uppercase(), &hash, bytes.len() as i64, calibre_utils::filenames::identity(&chosen_real))?;
    cache.orphans().clear(book_id, format)?;
    Ok(outcome)
}

/// Removes an orphan's book entry, optionally saving its metadata first.
///
/// The metadata is the expensive part, not the file: a book can be
/// re-downloaded, but hand-entered tags, ratings and reading progress
/// cannot be re-typed. So the export happens **before** the delete and a
/// failure to write it aborts the whole thing — losing both would be the
/// one outcome nobody wants.
pub fn forget(cache: &Cache, book_id: i32, keep_metadata_at: Option<&Path>) -> Result<()> {
    if let Some(destination) = keep_metadata_at {
        let opf = metadata_opf(cache, book_id)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(destination, opf).with_context(|| format!("saving metadata to {}", destination.display()))?;
    }
    cache.orphans().clear_all_for_book(book_id)?;
    cache.delete_book(book_id)?;
    Ok(())
}

/// One book's metadata as an OPF document.
fn metadata_opf(cache: &Cache, book_id: i32) -> Result<String> {
    use calibre_ebooks::metadata::MetaInformation;

    let mut mi = MetaInformation::default();
    mi.title = cache.field_for(book_id, "title")?.unwrap_or_default();
    mi.authors = cache.field_for(book_id, "authors")?.map(|a| a.split(" & ").map(str::to_string).collect()).unwrap_or_default();
    mi.uuid = cache.book_uuid(book_id)?;
    mi.comments = cache.field_for(book_id, "comments")?;
    mi.publisher = cache.field_for(book_id, "publisher")?;
    mi.series = cache.field_for(book_id, "series")?;
    mi.tags = cache.field_for(book_id, "tags")?.map(|t| t.split(", ").map(str::to_string).collect()).unwrap_or_default();
    mi.languages = cache.field_for(book_id, "languages")?.map(|l| l.split(", ").map(str::to_string).collect()).unwrap_or_default();

    // No manifest, spine, guide, NCX or cover: this is a metadata-only
    // document, not a package describing a book's files -- the files are
    // exactly what is gone.
    Ok(calibre_ebooks::opf_writer::write_opf(&mi, &[], &[], &[], None, None, None, None, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
    use std::time::{Duration, SystemTime};

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn scan(dir: &Path) -> crate::scan::ScanReport {
        walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap()
    }

    fn index(dir: &Path, cache: &Cache) {
        let report = scan(dir);
        crate::scan::index_scan(cache, dir, &report);
    }

    fn drift_and_reconcile(dir: &Path, cache: &Cache) -> OrphanChanges {
        let report = crate::drift::detect(cache, &scan(dir)).unwrap();
        reconcile(cache, &report).unwrap()
    }

    #[test]
    fn a_deleted_file_becomes_an_orphan() {
        let (dir, cache) = library();
        write(&dir.path().join("Receipts/invoice.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("Receipts/invoice.pdf")).unwrap();

        let changes = drift_and_reconcile(dir.path(), &cache);
        assert_eq!(changes.newly_orphaned.len(), 1, "{changes:?}");

        let orphans = cache.orphans().list().unwrap();
        assert_eq!(orphans.len(), 1);
        // Worth showing: "it used to be in Receipts/" is often all
        // somebody needs to find it again.
        assert_eq!(orphans[0].last_known_path, "Receipts/invoice.pdf");
    }

    /// E21. The whole reason the flag is durable: a user sees an orphan,
    /// decides to deal with it later, and closes the app.
    #[test]
    fn an_orphan_survives_reopening_the_library() {
        let dir = tempfile::tempdir().unwrap();
        {
            let cache = Cache::new(dir.path()).unwrap();
            write(&dir.path().join("a.pdf"), b"%PDF content");
            index(dir.path(), &cache);
            std::fs::remove_file(dir.path().join("a.pdf")).unwrap();
            drift_and_reconcile(dir.path(), &cache);
        }

        let reopened = Cache::new(dir.path()).unwrap();
        assert_eq!(reopened.orphans().list().unwrap().len(), 1, "the orphan list reset on restart");
    }

    /// E17, once more. A scan that could not read a directory must not
    /// produce orphans -- and saying so is better than quietly doing
    /// nothing, because the caller asked for something it did not get.
    #[test]
    fn reconciling_an_inconclusive_scan_is_refused() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        let inconclusive = crate::drift::DriftReport { missing: vec![(1, "PDF".into())], conclusive: false, ..Default::default() };
        let err = reconcile(&cache, &inconclusive).unwrap_err().to_string();
        assert!(err.contains("incomplete scan"), "got: {err}");
        assert!(cache.orphans().list().unwrap().is_empty(), "an incomplete scan orphaned something");
    }

    /// A rename is not a loss -- drift matches it, so reconcile sees no
    /// missing file and nothing is ever marked.
    #[test]
    fn a_renamed_file_never_becomes_an_orphan() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("b.pdf")).unwrap();

        let changes = drift_and_reconcile(dir.path(), &cache);
        assert!(changes.newly_orphaned.is_empty(), "{changes:?}");
        assert!(cache.orphans().list().unwrap().is_empty());
    }

    #[test]
    fn putting_the_file_back_clears_the_orphan() {
        let (dir, cache) = library();
        let path = dir.path().join("a.pdf");
        write(&path, b"%PDF content");
        index(dir.path(), &cache);

        std::fs::remove_file(&path).unwrap();
        drift_and_reconcile(dir.path(), &cache);
        assert_eq!(cache.orphans().list().unwrap().len(), 1);

        // The second of the three resolutions: put it back and rescan.
        write(&path, b"%PDF content");
        let changes = drift_and_reconcile(dir.path(), &cache);
        assert_eq!(changes.resolved.len(), 1, "{changes:?}");
        assert!(cache.orphans().list().unwrap().is_empty());
    }

    #[test]
    fn marking_twice_keeps_the_original_noticed_at() {
        let (_dir, cache) = library();
        let store = cache.orphans();
        store.mark(1, "PDF", "a.pdf").unwrap();
        let first = store.list().unwrap()[0].noticed_at.clone();
        store.mark(1, "PDF", "a.pdf").unwrap();
        // How long something has been missing is more useful than when
        // it was last looked at.
        assert_eq!(store.list().unwrap()[0].noticed_at, first);
    }

    // ---- relocate ----

    #[test]
    fn relocating_to_the_same_file_reports_a_content_match() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        // Moved somewhere the scan cannot see it, so it orphans, then
        // pointed at by hand.
        let hidden = dir.path().join("elsewhere/a.pdf");
        write(&hidden, b"%PDF content");
        std::fs::remove_file(dir.path().join("a.pdf")).unwrap();
        drift_and_reconcile(dir.path(), &cache);

        let outcome = relocate(&cache, 1, "PDF", &hidden).unwrap();
        assert_eq!(outcome, RelocateOutcome::ContentMatches);
        assert!(cache.orphans().list().unwrap().is_empty());

        // And the book resolves to the file that was chosen.
        let row = cache.get_data_as_dict(None, false, None, false).unwrap()[0].clone();
        assert_eq!(Path::new(row["fmt_pdf"].as_str().unwrap()), hidden);
    }

    /// Allowed, but said out loud: the user may be supplying a
    /// re-downloaded copy, or may have picked the wrong file.
    #[test]
    fn relocating_to_a_different_file_is_allowed_but_reported() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF original");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("a.pdf")).unwrap();
        drift_and_reconcile(dir.path(), &cache);

        let replacement = dir.path().join("replacement.pdf");
        write(&replacement, b"%PDF a different scan entirely");
        assert_eq!(relocate(&cache, 1, "PDF", &replacement).unwrap(), RelocateOutcome::ContentDiffers);
        assert!(cache.orphans().list().unwrap().is_empty(), "it still resolves the orphan");
    }

    /// A path outside the library cannot be recorded: it would break the
    /// moment the library was copied anywhere. Refused rather than
    /// silently copied in, because copying somebody's file unasked is
    /// worse than saying no.
    #[test]
    fn relocating_to_a_file_outside_the_library_is_refused() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("a.pdf");
        write(&elsewhere, b"%PDF content");

        let err = relocate(&cache, 1, "PDF", &elsewhere).unwrap_err().to_string();
        assert!(err.contains("outside the library"), "got: {err}");
    }

    // ---- forget ----

    #[test]
    fn forgetting_an_orphan_removes_the_book() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("a.pdf")).unwrap();
        drift_and_reconcile(dir.path(), &cache);

        forget(&cache, 1, None).unwrap();
        assert!(cache.field_for(1, "title").unwrap().is_none());
        assert!(cache.orphans().list().unwrap().is_empty());
    }

    /// E22. The metadata is the expensive part -- a book can be
    /// re-downloaded, hand-entered tags and ratings cannot be re-typed.
    #[test]
    fn forgetting_can_keep_the_metadata_first() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        cache.set_field(1, "title", "A Carefully Entered Title").unwrap();
        cache.set_field(1, "tags", "reference, heating").unwrap();

        let kept = dir.path().join("kept/a.opf");
        forget(&cache, 1, Some(&kept)).unwrap();

        let opf = std::fs::read_to_string(&kept).unwrap();
        assert!(opf.contains("A Carefully Entered Title"), "{opf}");
        assert!(opf.contains("heating"), "{opf}");
        assert!(cache.field_for(1, "title").unwrap().is_none(), "the book should be gone");
    }

    /// The export happens first, and a failure aborts: losing the file
    /// *and* the metadata is the one outcome nobody wants.
    #[test]
    fn a_failed_metadata_export_does_not_delete_the_book() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        // A destination that cannot be written: an existing *file* stands
        // where the parent directory would have to be.
        let blocker = dir.path().join("blocked");
        write(&blocker, b"not a directory");
        let impossible = blocker.join("nested/a.opf");

        assert!(forget(&cache, 1, Some(&impossible)).is_err());
        assert!(cache.field_for(1, "title").unwrap().is_some(), "the book was deleted despite the export failing");
    }

    #[test]
    fn orphaned_book_ids_are_listed_for_a_filter() {
        let (_dir, cache) = library();
        let store = cache.orphans();
        store.mark(3, "PDF", "a.pdf").unwrap();
        store.mark(3, "EPUB", "a.epub").unwrap();
        store.mark(7, "PDF", "b.pdf").unwrap();
        // One entry per book, not per format: the filter shows books.
        assert_eq!(store.orphaned_book_ids().unwrap(), vec![3, 7]);
    }

    #[test]
    fn is_orphan_is_case_insensitive_about_the_format() {
        let (_dir, cache) = library();
        cache.orphans().mark(1, "pdf", "a.pdf").unwrap();
        assert!(cache.orphans().is_orphan(1, "PDF").unwrap());
        assert!(cache.orphans().is_orphan(1, "pdf").unwrap());
    }
}
