//! Walking a library folder to find the books in it (issue #891, part
//! of #889).
//!
//! # Phase one of two
//!
//! Opening a folder already creates the database — `Backend::new`
//! creates the directory and the schema if absent. What it never did is
//! notice the files already sitting in it. This is that walk.
//!
//! It is deliberately the cheap half. Every file yields one `stat` and
//! nothing more: no hashing, no metadata parsing, no reading of
//! content. A folder of five thousand PDFs has to be browsable in
//! seconds, and hashing it is minutes to hours of I/O — so the
//! hash-derived conclusions (which files are duplicates, which belong
//! to one book) are a separate pass that lands afterwards.
//!
//! # A scan that failed must not conclude anything
//!
//! The most dangerous thing a scanner of somebody else's folder can do
//! is decide a file is gone when it merely could not be read. A network
//! share that dropped, a permissions error on one subdirectory, a
//! sleeping external disk — each makes files *look* absent, and acting
//! on that turns a temporary condition into a library full of orphans.
//!
//! So [`ScanReport::is_complete`] is false whenever any directory could
//! not be read, and the drift and orphan passes are required to check
//! it before concluding a single file is missing. A partial scan is
//! still useful for *finding* new files; it is worthless for deciding
//! what is gone.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use calibre_utils::filenames::{file_facts, FileFacts};

use crate::constants::LIBRARY_HANDLE_DIR_NAME;

/// How long a file must have been untouched before it is indexed.
///
/// A file that appeared a moment ago is very likely still being written
/// — a browser download, a copy over a network share, a scanner writing
/// page by page. Indexing it half-complete records a size and (once the
/// hashing pass runs) a hash for a file that is about to be different.
///
/// Matches the desktop app's auto-add watcher, which settled on the same
/// five seconds for the same reason.
pub const DEFAULT_SETTLE: Duration = Duration::from_secs(5);

/// Names that are never books, whatever their extension.
///
/// Extends the ignore list `check_library` already carries with the
/// things a NAS or a cloud client leaves lying around.
const JUNK_NAMES: &[&str] = &[
    ".ds_store",
    "thumbs.db",
    "desktop.ini",
    ".directory",
    "metadata.db",
    "metadata_db_prefs_backup.json",
    "metadata.opf",
    "cover.jpg",
];

/// Directory names that are somebody else's business.
const JUNK_DIRS: &[&str] = &[
    LIBRARY_HANDLE_DIR_NAME,
    "@eadir",          // Synology thumbnails
    ".@__thumb",       // QNAP
    "#recycle",        // Synology
    "$recycle.bin",    // Windows
    "system volume information",
    ".trash",
    ".trashes",
    ".git",
];

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub settle: Duration,
    /// Extensions worth indexing, lowercase and without the dot.
    pub extensions: HashSet<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            settle: DEFAULT_SETTLE,
            // `metadata_extensions` is the list this crate already
            // treats as "a book file", minus `opf`, which is metadata
            // *about* a book rather than one.
            extensions: crate::adding::metadata_extensions().iter().filter(|e| **e != "opf").map(|e| e.to_string()).collect(),
        }
    }
}

/// One book file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    /// Relative to the library root, `/`-separated so the value is the
    /// same on every platform and can go straight into `books.path` and
    /// `data.name`.
    pub relative_path: String,
    /// The `books.path` half: the folder, `""` for the library root.
    pub folder: String,
    /// The `data.name` half: the filename without its extension.
    pub stem: String,
    /// Lowercase, no dot.
    pub extension: String,
    pub facts: FileFacts,
}

#[derive(Debug, Default, Clone)]
pub struct ScanReport {
    pub files: Vec<DiscoveredFile>,
    /// Directories skipped because they hold a library of their own.
    pub nested_libraries: Vec<String>,
    /// Files skipped because they are still settling.
    pub settling: Vec<String>,
    /// Files skipped because the bytes are not local — a cloud-sync
    /// placeholder. Reading one downloads it.
    pub offline: Vec<String>,
    /// Directories that could not be read. Any entry here makes the
    /// scan incomplete.
    pub unreadable: Vec<String>,
}

impl ScanReport {
    /// Whether every directory under the library was successfully read.
    ///
    /// **Check this before concluding a file is missing.** A scan with
    /// an unreadable directory cannot tell "gone" from "not visible
    /// right now", and treating the second as the first is how a dropped
    /// network share becomes a library full of orphans.
    pub fn is_complete(&self) -> bool {
        self.unreadable.is_empty()
    }
}

/// Walks `library_path` and reports the book files in it.
///
/// `now` is passed in rather than read from the clock so the settle
/// window is testable without sleeping.
pub fn walk(library_path: &Path, options: &ScanOptions, now: SystemTime) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    // The root failing is different from a subdirectory failing: there
    // is nothing to report on, and returning an empty report would say
    // "this library has no books".
    let root_entries = std::fs::read_dir(library_path).with_context(|| format!("reading the library folder {}", library_path.display()))?;
    drop(root_entries);

    walk_dir(library_path, library_path, options, now, &mut report);
    report.files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(report)
}

