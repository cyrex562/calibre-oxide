//! Checking a library for problems (issue #897, part of #889).
//!
//! # Why this replaces `check_library`
//!
//! [`crate::check_library`] is a faithful port of upstream's
//! `check_library.py`, and two things made it unusable here.
//!
//! **It was partly inert.** It recognises a book directory by calibre's
//! `Title (id)` naming, via a regex this project has never produced —
//! `restore.rs`'s own module doc already noted the mismatch. Its
//! `missing_formats` and `extra_formats` results were therefore
//! unreliable, and after #889 there are no per-book directories to
//! recognise at all.
//!
//! **It was actively wrong about the most common event.** Its
//! `corrupted_formats` check reports any format whose content no longer
//! matches its recorded hash. In a folder the user edits, that is the
//! *normal* result of annotating a PDF — so every edited file was
//! reported as damaged, and a check that cries wolf is worse than no
//! check at all, because the user learns to dismiss it.
//!
//! # What replaced it
//!
//! Composition rather than a second traversal. [`crate::scan`] walks the
//! folder, [`crate::drift`] works out what changed, and this assembles
//! the answer. One traversal, one set of rules, and the edge cases
//! (nested libraries, unreadable directories, cloud placeholders,
//! swapped filenames) are handled once in the places that already handle
//! them rather than twice, differently.
//!
//! # "Changed", not "corrupted"
//!
//! A file whose content differs from what was recorded might be an edit
//! or it might be bit rot, and **nothing can tell those apart**. Calling
//! it corruption asserts something unknowable; calling it an edit
//! dismisses a real risk. So it is reported as *changed since the last
//! check* and left to the person who knows whether they opened it.

use anyhow::Result;

use crate::cache::Cache;
use crate::drift::{self, DriftReport};
use crate::scan::{self, ScanOptions, ScanReport};

/// One thing worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The book involved, where there is one.
    pub book_id: Option<i32>,
    /// The book's title, so the report reads as something other than a
    /// list of paths.
    pub title: String,
    /// The file or folder, relative to the library root.
    pub path: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LibraryCheck {
    /// Recorded but not found anywhere. Empty when the scan was
    /// incomplete — see [`LibraryCheck::conclusive`].
    pub missing_files: Vec<Finding>,
    /// Book files on disk that no book claims.
    pub untracked_files: Vec<Finding>,
    /// Found somewhere other than where they were recorded, and
    /// identified. **Fixable automatically** — the point of listing them
    /// is that the fix is one action, not that something is wrong.
    pub moved_files: Vec<Finding>,
    /// Content differs from what was recorded. An edit or bit rot;
    /// nothing can distinguish them, so neither is claimed.
    pub changed_files: Vec<Finding>,
    /// `has_cover` is set but there is no cover file.
    pub missing_covers: Vec<Finding>,
    /// Directories that could not be read. Any entry here makes the
    /// report inconclusive.
    pub unreadable_folders: Vec<Finding>,
    /// Books whose files are gone and which the user has not yet
    /// resolved (#895).
    pub orphaned_books: Vec<Finding>,
    /// Files the scan skipped because the user removed the book and kept
    /// the file (#896). Not a problem — listed so the ignore list is
    /// visible from the same place everything else is.
    pub ignored_files: Vec<Finding>,
    /// Subfolders holding a library of their own, skipped rather than
    /// absorbed.
    pub nested_libraries: Vec<Finding>,
    /// Whether the scan saw the whole library. When false,
    /// `missing_files` was not computed: absence could not be told from
    /// invisibility.
    pub conclusive: bool,
}

impl LibraryCheck {
    /// Whether anything needs a person's attention.
    ///
    /// Deliberately excludes `moved_files` (fixable without asking),
    /// `ignored_files` (a deliberate choice), `changed_files` (usually
    /// the user's own edit) and `nested_libraries` (correct behaviour).
    /// A check that flags normal states trains people to ignore it.
    pub fn needs_attention(&self) -> bool {
        !self.missing_files.is_empty() || !self.untracked_files.is_empty() || !self.missing_covers.is_empty() || !self.unreadable_folders.is_empty() || !self.orphaned_books.is_empty()
    }
}

/// Runs a full check.
pub fn check(cache: &Cache) -> Result<LibraryCheck> {
    let library = cache.backend.library_path.clone();
    let scanned = scan::walk(&library, &ScanOptions::default(), std::time::SystemTime::now())?;
    let drifted = drift::detect(cache, &scanned)?;
    Ok(assemble(cache, &scanned, &drifted))
}

