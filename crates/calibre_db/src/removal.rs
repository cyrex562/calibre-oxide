//! Removing a book, and remembering not to re-add it (issue #896, part
//! of #889).
//!
//! # Why "delete" needed deciding at all
//!
//! In calibre's model the app owns the file, so Delete removing it is
//! unambiguous. Once the library is a folder the user manages by hand
//! (#889), that file is *theirs* — and an app that erases it on a
//! keypress has made a strong assumption on their behalf. So the two
//! are separate operations: [`remove_from_library`] takes the book out
//! of the index and leaves the file alone; [`delete_with_file`] does
//! both, through the system wastebasket where there is one.
//!
//! # The ignore list, and why it is not optional
//!
//! Removing the entry and leaving the file is only half an operation.
//! The next scan finds an unclaimed book file sitting in the library and
//! does exactly what it is supposed to: adds it. The book the user just
//! removed comes back, and no amount of removing it again will help.
//!
//! So a removal records the file as ignored, and the scanner skips it.
//! That makes the ignore list load-bearing — and a hidden list of files
//! the app silently refuses to show you is its own kind of bug, so it is
//! inspectable ([`IgnoreStore::list`]) and reversible
//! ([`IgnoreStore::unignore`]), and the scan reports what it skipped
//! rather than quietly omitting it.
//!
//! # Keyed by content *and* path
//!
//! By hash, so the ignore survives the file being renamed afterwards —
//! which is the common case, since somebody who removed a book from the
//! library is quite likely to tidy up its file next. By path as well, so
//! a file whose hash was never recorded can still be ignored.
//!
//! The consequence, which is worth stating plainly: ignoring by content
//! means a *different* file with identical bytes is also skipped. That
//! is usually what somebody means ("I do not want this in my library"),
//! and where it is not, the list is right there to undo.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::cache::Cache;

/// A file the scanner should not re-add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFile {
    /// Where it was when it was removed, relative to the library root.
    pub path: String,
    pub hash: Option<String>,
    /// What the book was called. The list is for a person to read, and
    /// `Receipts/scan0042.pdf` alone tells them very little.
    pub title: String,
    pub ignored_at: String,
}

pub struct IgnoreStore {
    conn: Arc<Mutex<Connection>>,
    library_path: PathBuf,
}

impl IgnoreStore {
    pub fn new(conn: Arc<Mutex<Connection>>, library_path: &Path) -> Self {
        IgnoreStore { conn, library_path: library_path.to_path_buf() }
    }

    fn initialize(&self) -> Result<()> {
        crate::checksums::ChecksumStore::new(self.conn.clone(), &self.library_path).initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checksums_db.ignored_files (
                path TEXT NOT NULL PRIMARY KEY,
                blake3_hex TEXT,
                title TEXT NOT NULL,
                ignored_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS checksums_db.ignored_files_by_hash
                ON ignored_files (blake3_hex);",
        )?;
        Ok(())
    }

    pub fn ignore(&self, path: &str, hash: Option<&str>, title: &str) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checksums_db.ignored_files (path, blake3_hex, title, ignored_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(path) DO UPDATE SET blake3_hex = excluded.blake3_hex, title = excluded.title",
            (path, hash, title),
        )?;
        Ok(())
    }

    /// Whether the scanner should skip this file.
    ///
    /// Either signal is enough: the path catches a file that has not
    /// moved, the hash catches one that has been renamed since.
    pub fn is_ignored(&self, path: &str, hash: Option<&str>) -> Result<bool> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let by_path: bool = conn.prepare("SELECT 1 FROM checksums_db.ignored_files WHERE path = ?1")?.exists([path])?;
        if by_path {
            return Ok(true);
        }
        match hash {
            Some(hash) => Ok(conn.prepare("SELECT 1 FROM checksums_db.ignored_files WHERE blake3_hex = ?1")?.exists([hash])?),
            None => Ok(false),
        }
    }

    /// Everything currently being skipped, newest last.
    pub fn list(&self) -> Result<Vec<IgnoredFile>> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path, blake3_hex, title, ignored_at FROM checksums_db.ignored_files ORDER BY ignored_at, path")?;
        let rows = stmt.query_map([], |row| {
            Ok(IgnoredFile { path: row.get(0)?, hash: row.get(1)?, title: row.get(2)?, ignored_at: row.get(3)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Stops skipping a file, so the next scan adds it back.
    pub fn unignore(&self, path: &str) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checksums_db.ignored_files WHERE path = ?1", [path])?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.initialize()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checksums_db.ignored_files", ())?;
        Ok(())
    }
}

/// What a removal did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemovalReport {
    /// Files left on disk and added to the ignore list.
    pub ignored: Vec<String>,
    /// Files sent to the wastebasket, or deleted outright where there is
    /// none.
    pub deleted: Vec<String>,
    /// `(path, why)` for files that could not be deleted. The entry is
    /// still removed: the book is gone from the library either way, and
    /// failing the whole operation over a locked file would leave the
    /// user unable to remove it at all.
    pub failed: Vec<(String, String)>,
}