fn walk_dir(root: &Path, dir: &Path, options: &ScanOptions, now: SystemTime, report: &mut ScanReport) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            report.unreadable.push(relative_to(root, dir));
            return;
        }
    };

    let mut subdirs = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            // An entry that cannot even be read counts as an unreadable
            // directory: something in here is invisible to us, and that
            // is exactly the condition that must not become an orphan.
            report.unreadable.push(relative_to(root, dir));
            continue;
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let lower = name.to_ascii_lowercase();

        // `file_type` rather than `metadata`: it does not follow
        // symlinks, which is what lets a directory symlink be
        // recognised and skipped before it is descended into.
        let Ok(file_type) = entry.file_type() else {
            report.unreadable.push(relative_to(root, dir));
            continue;
        };

        if file_type.is_dir() {
            if JUNK_DIRS.contains(&lower.as_str()) {
                continue;
            }
            if holds_its_own_library(&path) {
                report.nested_libraries.push(relative_to(root, &path));
                continue;
            }
            subdirs.push(path);
            continue;
        }

        if file_type.is_symlink() {
            // A symlink to a file is indexed by its own path -- the
            // library contains this name, whatever it points at. A
            // symlink to a *directory* is not descended into, because a
            // loop would otherwise walk forever; `is_dir` above is false
            // for it since `file_type` does not follow the link.
            if path.is_dir() {
                continue;
            }
        }

        if JUNK_NAMES.contains(&lower.as_str()) {
            continue;
        }
        // Before the extension check, not after: an iCloud placeholder
        // for `Big Scan.pdf` is named `.Big Scan.pdf.icloud`, so its
        // extension is `icloud` and the allowlist below would discard
        // it as an uninteresting file rather than reporting that a book
        // is here but not local.
        if let Some(inner) = cloud_placeholder_for(name) {
            if extension_of(&inner).is_some_and(|e| options.extensions.contains(&e)) {
                report.offline.push(relative_to(root, &path));
            }
            continue;
        }
        let Some(extension) = extension_of(name) else { continue };
        if !options.extensions.contains(&extension) {
            continue;
        }
        if is_offline_on_disk(&path) {
            report.offline.push(relative_to(root, &path));
            continue;
        }
        let Some(facts) = file_facts(&path) else {
            // Present in the listing but not stat-able: another
            // unreadable condition, not an absent file.
            report.unreadable.push(relative_to(root, dir));
            continue;
        };
        if is_settling(&facts, options.settle, now) {
            report.settling.push(relative_to(root, &path));
            continue;
        }

        let relative_path = relative_to(root, &path);
        let folder = relative_to(root, dir);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name).to_string();
        report.files.push(DiscoveredFile { relative_path, folder, stem, extension, facts });
    }

    subdirs.sort();
    for subdir in subdirs {
        walk_dir(root, &subdir, options, now, report);
    }
}

/// Whether `dir` is a library in its own right.
///
/// Descending into one would double-track every book it contains: the
/// nested library's own database already knows about them, and this one
/// would claim them too. Recognised by the two things every library has.
fn holds_its_own_library(dir: &Path) -> bool {
    dir.join("metadata.db").exists() || dir.join(LIBRARY_HANDLE_DIR_NAME).is_dir()
}

/// The real filename an iCloud placeholder stands in for.
///
/// macOS evicts `Big Scan.pdf` by replacing it with
/// `.Big Scan.pdf.icloud`. The name is the only signal there is, and it
/// is a reliable one — but it means the placeholder's own extension is
/// `icloud`, so the book extension has to be read out of the inner name.
fn cloud_placeholder_for(name: &str) -> Option<String> {
    let inner = name.strip_prefix('.')?.strip_suffix(".icloud")?;
    (!inner.is_empty()).then(|| inner.to_string())
}

fn extension_of(name: &str) -> Option<String> {
    Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
}

/// Whether a file that is present under its own name has no local
/// bytes.
///
/// OneDrive and Dropbox keep the real filename and mark the file
/// instead, so unlike iCloud there is nothing in the name to notice.
/// Reading one downloads it, which on a metered connection is somebody's
/// data allowance, so it is reported and left alone until it is local.
fn is_offline_on_disk(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
        const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
        const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
        if let Ok(metadata) = std::fs::metadata(path) {
            let attrs = metadata.file_attributes();
            let placeholder = FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
            return attrs & placeholder != 0;
        }
    }
    #[cfg(not(windows))]
    {
        // No portable equivalent: Linux has no such attribute, and the
        // macOS case is caught by name above.
        let _ = path;
    }
    false
}

fn is_settling(facts: &FileFacts, settle: Duration, now: SystemTime) -> bool {
    let Some(mtime_ms) = facts.mtime_ms else {
        // No modification time to judge by. Indexing it is the better
        // risk: refusing would mean never indexing anything on a
        // filesystem that reports no mtime.
        return false;
    };
    let now_ms = now.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    // A file stamped in the future (clock skew, a bad archive) is not
    // settling -- it would otherwise be skipped on every scan forever.
    now_ms.saturating_sub(mtime_ms) < settle.as_millis() as u64 && mtime_ms <= now_ms
}