/// Builds the report from an already-computed scan and drift.
///
/// Split out so a caller that has just scanned does not scan twice, and
/// so the assembly is testable against a handmade report.
pub fn assemble(cache: &Cache, scanned: &ScanReport, drifted: &DriftReport) -> LibraryCheck {
    let title_of = |book_id: i32| cache.field_for(book_id, "title").ok().flatten().unwrap_or_default();
    let recorded_path = |book_id: i32, format: &str| {
        let folder = cache.field_for(book_id, "path").ok().flatten().unwrap_or_default();
        let name = cache.format_file_names(book_id).ok().and_then(|files| files.into_iter().find(|(f, _)| f.eq_ignore_ascii_case(format)).map(|(_, n)| n)).unwrap_or_default();
        let file = format!("{name}.{}", format.to_lowercase());
        if folder.is_empty() {
            file
        } else {
            format!("{folder}/{file}")
        }
    };

    let mut check = LibraryCheck { conclusive: drifted.conclusive, ..Default::default() };

    for (book_id, format) in &drifted.missing {
        check.missing_files.push(Finding { book_id: Some(*book_id), title: title_of(*book_id), path: recorded_path(*book_id, format) });
    }
    for path in &drifted.untracked {
        check.untracked_files.push(Finding { book_id: None, title: String::new(), path: path.clone() });
    }
    for relocation in &drifted.moved {
        check.moved_files.push(Finding { book_id: Some(relocation.book_id), title: title_of(relocation.book_id), path: relocation.to.clone() });
    }
    for edit in &drifted.edited {
        check.changed_files.push(Finding { book_id: Some(edit.book_id), title: title_of(edit.book_id), path: edit.path.clone() });
    }
    for path in &scanned.unreadable {
        check.unreadable_folders.push(Finding { book_id: None, title: String::new(), path: path.clone() });
    }
    for path in &scanned.nested_libraries {
        check.nested_libraries.push(Finding { book_id: None, title: String::new(), path: path.clone() });
    }

    // Covers: `has_cover` set with no file behind it. The reverse -- a
    // cover file with `has_cover` unset -- is not checked, because a
    // cover now lives in the state directory under the book's uuid and
    // cannot be there without having been written for that book.
    if let Ok(ids) = cache.all_book_ids() {
        for book_id in ids {
            if cache.has_cover(book_id).unwrap_or(false) {
                let exists = crate::covers::cover_path(cache, book_id).map(|p| p.is_file()).unwrap_or(false);
                if !exists {
                    check.missing_covers.push(Finding { book_id: Some(book_id), title: title_of(book_id), path: "cover".to_string() });
                }
            }
        }
    }

    if let Ok(orphans) = cache.orphans().list() {
        for orphan in orphans {
            check.orphaned_books.push(Finding { book_id: Some(orphan.book_id), title: title_of(orphan.book_id), path: orphan.last_known_path });
        }
    }

    if let Ok(ignored) = cache.ignored().list() {
        for entry in ignored {
            check.ignored_files.push(Finding { book_id: None, title: entry.title, path: entry.path });
        }
    }

    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    /// Indexes, waiting out the settle window the scanner applies to
    /// freshly-written files.
    fn index(dir: &Path, cache: &Cache) {
        let report = scan::walk(dir, &ScanOptions { settle: std::time::Duration::ZERO, ..Default::default() }, std::time::SystemTime::now()).unwrap();
        scan::index_scan(cache, dir, &report);
    }

    fn check_now(dir: &Path, cache: &Cache) -> LibraryCheck {
        let scanned = scan::walk(dir, &ScanOptions { settle: std::time::Duration::ZERO, ..Default::default() }, std::time::SystemTime::now()).unwrap();
        let drifted = drift::detect(cache, &scanned).unwrap();
        assemble(cache, &scanned, &drifted)
    }

    #[test]
    fn a_healthy_library_needs_no_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        let check = check_now(dir.path(), &cache);
        assert!(!check.needs_attention(), "{check:?}");
        assert!(check.conclusive);
    }

    /// The headline fix. An annotated PDF was reported as a **corrupted
    /// format**; a check that calls a normal edit damage is one the user
    /// learns to dismiss.
    #[test]
    fn an_edited_file_is_reported_as_changed_and_does_not_demand_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF original");
        index(dir.path(), &cache);
        write(&dir.path().join("a.pdf"), b"%PDF now with annotations");

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.changed_files.len(), 1, "{check:?}");
        assert!(check.missing_files.is_empty());
        // Informational, not a problem to solve.
        assert!(!check.needs_attention(), "an edit should not demand attention: {check:?}");
    }

    #[test]
    fn a_deleted_file_is_missing_and_does_demand_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("Receipts/a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("Receipts/a.pdf")).unwrap();

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.missing_files.len(), 1, "{check:?}");
        // Named, not just pathed: a list of paths is hard to act on.
        assert_eq!(check.missing_files[0].path, "Receipts/a.pdf");
        assert!(check.missing_files[0].book_id.is_some());
        assert!(check.needs_attention());
    }

    /// A moved file is listed because the fix is one action, not because
    /// anything is wrong -- so it must not raise the alarm.
    #[test]
    fn a_moved_file_is_listed_without_demanding_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("b.pdf")).unwrap();

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.moved_files.len(), 1, "{check:?}");
        assert_eq!(check.moved_files[0].path, "b.pdf");
        assert!(!check.needs_attention(), "{check:?}");
    }

    #[test]
    fn an_unclaimed_file_is_untracked() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        write(&dir.path().join("dropped in.pdf"), b"%PDF new");

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.untracked_files.len(), 1, "{check:?}");
        assert!(check.needs_attention());
    }

    #[test]
    fn a_missing_cover_is_reported() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        crate::covers::set_cover(&cache, id, b"cover bytes").unwrap();
        std::fs::remove_file(crate::covers::cover_path(&cache, id).unwrap()).unwrap();

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.missing_covers.len(), 1, "{check:?}");
    }

    #[test]
    fn a_book_with_no_cover_recorded_is_not_a_missing_cover() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        assert!(check_now(dir.path(), &cache).missing_covers.is_empty());
    }

    /// E17 once more, through the reporting layer this time.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_makes_the_report_inconclusive() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, cache) = library();
        write(&dir.path().join("readable.pdf"), b"%PDF one");
        let locked = dir.path().join("locked");
        write(&locked.join("hidden.pdf"), b"%PDF two");
        index(dir.path(), &cache);

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let check = check_now(dir.path(), &cache);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!check.conclusive);
        assert_eq!(check.unreadable_folders.len(), 1, "{check:?}");
        // And nothing is claimed missing: `hidden.pdf` is invisible, not
        // gone.
        assert!(check.missing_files.is_empty(), "{check:?}");
        // It still demands attention -- the user should know the check
        // could not finish.
        assert!(check.needs_attention());
    }

    /// The ignore list surfaces here so it is visible from the same place
    /// as everything else -- a hidden list of files the app refuses to
    /// show is its own kind of bug (#896).
    #[test]
    fn ignored_files_are_listed_without_demanding_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        let id = cache.all_book_ids().unwrap()[0];
        cache.set_field(id, "title", "A Removed Book").unwrap();
        crate::removal::remove_from_library(&cache, id).unwrap();

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.ignored_files.len(), 1, "{check:?}");
        assert_eq!(check.ignored_files[0].title, "A Removed Book");
        // The file is still on disk, but it is not untracked: it is
        // deliberately ignored.
        assert!(check.untracked_files.is_empty(), "{check:?}");
        assert!(!check.needs_attention(), "{check:?}");
    }

    #[test]
    fn an_orphan_is_reported_with_where_its_file_used_to_be() {
        let (dir, cache) = library();
        write(&dir.path().join("Receipts/a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("Receipts/a.pdf")).unwrap();

        let scanned = scan::walk(dir.path(), &ScanOptions { settle: std::time::Duration::ZERO, ..Default::default() }, std::time::SystemTime::now()).unwrap();
        let drifted = drift::detect(&cache, &scanned).unwrap();
        crate::orphans::reconcile(&cache, &drifted).unwrap();

        let check = assemble(&cache, &scanned, &drifted);
        assert_eq!(check.orphaned_books.len(), 1, "{check:?}");
        assert_eq!(check.orphaned_books[0].path, "Receipts/a.pdf");
        assert!(check.needs_attention());
    }

    /// Skipping a nested library is correct behaviour, so it is listed
    /// for transparency and raises no alarm.
    #[test]
    fn a_nested_library_is_listed_without_demanding_attention() {
        let (dir, cache) = library();
        write(&dir.path().join("mine.pdf"), b"%PDF content");
        write(&dir.path().join("Theirs/metadata.db"), b"SQLite");
        write(&dir.path().join("Theirs/book.pdf"), b"%PDF theirs");
        index(dir.path(), &cache);

        let check = check_now(dir.path(), &cache);
        assert_eq!(check.nested_libraries.len(), 1, "{check:?}");
        assert!(!check.needs_attention(), "{check:?}");
    }

    #[test]
    fn check_runs_end_to_end_against_a_real_library() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        // The public entry point, which does its own scan -- and applies
        // the real settle window, so a file written this instant is not
        // yet visible to it. That is correct behaviour, not a bug: the
        // interesting assertion is that it runs and reports coherently.
        let check = check(&cache).unwrap();
        assert!(check.conclusive);
    }
}