/// Takes a book out of the library, leaving its files where they are.
///
/// The default for a tracked folder, and the reason the ignore list
/// exists: without it the next scan finds the file, does its job, and
/// puts the book straight back.
pub fn remove_from_library(cache: &Cache, book_id: i32) -> Result<RemovalReport> {
    let mut report = RemovalReport::default();
    let title = cache.field_for(book_id, "title")?.unwrap_or_default();
    let store = cache.ignored();

    for (path, hash) in book_files(cache, book_id)? {
        store.ignore(&path, hash.as_deref(), &title)?;
        report.ignored.push(path);
    }
    cache.orphans().clear_all_for_book(book_id)?;
    cache.delete_book(book_id)?;
    Ok(report)
}

/// Takes a book out of the library **and** removes its files.
///
/// To the system wastebasket where one exists, so the choice stays
/// recoverable — the explicit second option, not the default. No ignore
/// entry is recorded: there is no file left to re-add, and an ignore for
/// a path that no longer exists would only accumulate.
pub fn delete_with_file(cache: &Cache, book_id: i32) -> Result<RemovalReport> {
    let mut report = RemovalReport::default();
    let library = cache.backend.library_path.clone();

    for (path, _) in book_files(cache, book_id)? {
        let absolute = library.join(&path);
        if !absolute.is_file() {
            continue;
        }
        match trash::delete(&absolute) {
            Ok(()) => report.deleted.push(path),
            Err(e) => {
                // No wastebasket (a headless server, some network
                // mounts, a container) is the common reason. Falling back
                // to a permanent delete is right here: the user asked for
                // the file to go, and refusing would leave them unable to
                // delete anything on such a system.
                match std::fs::remove_file(&absolute) {
                    Ok(()) => report.deleted.push(path),
                    Err(remove_err) => report.failed.push((path, format!("{e}; and deleting outright also failed: {remove_err}"))),
                }
            }
        }
    }

    cache.orphans().clear_all_for_book(book_id)?;
    cache.delete_book(book_id)?;
    Ok(report)
}

/// Every file a book owns, as `(relative path, recorded hash)`.
fn book_files(cache: &Cache, book_id: i32) -> Result<Vec<(String, Option<String>)>> {
    let folder = cache.field_for(book_id, "path")?.unwrap_or_default();
    let checksums = cache.checksums();
    Ok(cache
        .format_file_names(book_id)?
        .into_iter()
        .map(|(format, name)| {
            let file = format!("{name}.{}", format.to_lowercase());
            let path = if folder.is_empty() { file } else { format!("{folder}/{file}") };
            let (hash, _) = checksums.recorded_identity(book_id, "format", &format).unwrap_or((None, None));
            (path, hash)
        })
        .collect())
}