/// `/`-separated path relative to the library root.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// What indexing a scan did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Books created from files that were not tracked before.
    pub added: Vec<i32>,
    /// Files already claimed by a book — a rescan recognising its own
    /// earlier work, which is the normal case on every scan after the
    /// first.
    pub already_known: usize,
    /// `(relative path, why)` for files that could not be indexed. One
    /// bad file does not abandon the rest of the folder.
    pub failed: Vec<(String, String)>,
    /// Files skipped because the user removed the book and kept the file
    /// (#896). Reported rather than quietly omitted: a hidden list of
    /// files the app refuses to show is its own kind of bug.
    pub ignored: Vec<String>,
}

/// Creates a book for every discovered file the library does not
/// already know about.
///
/// Each file's own embedded metadata is read here — title, authors, and
/// for a PDF the cover rendered from page 1. That is the expensive part
/// of phase one, and it is still far cheaper than hashing: it reads a
/// header rather than the whole file.
///
/// **Does not move or copy anything.** Indexing a folder the user filled
/// must leave it exactly as it was found.
///
/// Grouping several files into one book ([`crate::grouping`]) is not
/// applied yet: it depends on content hashes, which arrive in the
/// background pass. Until then each file is its own book, and merging is
/// something the later pass does rather than something this has to
/// guess at.
pub fn index_scan(cache: &crate::cache::Cache, library_path: &Path, report: &ScanReport) -> IndexReport {
    let mut indexed = IndexReport::default();

    for file in &report.files {
        match cache.book_for_file(&file.relative_path) {
            Ok(Some(_)) => {
                indexed.already_known += 1;
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                indexed.failed.push((file.relative_path.clone(), e.to_string()));
                continue;
            }
        }

        let absolute = library_path.join(&file.relative_path);

        // Removed by the user, who chose to keep the file. Without this
        // the scan does exactly its job and puts the book straight back.
        // Checked against the path *and* the content, so the ignore
        // survives the file being renamed afterwards.
        let hash = crate::removal::hash_for_ignore_check(&absolute);
        if cache.ignored().is_ignored(&file.relative_path, hash.as_deref()).unwrap_or(false) {
            indexed.ignored.push(file.relative_path.clone());
            continue;
        }

        // A file whose metadata cannot be read is still a book. Falling
        // back to the filename is what the user would do, and refusing
        // to index it would leave a file sitting in the library that
        // nothing ever shows.
        let mut metadata = calibre_ebooks::metadata::get_metadata(&absolute).unwrap_or_default();
        if metadata.title.trim().is_empty() || metadata.title == "Unknown" {
            metadata.title = file.stem.clone();
        }
        if metadata.authors.is_empty() {
            metadata.authors = vec!["Unknown".to_string()];
        }

        match cache.register_book_in_place(&file.relative_path, &metadata) {
            Ok(id) => indexed.added.push(id),
            Err(e) => indexed.failed.push((file.relative_path.clone(), e.to_string())),
        }
    }
    indexed
}

/// What a rescan did.
#[derive(Debug)]
pub struct Rescan {
    /// The walk itself. Carries `is_complete`, which the caller must
    /// check before telling anyone a book is gone.
    pub scanned: ScanReport,
    /// Books whose recorded path was corrected to where the file
    /// actually is. Silent by design (#894): the user moved a file
    /// inside their own folder, which is allowed.
    pub relocated: usize,
    /// New books created from files nothing claimed.
    pub indexed: IndexReport,
}