/// Reads a file's hash for an ignore check, or `None` if it cannot be
/// read.
pub fn hash_for_ignore_check(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

/// Removes a book by [`remove_from_library`] or [`delete_with_file`].
///
/// `delete_file` is the explicit choice, never a default: see this
/// module's docs.
pub fn remove(cache: &Cache, book_id: i32, delete_file: bool) -> Result<RemovalReport> {
    if delete_file {
        delete_with_file(cache, book_id).with_context(|| format!("deleting book {book_id} and its files"))
    } else {
        remove_from_library(cache, book_id).with_context(|| format!("removing book {book_id} from the library"))
    }
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

    fn index(dir: &Path, cache: &Cache) -> usize {
        let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
        crate::scan::index_scan(cache, dir, &report).added.len()
    }

    /// The bug the ignore list exists to prevent: remove a book, rescan,
    /// and the book you just removed is back.
    #[test]
    fn a_removed_book_does_not_come_back_on_the_next_scan() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        assert_eq!(index(dir.path(), &cache), 1);
        let id = cache.all_book_ids().unwrap()[0];

        let report = remove_from_library(&cache, id).unwrap();
        assert_eq!(report.ignored, vec!["a.pdf"]);
        assert!(report.deleted.is_empty());

        // The file is still there -- that is the whole point of this
        // removal -- and the scan must not re-add it.
        assert!(dir.path().join("a.pdf").exists());
        assert_eq!(index(dir.path(), &cache), 0, "the removed book came back");
        assert!(cache.all_book_ids().unwrap().is_empty());
    }

    /// The ignore survives the file being renamed afterwards, which is
    /// exactly what somebody tidying up after a removal will do.
    #[test]
    fn the_ignore_follows_the_file_when_it_is_renamed() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF distinctive content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        remove_from_library(&cache, id).unwrap();

        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("renamed later.pdf")).unwrap();

        assert_eq!(index(dir.path(), &cache), 0, "renaming the file defeated the ignore");
    }

    /// Reversible, and the reversal has to actually bring the book back
    /// -- a list you cannot act on is no better than a hidden one.
    #[test]
    fn unignoring_lets_the_next_scan_add_it_again() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        remove_from_library(&cache, id).unwrap();
        assert_eq!(index(dir.path(), &cache), 0);

        cache.ignored().unignore("a.pdf").unwrap();
        assert_eq!(index(dir.path(), &cache), 1, "unignoring did not bring it back");
    }

    /// Inspectable: the list is for a person, so it carries the title
    /// rather than only a path.
    #[test]
    fn the_ignore_list_is_readable() {
        let (dir, cache) = library();
        write(&dir.path().join("Receipts/scan0042.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        cache.set_field(id, "title", "Boiler Service Invoice").unwrap();
        remove_from_library(&cache, id).unwrap();

        let listed = cache.ignored().list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "Receipts/scan0042.pdf");
        assert_eq!(listed[0].title, "Boiler Service Invoice");
        assert!(listed[0].hash.is_some(), "without a hash the ignore cannot survive a rename");
    }

    #[test]
    fn deleting_with_the_file_removes_both_and_records_no_ignore() {
        let (dir, cache) = library();
        let path = dir.path().join("a.pdf");
        write(&path, b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];

        let report = delete_with_file(&cache, id).unwrap();
        assert_eq!(report.deleted, vec!["a.pdf"], "{report:?}");
        assert!(!path.exists());
        assert!(cache.all_book_ids().unwrap().is_empty());
        // Nothing to re-add, so nothing to ignore -- an entry for a path
        // that no longer exists would only accumulate.
        assert!(cache.ignored().list().unwrap().is_empty());
    }

    #[test]
    fn remove_dispatches_on_the_explicit_choice() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        write(&dir.path().join("b.pdf"), b"%PDF two");
        index(dir.path(), &cache);
        let ids = cache.all_book_ids().unwrap();

        let kept = remove(&cache, ids[0], false).unwrap();
        assert!(!kept.ignored.is_empty() && kept.deleted.is_empty());

        let gone = remove(&cache, ids[1], true).unwrap();
        assert!(gone.deleted.len() == 1 && gone.ignored.is_empty());
    }

    /// Stated plainly in the module docs, and worth a test so nobody is
    /// surprised later: ignoring by content skips a different file with
    /// the same bytes.
    #[test]
    fn ignoring_by_content_also_skips_an_identical_copy_elsewhere() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF the same bytes");
        index(dir.path(), &cache);
        remove_from_library(&cache, cache.all_book_ids().unwrap()[0]).unwrap();

        write(&dir.path().join("Copies/also a.pdf"), b"%PDF the same bytes");
        assert_eq!(index(dir.path(), &cache), 0, "an identical copy was added despite the content being ignored");

        // And the list is right there to undo it.
        cache.ignored().clear().unwrap();
        assert_eq!(index(dir.path(), &cache), 2);
    }

    #[test]
    fn a_book_with_several_formats_ignores_all_of_them() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        let epub = dir.path().join("a.epub");
        write(&epub, b"epub content");
        cache.add_format(id, &epub, "epub", true).unwrap();

        let report = remove_from_library(&cache, id).unwrap();
        assert_eq!(report.ignored.len(), 2, "{report:?}");
    }

    #[test]
    fn removing_a_book_also_clears_any_orphan_flag_it_had() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        cache.orphans().mark(id, "PDF", "a.pdf").unwrap();

        remove_from_library(&cache, id).unwrap();
        // Otherwise the orphan list would keep naming a book that no
        // longer exists.
        assert!(cache.orphans().list().unwrap().is_empty());
    }

    #[test]
    fn an_unignored_path_is_not_reported_as_ignored() {
        let (_dir, cache) = library();
        let store = cache.ignored();
        store.ignore("a.pdf", Some("abc"), "A").unwrap();
        assert!(store.is_ignored("a.pdf", None).unwrap());
        assert!(store.is_ignored("somewhere/else.pdf", Some("abc")).unwrap());
        assert!(!store.is_ignored("b.pdf", Some("different")).unwrap());
        assert!(!store.is_ignored("b.pdf", None).unwrap());
    }
}