/// Walks the library, re-attaches what moved, and indexes what is new.
///
/// The order is the whole point. Re-attachment runs **before** indexing
/// because a file that turned up in a new place is usually a file that
/// left an old one -- the same book, moved. Indexing first would create
/// a second book for it and leave the first pointing at nothing, so one
/// drag-and-drop in the user's file manager would turn one book into a
/// duplicate plus an orphan.
///
/// Nothing is moved, copied, or deleted. This is how a folder the user
/// filled becomes a library: by being read.
///
/// `now` is passed in rather than read from the clock, like [`walk`]'s:
/// every file a test has just written is inside the settle window, so a
/// rescan that read the clock itself would find nothing and could only
/// be tested by sleeping.
pub fn rescan(cache: &crate::cache::Cache, now: SystemTime) -> Result<Rescan> {
    let library = cache.backend.library_path.clone();
    let scanned = walk(&library, &ScanOptions::default(), now)?;

    // Safe on an incomplete scan: a relocation is concluded from a file
    // that *was* found, not from one that was not. Only `missing` needs
    // the scan to have seen everything, and nothing here acts on it.
    let drifted = crate::drift::detect(cache, &scanned)?;
    let relocated = crate::drift::apply_relocations(cache, &drifted)?;

    let indexed = index_scan(cache, &library, &scanned);
    Ok(Rescan { scanned, relocated, indexed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Far enough in the past that nothing is settling.
    fn much_later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(3600)
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn scan(dir: &Path) -> ScanReport {
        walk(dir, &ScanOptions::default(), much_later()).unwrap()
    }

    fn names(report: &ScanReport) -> Vec<&str> {
        report.files.iter().map(|f| f.relative_path.as_str()).collect()
    }

    #[test]
    fn finds_books_in_the_library_root() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF");
        write(&dir.path().join("scan0042.pdf"), b"%PDF");

        let report = scan(dir.path());
        assert_eq!(names(&report), vec!["Boiler Manual.pdf", "scan0042.pdf"]);
        assert!(report.is_complete());
    }

    #[test]
    fn a_root_level_book_has_an_empty_folder_and_its_real_filename() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("scan0001.pdf"), b"%PDF");

        let file = &scan(dir.path()).files[0];
        assert_eq!(file.folder, "", "a book in the root has no subfolder");
        assert_eq!(file.stem, "scan0001");
        assert_eq!(file.extension, "pdf");
    }

    #[test]
    fn recurses_into_subfolders_and_records_them() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("Receipts/2019 invoice.pdf"), b"%PDF");
        write(&dir.path().join("Receipts/Old/2015.pdf"), b"%PDF");

        let report = scan(dir.path());
        assert_eq!(names(&report), vec!["Receipts/2019 invoice.pdf", "Receipts/Old/2015.pdf"]);
        assert_eq!(report.files[0].folder, "Receipts");
        assert_eq!(report.files[1].folder, "Receipts/Old");
    }

    #[test]
    fn ignores_files_that_are_not_books() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("book.pdf"), b"%PDF");
        write(&dir.path().join("notes.xlsx"), b"x");
        write(&dir.path().join("photo.jpeg"), b"x");
        write(&dir.path().join("README"), b"x");

        assert_eq!(names(&scan(dir.path())), vec!["book.pdf"]);
    }

    #[test]
    fn ignores_os_and_nas_junk() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("book.pdf"), b"%PDF");
        write(&dir.path().join(".DS_Store"), b"x");
        write(&dir.path().join("Thumbs.db"), b"x");
        write(&dir.path().join("@eaDir/thumb.pdf"), b"x");
        write(&dir.path().join("#recycle/deleted.pdf"), b"x");

        assert_eq!(names(&scan(dir.path())), vec!["book.pdf"]);
    }

    /// The library's own files are not books in it.
    #[test]
    fn ignores_the_librarys_own_metadata() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("book.pdf"), b"%PDF");
        write(&dir.path().join("metadata.opf"), b"<opf/>");
        write(&dir.path().join("cover.jpg"), b"jpeg");
        write(&dir.path().join(LIBRARY_HANDLE_DIR_NAME).join("changes").join("x.pdf"), b"x");

        assert_eq!(names(&scan(dir.path())), vec!["book.pdf"]);
    }

    /// E2. Absorbing a nested library would double-track every book in
    /// it -- its own database already claims them.
    #[test]
    fn skips_a_subfolder_that_is_a_library_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("mine.pdf"), b"%PDF");
        write(&dir.path().join("Someone Elses Library/metadata.db"), b"SQLite");
        write(&dir.path().join("Someone Elses Library/theirs.pdf"), b"%PDF");

        let report = scan(dir.path());
        assert_eq!(names(&report), vec!["mine.pdf"]);
        assert_eq!(report.nested_libraries, vec!["Someone Elses Library"]);
    }

    #[test]
    fn a_nested_library_is_recognised_by_its_state_folder_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Nested").join(LIBRARY_HANDLE_DIR_NAME)).unwrap();
        write(&dir.path().join("Nested/theirs.pdf"), b"%PDF");

        let report = scan(dir.path());
        assert!(report.files.is_empty());
        assert_eq!(report.nested_libraries, vec!["Nested"]);
    }

    /// E6. A file that appeared a moment ago is probably still being
    /// written, and indexing it half-copied records the wrong size.
    #[test]
    fn a_file_still_being_written_is_left_for_the_next_scan() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("arriving.pdf"), b"%PDF");

        // "Now" is the moment the file was written, so it is inside the
        // settle window.
        let report = walk(dir.path(), &ScanOptions::default(), SystemTime::now()).unwrap();
        assert!(report.files.is_empty(), "{:?}", names(&report));
        assert_eq!(report.settling, vec!["arriving.pdf"]);

        // And it is picked up once it has stopped changing.
        assert_eq!(names(&scan(dir.path())), vec!["arriving.pdf"]);
    }

    /// A file stamped in the future would otherwise be inside the
    /// settle window on every scan for as long as the skew lasted, and
    /// never get indexed at all.
    #[test]
    fn a_file_stamped_in_the_future_is_not_treated_as_settling() {
        let facts = FileFacts {
            identity: calibre_utils::filenames::FileIdentity { volume: 1, index: 1 },
            size: 10,
            mtime_ms: Some(9_000_000),
        };
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(1_000_000);
        assert!(!is_settling(&facts, DEFAULT_SETTLE, now));
    }

    #[test]
    fn a_file_with_no_modification_time_is_indexed_rather_than_skipped_forever() {
        let facts = FileFacts {
            identity: calibre_utils::filenames::FileIdentity { volume: 1, index: 1 },
            size: 10,
            mtime_ms: None,
        };
        assert!(!is_settling(&facts, DEFAULT_SETTLE, SystemTime::now()));
    }

    /// E17, the one that turns a dropped share into a library of
    /// orphans. The scan still reports what it found; what it must not
    /// do is claim to be complete.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subfolder_makes_the_scan_incomplete() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("readable.pdf"), b"%PDF");
        let locked = dir.path().join("locked");
        write(&locked.join("hidden.pdf"), b"%PDF");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let report = scan(dir.path());
        // Restore before the assertions, so a failure does not leave an
        // undeletable temp directory behind.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(names(&report), vec!["readable.pdf"], "what was readable is still reported");
        assert!(!report.is_complete(), "a scan that could not read a folder must not claim completeness");
        assert_eq!(report.unreadable, vec!["locked"]);
    }

    /// An unreadable *root* is different: there is nothing to report on,
    /// and an empty report would read as "this library has no books".
    #[test]
    fn an_unreadable_library_root_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-there");
        assert!(walk(&missing, &ScanOptions::default(), much_later()).is_err());
    }

    #[test]
    fn a_clean_scan_reports_itself_complete() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("book.pdf"), b"%PDF");
        assert!(scan(dir.path()).is_complete());
    }

    /// E18. Reading one of these downloads it, which on a metered
    /// connection is somebody's data allowance.
    #[test]
    fn an_icloud_placeholder_is_reported_rather_than_read() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("book.pdf"), b"%PDF");
        // iCloud evicts `Big Scan.pdf` by leaving this in its place.
        write(&dir.path().join(".Big Scan.pdf.icloud"), b"placeholder");

        let report = scan(dir.path());
        assert_eq!(names(&report), vec!["book.pdf"]);
        assert_eq!(report.offline, vec![".Big Scan.pdf.icloud"]);
    }

    /// The ordering bug this had at first: a placeholder's own extension
    /// is `icloud`, so an extension check placed before the placeholder
    /// check discards it as an uninteresting file and never reports that
    /// a book is here but not local.
    #[test]
    fn an_icloud_placeholder_for_a_non_book_is_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".holiday.mov.icloud"), b"placeholder");
        write(&dir.path().join(".Big Scan.pdf.icloud"), b"placeholder");

        let report = scan(dir.path());
        assert_eq!(report.offline, vec![".Big Scan.pdf.icloud"], "only evicted *books* are worth reporting");
    }

    #[test]
    fn placeholder_names_are_parsed_back_to_the_real_filename() {
        assert_eq!(cloud_placeholder_for(".Big Scan.pdf.icloud").as_deref(), Some("Big Scan.pdf"));
        // Not a placeholder: no leading dot.
        assert_eq!(cloud_placeholder_for("Big Scan.pdf.icloud"), None);
        assert_eq!(cloud_placeholder_for("book.pdf"), None);
        assert_eq!(cloud_placeholder_for(".icloud"), None);
    }

    /// E7. Without this a symlinked loop walks until the stack runs out.
    #[cfg(unix)]
    #[test]
    fn a_directory_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("real/book.pdf"), b"%PDF");
        std::os::unix::fs::symlink(dir.path(), dir.path().join("real/loop")).unwrap();

        let report = scan(dir.path());
        assert_eq!(names(&report), vec!["real/book.pdf"]);
    }

    /// A symlink to a *file* is a name the library contains, so it is
    /// indexed -- unlike a directory link, which is only ever a way
    /// back into somewhere already walked.
    #[cfg(unix)]
    #[test]
    fn a_file_symlink_is_indexed_under_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.pdf");
        write(&target, b"%PDF");
        std::os::unix::fs::symlink(&target, dir.path().join("alias.pdf")).unwrap();

        assert_eq!(names(&scan(dir.path())), vec!["alias.pdf", "real.pdf"]);
    }

    #[test]
    fn extensions_are_matched_regardless_of_case() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("SHOUTING.PDF"), b"%PDF");
        write(&dir.path().join("Mixed.EpUb"), b"x");

        let report = scan(dir.path());
        assert_eq!(report.files.len(), 2);
        assert!(report.files.iter().all(|f| f.extension == f.extension.to_lowercase()));
    }

    #[test]
    fn a_stat_is_recorded_for_every_file_found() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"%PDF-1.4 and some content";
        write(&dir.path().join("book.pdf"), bytes);

        let file = &scan(dir.path()).files[0];
        assert_eq!(file.facts.size, bytes.len() as u64);
        assert!(file.facts.mtime_ms.is_some());
    }

    /// Paths use `/` on every platform, because they go straight into
    /// `books.path`, which has to mean the same thing in a library
    /// carried between Windows and Linux.
    #[test]
    fn relative_paths_are_slash_separated() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/b/book.pdf"), b"%PDF");
        assert_eq!(names(&scan(dir.path())), vec!["a/b/book.pdf"]);
    }

    #[test]
    fn an_empty_library_is_a_clean_empty_scan() {
        let dir = tempfile::tempdir().unwrap();
        let report = scan(dir.path());
        assert!(report.files.is_empty());
        assert!(report.is_complete());
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use crate::cache::Cache;

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

    /// Far enough ahead that nothing written by a test is still settling.
    fn much_later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(3600)
    }

    fn scan_and_index(dir: &Path, cache: &Cache) -> IndexReport {
        let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
        index_scan(cache, dir, &report)
    }

    /// The headline: opening a folder of PDFs populates the library.
    #[test]
    fn a_folder_of_books_becomes_a_library() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 one");
        write(&dir.path().join("Receipts/2019 invoice.pdf"), b"%PDF-1.4 two");

        let indexed = scan_and_index(dir.path(), &cache);
        assert_eq!(indexed.added.len(), 2, "{indexed:?}");
        assert!(indexed.failed.is_empty(), "{indexed:?}");

        let titles: Vec<String> = indexed.added.iter().map(|id| cache.field_for(*id, "title").unwrap().unwrap()).collect();
        assert!(titles.contains(&"Boiler Manual".to_string()), "{titles:?}");
        assert!(titles.contains(&"2019 invoice".to_string()), "{titles:?}");
    }

    /// **Nothing moves.** Indexing a folder somebody else filled must
    /// leave it exactly as it was found -- this is the property the whole
    /// tracked model rests on.
    #[test]
    fn indexing_does_not_move_or_copy_a_single_file() {
        let (dir, cache) = library();
        let original = dir.path().join("Receipts/2019 invoice.pdf");
        write(&original, b"%PDF-1.4 content");

        scan_and_index(dir.path(), &cache);

        assert!(original.exists(), "the file moved");
        assert_eq!(std::fs::read(&original).unwrap(), b"%PDF-1.4 content");
        // No author/title tree was created beside it.
        let top: Vec<String> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(top.iter().all(|n| n == "Receipts" || n == "metadata.db" || n.starts_with(".calibre-oxide") || n.starts_with("metadata.db-")), "unexpected entries: {top:?}");
    }

    #[test]
    fn a_books_recorded_location_matches_where_the_file_actually_is() {
        let (dir, cache) = library();
        write(&dir.path().join("Receipts/2019 invoice.pdf"), b"%PDF");

        let id = scan_and_index(dir.path(), &cache).added[0];
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some("Receipts"));
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("2019 invoice"));

        // And the book resolves to a file that is really there.
        let row = cache.get_data_as_dict(None, false, None, false).unwrap()[0].clone();
        let served = row["fmt_pdf"].as_str().expect("the format should resolve");
        assert_eq!(Path::new(served), dir.path().join("Receipts/2019 invoice.pdf"));
    }

    /// Scanning twice must not double the library -- the normal case on
    /// every open after the first.
    #[test]
    fn a_second_scan_recognises_what_the_first_one_indexed() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        write(&dir.path().join("b.pdf"), b"%PDF two");

        let first = scan_and_index(dir.path(), &cache);
        assert_eq!(first.added.len(), 2);

        let second = scan_and_index(dir.path(), &cache);
        assert!(second.added.is_empty(), "{second:?}");
        assert_eq!(second.already_known, 2);
        assert_eq!(cache.all_book_ids().unwrap().len(), 2);
    }

    #[test]
    fn a_file_added_later_is_picked_up_without_re_adding_the_rest() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        scan_and_index(dir.path(), &cache);

        write(&dir.path().join("b.pdf"), b"%PDF two");
        let second = scan_and_index(dir.path(), &cache);

        assert_eq!(second.added.len(), 1);
        assert_eq!(second.already_known, 1);
    }

    /// Two files with the same name in different folders are different
    /// books -- the lookup is on the whole location, not the filename.
    #[test]
    fn the_same_filename_in_two_folders_is_two_books() {
        let (dir, cache) = library();
        write(&dir.path().join("a/scan0001.pdf"), b"%PDF one");
        write(&dir.path().join("b/scan0001.pdf"), b"%PDF two");

        let indexed = scan_and_index(dir.path(), &cache);
        assert_eq!(indexed.added.len(), 2, "{indexed:?}");
    }

    /// A file with no readable metadata is still a book: refusing would
    /// leave it sitting in the library with nothing showing it.
    #[test]
    fn a_file_with_unreadable_metadata_is_titled_from_its_name() {
        let (dir, cache) = library();
        write(&dir.path().join("not really a pdf.pdf"), b"this is not a PDF at all");

        let indexed = scan_and_index(dir.path(), &cache);
        assert_eq!(indexed.added.len(), 1, "{indexed:?}");
        assert_eq!(cache.field_for(indexed.added[0], "title").unwrap().as_deref(), Some("not really a pdf"));
    }

    #[test]
    fn an_empty_folder_indexes_nothing() {
        let (dir, cache) = library();
        assert_eq!(scan_and_index(dir.path(), &cache), IndexReport::default());
    }

    #[test]
    fn registering_a_file_that_is_not_there_is_an_error() {
        let (_dir, cache) = library();
        let meta = calibre_ebooks::metadata::MetaInformation::default();
        assert!(cache.register_book_in_place("nope.pdf", &meta).is_err());
    }

    /// A rescan of a folder the user filled: every file becomes a book,
    /// and nothing on disk changes.
    #[test]
    fn rescan_indexes_an_untracked_folder_in_place() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");
        write(&dir.path().join("Receipts/2019.pdf"), b"%PDF-1.4 receipt");

        let result = rescan(&cache, much_later()).unwrap();

        assert_eq!(result.indexed.added.len(), 2);
        assert_eq!(result.relocated, 0);
        assert!(result.scanned.is_complete());
        assert!(dir.path().join("Boiler Manual.pdf").exists(), "indexing must not move the file");
        assert!(dir.path().join("Receipts/2019.pdf").exists(), "nor the one in a subfolder");
    }

    /// The second rescan recognises its own earlier work rather than
    /// adding everything again.
    #[test]
    fn rescanning_twice_adds_nothing_the_second_time() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");

        assert_eq!(rescan(&cache, much_later()).unwrap().indexed.added.len(), 1);

        let second = rescan(&cache, much_later()).unwrap();
        assert!(second.indexed.added.is_empty(), "the same file must not become a second book");
        assert_eq!(second.indexed.already_known, 1);
    }

    /// The case the ordering exists for. Indexing before re-attaching
    /// would make the moved file a *new* book and leave the original
    /// pointing at nothing -- one drag in a file manager turning one
    /// book into a duplicate plus an orphan.
    #[test]
    fn rescan_reattaches_a_moved_file_instead_of_duplicating_it() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");

        let book_id = rescan(&cache, much_later()).unwrap().indexed.added[0];

        // The user files it away in their own folder.
        std::fs::create_dir_all(dir.path().join("Manuals")).unwrap();
        std::fs::rename(dir.path().join("Boiler Manual.pdf"), dir.path().join("Manuals/Boiler Manual.pdf")).unwrap();

        let result = rescan(&cache, much_later()).unwrap();

        assert_eq!(result.relocated, 1, "the move should be recognised");
        assert!(result.indexed.added.is_empty(), "a moved file is not a new book");
        assert_eq!(cache.field_for(book_id, "path").unwrap().unwrap(), "Manuals", "the record should follow the file");
    }

    /// A file the user removed from the library while keeping the file
    /// must not be silently put back by the next scan.
    #[test]
    fn rescan_respects_the_ignore_list() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");

        let book_id = rescan(&cache, much_later()).unwrap().indexed.added[0];
        crate::removal::remove(&cache, book_id, false).unwrap();

        let result = rescan(&cache, much_later()).unwrap();
        assert!(result.indexed.added.is_empty(), "a removed-but-kept file must stay out");
        assert_eq!(result.indexed.ignored, vec!["Boiler Manual.pdf"]);
    }

    /// A file still being written is not indexed yet -- half a book is
    /// worse than a book a few seconds late.
    #[test]
    fn rescan_leaves_a_settling_file_for_the_next_pass() {
        let (dir, cache) = library();
        write(&dir.path().join("half-copied.pdf"), b"%PDF-1.4");

        // Real clock: the file was written microseconds ago, so it is
        // inside the settle window.
        let result = rescan(&cache, SystemTime::now()).unwrap();
        assert!(result.indexed.added.is_empty(), "a settling file must not be indexed");
        assert_eq!(result.scanned.settling, vec!["half-copied.pdf"]);
    }

    /// Copies a library's `.calibre-oxide/` state into a fresh folder and
    /// rebuilds `metadata.db` from the change log alone.
    ///
    /// The log is meant to be the authority (#899), so anything a rebuild
    /// cannot reproduce was never really recorded.
    fn rebuild_elsewhere(source: &Path) -> (tempfile::TempDir, Cache) {
        let fresh = tempfile::tempdir().unwrap();
        let state = fresh.path().join(".calibre-oxide");
        std::fs::create_dir_all(&state).unwrap();
        for entry in walkdir::WalkDir::new(source.join(".calibre-oxide")).into_iter().filter_map(Result::ok) {
            let relative = entry.path().strip_prefix(source.join(".calibre-oxide")).unwrap();
            // `metadata.db` is the thing being rebuilt, so it must not be
            // copied -- and neither may the writer lock.
            if relative.as_os_str().is_empty() || relative.starts_with("journal") || relative.to_string_lossy().contains("writer.lock") {
                continue;
            }
            let destination = state.join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&destination).unwrap();
            } else {
                std::fs::copy(entry.path(), &destination).unwrap();
            }
        }
        let cache = Cache::new(fresh.path()).unwrap();
        cache.rebuild_from_change_log().unwrap();
        (fresh, cache)
    }

    /// A file the user moved inside their own folder is re-attached
    /// silently -- and that re-attachment has to reach the log, or the
    /// next rebuild sends the book back to a path with no file at it.
    #[test]
    fn a_relocation_survives_a_rebuild_from_the_log() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");
        let book_id = crate::scan::rescan(&cache, much_later()).unwrap().indexed.added[0];
        let uuid = cache.book_uuid(book_id).unwrap().unwrap();

        std::fs::create_dir_all(dir.path().join("Manuals")).unwrap();
        std::fs::rename(dir.path().join("Boiler Manual.pdf"), dir.path().join("Manuals/Boiler Manual.pdf")).unwrap();
        assert_eq!(crate::scan::rescan(&cache, much_later()).unwrap().relocated, 1);
        assert_eq!(cache.field_for(book_id, "path").unwrap().unwrap(), "Manuals");

        let (_fresh, rebuilt) = rebuild_elsewhere(dir.path());
        let rebuilt_id = rebuilt.book_id_for_uuid(&uuid).unwrap().expect("the book should come back from the log");
        assert_eq!(rebuilt.field_for(rebuilt_id, "path").unwrap().unwrap(), "Manuals", "the rebuild put the book back at its pre-move path, so the relocation never reached the log");
    }

    /// The same for a rename: `data.name` is the only authority for which
    /// file belongs to a book (#885), so the log has to carry it.
    #[test]
    fn a_renamed_file_survives_a_rebuild_from_the_log() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");
        let book_id = crate::scan::rescan(&cache, much_later()).unwrap().indexed.added[0];
        let uuid = cache.book_uuid(book_id).unwrap().unwrap();

        std::fs::rename(dir.path().join("Boiler Manual.pdf"), dir.path().join("boiler-v2.pdf")).unwrap();
        assert_eq!(crate::scan::rescan(&cache, much_later()).unwrap().relocated, 1);

        let (_fresh, rebuilt) = rebuild_elsewhere(dir.path());
        let rebuilt_id = rebuilt.book_id_for_uuid(&uuid).unwrap().unwrap();
        let names = rebuilt.format_file_names(rebuilt_id).unwrap();
        assert_eq!(names, vec![("PDF".to_string(), "boiler-v2".to_string())], "the rebuild lost the new filename");
    }

    /// Resolving an orphan is a real edit to where a book's file is, so
    /// it has to reach the log like any other. Goes through the same
    /// primitives as a relocation but by a different route -- the user
    /// pointing at a file rather than the scanner matching one.
    #[test]
    fn resolving_an_orphan_survives_a_rebuild_from_the_log() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");
        let book_id = crate::scan::rescan(&cache, much_later()).unwrap().indexed.added[0];
        let uuid = cache.book_uuid(book_id).unwrap().unwrap();

        // Lost the file, then found it again under a new name in a
        // subfolder -- both halves of the location change at once.
        std::fs::remove_file(dir.path().join("Boiler Manual.pdf")).unwrap();
        cache.orphans().mark(book_id, "PDF", "Boiler Manual.pdf").unwrap();
        write(&dir.path().join("Manuals/boiler-recovered.pdf"), b"%PDF-1.4 boiler");
        crate::orphans::relocate(&cache, book_id, "PDF", &dir.path().join("Manuals/boiler-recovered.pdf")).unwrap();

        let (_fresh, rebuilt) = rebuild_elsewhere(dir.path());
        let rebuilt_id = rebuilt.book_id_for_uuid(&uuid).unwrap().unwrap();
        assert_eq!(rebuilt.field_for(rebuilt_id, "path").unwrap().unwrap(), "Manuals");
        assert_eq!(rebuilt.format_file_names(rebuilt_id).unwrap(), vec![("PDF".to_string(), "boiler-recovered".to_string())]);
    }

    /// A rename must not cost the recorded hash. Dropping it would make
    /// the next scan re-read the whole file to learn what it already
    /// knew, and would make the content-match check in an orphan
    /// relocation unable to conclude anything.
    #[test]
    fn renaming_a_format_keeps_its_recorded_hash() {
        let (dir, cache) = library();
        write(&dir.path().join("Boiler Manual.pdf"), b"%PDF-1.4 boiler");
        let book_id = crate::scan::rescan(&cache, much_later()).unwrap().indexed.added[0];

        let before = cache.checksums().recorded_identity(book_id, "format", "PDF").unwrap().0;
        assert!(before.is_some(), "the scan should have recorded a hash to begin with");

        cache.set_format_name(book_id, "PDF", "renamed").unwrap();

        assert_eq!(cache.checksums().recorded_identity(book_id, "format", "PDF").unwrap().0, before, "a rename does not change the content");
    }
}
