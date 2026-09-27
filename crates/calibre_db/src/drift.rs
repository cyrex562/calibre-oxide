//! Reconciling what the library records against what is on disk
//! (issue #894, part of #889).
//!
//! In a folder the user manages by hand, files get renamed, moved and
//! deleted by other tools. This decides what actually happened.
//!
//! # Orphaning is a last resort
//!
//! A file renamed or moved *inside* the library can be **proved** to be
//! the same file — by its filesystem identity, or failing that by its
//! content. Those are re-attached silently: `data.name` and
//! `books.path` are updated and the user is never told, because nothing
//! has gone wrong. Only a file that cannot be found anywhere becomes
//! their problem.
//!
//! # Absences are resolved as a set, never one at a time
//!
//! Two books that swap filenames both look absent individually, and
//! matching each against "some untracked file" in turn can pair them
//! backwards. The whole set of absences is matched against the whole set
//! of untracked files at once, strongest evidence first.
//!
//! # A scan that failed concludes nothing
//!
//! [`crate::scan::ScanReport::is_complete`] is false whenever a
//! directory could not be read. A dropped network share, one bad
//! permission, a sleeping external disk — each makes files *look*
//! absent. Reporting those as missing would turn a temporary condition
//! into a library full of orphans, so when the scan is incomplete the
//! missing list is not computed at all and [`DriftReport::conclusive`]
//! says so.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use calibre_utils::filenames::{file_facts, FileIdentity};

use crate::cache::Cache;
use crate::scan::ScanReport;

/// A file that turned up somewhere other than where it was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub book_id: i32,
    pub format: String,
    pub from: String,
    pub to: String,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The filesystem says it is the same file. Survives an edit as well
    /// as a move, and costs one `stat`.
    Identity,
    /// Byte-identical content. Catches a move across volumes, where the
    /// identity necessarily changes.
    Content,
}

/// A recorded file whose content changed where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub book_id: i32,
    pub format: String,
    pub path: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DriftReport {
    /// Recorded files found where they should be, unchanged.
    pub unchanged: usize,
    /// Found where they should be, with different content. **An edit,
    /// not corruption** — somebody annotated a PDF.
    pub edited: Vec<Edit>,
    /// Recorded elsewhere and matched to a file found somewhere else.
    pub moved: Vec<Relocation>,
    /// Files on disk that no book claims.
    pub untracked: Vec<String>,
    /// `(book id, format)` recorded but not found anywhere. Empty and
    /// meaningless when [`DriftReport::conclusive`] is false.
    pub missing: Vec<(i32, String)>,
    /// Whether the scan saw the whole library. When false, `missing` was
    /// not computed: absence could not be distinguished from invisibility.
    pub conclusive: bool,
}

/// One recorded `(book, format)` and where it should be.
struct Recorded {
    book_id: i32,
    format: String,
    path: String,
    hash: Option<String>,
    identity: Option<FileIdentity>,
}

/// Works out what changed between the library's records and `scan`.
///
/// Read-only: nothing is written here. Applying the result is
/// [`apply_relocations`], kept separate so a caller can show the user
/// what would happen first — and so a test can assert on the decision
/// without a write.
pub fn detect(cache: &Cache, scan: &ScanReport) -> Result<DriftReport> {
    let library = cache.backend.library_path.clone();
    let recorded = recorded_files(cache)?;

    let on_disk: HashSet<&str> = scan.files.iter().map(|f| f.relative_path.as_str()).collect();
    let claimed: HashSet<&str> = recorded.iter().map(|r| r.path.as_str()).collect();

    let mut report = DriftReport { conclusive: scan.is_complete(), ..Default::default() };
    let mut absent: Vec<&Recorded> = Vec::new();

    // Which identities the library believes it owns. Needed to tell a
    // *swap* from an *edit*: when two books exchange filenames, both
    // files still exist at their recorded paths with the wrong contents,
    // which by path alone is indistinguishable from each having been
    // edited.
    let owned_identities: HashSet<FileIdentity> = recorded.iter().filter_map(|r| r.identity).collect();

    for record in &recorded {
        if !on_disk.contains(record.path.as_str()) {
            absent.push(record);
            continue;
        }

        let full = library.join(&record.path);
        let actual_identity = file_facts(&full).map(|f| f.identity);

        // Something is at our path, but is it our file? If the identity
        // differs *and* belongs to another record, the files were
        // swapped or shuffled -- this record's file is elsewhere, so
        // treat it as absent and let set matching find it.
        //
        // An identity that differs and belongs to nobody is an editor
        // that saves by writing a temporary file and renaming over the
        // original, which gives the same path a new inode. That is an
        // edit, and mistaking it for a move would chase a file that
        // never went anywhere.
        if let (Some(recorded_id), Some(actual_id)) = (record.identity, actual_identity) {
            if recorded_id != actual_id && owned_identities.contains(&actual_id) {
                absent.push(record);
                continue;
            }
        }

        // Our file, where it should be. The only question left is
        // whether the bytes are still the ones we recorded.
        match (&record.hash, hash_of(&full)) {
            (Some(recorded_hash), Some(actual)) if *recorded_hash != actual => {
                report.edited.push(Edit { book_id: record.book_id, format: record.format.clone(), path: record.path.clone() });
            }
            _ => report.unchanged += 1,
        }
    }

    // Match candidates: files no record claims, plus the paths vacated
    // by records that turned out to be holding somebody else's file. A
    // swap produces no *unclaimed* paths at all, so without the second
    // group there would be nothing to match against.
    let displaced: HashSet<&str> = absent.iter().map(|r| r.path.as_str()).filter(|p| on_disk.contains(*p)).collect();

    // Files the user removed while keeping (#896) are neither untracked
    // nor match candidates. No book claims them, so without this they
    // would be reported as unclaimed files needing attention -- which is
    // precisely the state the user chose -- and could be matched to an
    // unrelated absent book that happened to share their content.
    let ignored = cache.ignored();
    let mut untracked: Vec<&str> = scan
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .filter(|p| !claimed.contains(p) || displaced.contains(p))
        .filter(|p| {
            let hash = hash_of(&library.join(p));
            !ignored.is_ignored(p, hash.as_deref()).unwrap_or(false)
        })
        .collect();
    untracked.sort();

    // Strongest evidence first, and both passes run over the whole set:
    // two books that swapped filenames are two identity matches, where
    // matching them one at a time could pair them backwards.
    let mut taken: HashSet<&str> = HashSet::new();
    let mut still_absent: Vec<&Recorded> = Vec::new();

    let mut by_identity: HashMap<FileIdentity, &str> = HashMap::new();
    let mut by_hash: HashMap<String, Vec<&str>> = HashMap::new();
    for path in &untracked {
        let full = library.join(path);
        if let Some(facts) = file_facts(&full) {
            by_identity.entry(facts.identity).or_insert(path);
        }
        if let Some(hash) = hash_of(&full) {
            by_hash.entry(hash).or_default().push(path);
        }
    }

    for record in absent {
        let matched = record
            .identity
            .and_then(|id| by_identity.get(&id).copied())
            .filter(|p| !taken.contains(*p))
            .map(|p| (p, Evidence::Identity))
            .or_else(|| {
                record
                    .hash
                    .as_ref()
                    .and_then(|h| by_hash.get(h))
                    // Deterministic when several untracked files share
                    // content: `untracked` is sorted, so the first
                    // unclaimed one is the same choice on every machine.
                    .and_then(|paths| paths.iter().find(|p| !taken.contains(**p)).copied())
                    .map(|p| (p, Evidence::Content))
            });

        match matched {
            Some((to, evidence)) => {
                taken.insert(to);
                report.moved.push(Relocation { book_id: record.book_id, format: record.format.clone(), from: record.path.clone(), to: to.to_string(), evidence });
            }
            None => still_absent.push(record),
        }
    }

    report.untracked = untracked.iter().filter(|p| !taken.contains(**p) && !displaced.contains(**p)).map(|p| p.to_string()).collect();

    if report.conclusive {
        report.missing = still_absent.iter().map(|r| (r.book_id, r.format.clone())).collect();
    }

    report.moved.sort_by(|a, b| (a.book_id, &a.format).cmp(&(b.book_id, &b.format)));
    report.edited.sort_by(|a, b| (a.book_id, &a.format).cmp(&(b.book_id, &b.format)));
    report.missing.sort();
    Ok(report)
}

/// Re-points the database at where the files actually are.
///
/// Only the relocations: an edit needs its hash re-recorded rather than
/// its path changed, and a missing file is the user's decision (#895),
/// not something to act on automatically.
pub fn apply_relocations(cache: &Cache, report: &DriftReport) -> Result<usize> {
    for relocation in &report.moved {
        let path = std::path::Path::new(&relocation.to);
        let folder = path.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();

        cache.set_book_path(relocation.book_id, &folder)?;
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE data SET name = ?1 WHERE book = ?2 AND format = ?3", (stem, relocation.book_id, &relocation.format))?;
    }
    Ok(report.moved.len())
}

/// Every `(book, format)` the library records, with where it should be
/// and what it should contain.
fn recorded_files(cache: &Cache) -> Result<Vec<Recorded>> {
    let rows: Vec<(i32, String, String, String)> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT b.id, b.path, d.format, d.name FROM books b JOIN data d ON d.book = b.id ORDER BY b.id, d.format")?;
        let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row?);
        }
        out
    };

    let checksums = cache.checksums();
    Ok(rows
        .into_iter()
        .map(|(book_id, folder, format, name)| {
            let file = format!("{name}.{}", format.to_lowercase());
            let path = if folder.is_empty() { file } else { format!("{folder}/{file}") };
            let (hash, identity) = checksums.recorded_identity(book_id, "format", &format).unwrap_or((None, None));
            Recorded { book_id, format, path, hash, identity }
        })
        .collect())
}

fn hash_of(path: &std::path::Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
    use std::path::Path;
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

    fn scan(dir: &Path) -> ScanReport {
        walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap()
    }

    fn index(dir: &Path, cache: &Cache) {
        let report = scan(dir);
        crate::scan::index_scan(cache, dir, &report);
    }

    #[test]
    fn an_untouched_library_has_drifted_in_no_way() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        index(dir.path(), &cache);

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.unchanged, 1);
        assert!(report.moved.is_empty() && report.edited.is_empty() && report.missing.is_empty() && report.untracked.is_empty());
        assert!(report.conclusive);
    }

    /// The core promise: a rename outside the app is re-attached
    /// silently, not reported as a loss.
    #[test]
    fn a_file_renamed_outside_the_app_is_matched_not_orphaned() {
        let (dir, cache) = library();
        write(&dir.path().join("scan0001.pdf"), b"%PDF the content");
        index(dir.path(), &cache);

        std::fs::rename(dir.path().join("scan0001.pdf"), dir.path().join("Boiler Manual.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert!(report.missing.is_empty(), "{report:?}");
        assert_eq!(report.moved.len(), 1, "{report:?}");
        assert_eq!(report.moved[0].to, "Boiler Manual.pdf");
        // One `stat`, no reading of content.
        assert_eq!(report.moved[0].evidence, Evidence::Identity);
    }

    #[test]
    fn a_file_moved_to_a_subfolder_is_matched() {
        let (dir, cache) = library();
        write(&dir.path().join("invoice.pdf"), b"%PDF content");
        index(dir.path(), &cache);

        write(&dir.path().join("Receipts/placeholder"), b"");
        std::fs::rename(dir.path().join("invoice.pdf"), dir.path().join("Receipts/invoice.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 1, "{report:?}");
        assert_eq!(report.moved[0].to, "Receipts/invoice.pdf");
    }

    /// E13. Matched one at a time, these two can be paired backwards --
    /// each book would claim the other's file.
    #[test]
    fn two_files_that_swap_names_are_not_paired_backwards() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF AAAA");
        write(&dir.path().join("b.pdf"), b"%PDF BBBB");
        index(dir.path(), &cache);

        let a_id = cache.book_for_file("a.pdf").unwrap().unwrap();
        let b_id = cache.book_for_file("b.pdf").unwrap().unwrap();

        let tmp = dir.path().join("tmp.pdf");
        std::fs::rename(dir.path().join("a.pdf"), &tmp).unwrap();
        std::fs::rename(dir.path().join("b.pdf"), dir.path().join("a.pdf")).unwrap();
        std::fs::rename(&tmp, dir.path().join("b.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 2, "{report:?}");
        apply_relocations(&cache, &report).unwrap();

        // Each book must still own the bytes it always owned.
        let path_of = |id: i32| {
            let rows = cache.get_data_as_dict(None, false, None, false).unwrap();
            rows.into_iter().find(|r| r["id"] == id).unwrap()["fmt_pdf"].as_str().unwrap().to_string()
        };
        assert_eq!(std::fs::read(path_of(a_id)).unwrap(), b"%PDF AAAA");
        assert_eq!(std::fs::read(path_of(b_id)).unwrap(), b"%PDF BBBB");
    }

    /// E14. Somebody annotated a PDF. That is an edit, and calling it
    /// corruption teaches the user to ignore the warning.
    #[test]
    fn a_file_edited_in_place_is_an_edit_not_a_loss() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF original");
        index(dir.path(), &cache);

        write(&dir.path().join("a.pdf"), b"%PDF now with annotations");

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.edited.len(), 1, "{report:?}");
        assert!(report.missing.is_empty() && report.moved.is_empty() && report.untracked.is_empty());
    }

    /// An editor that saves by writing a temporary file and renaming
    /// over the original gives the same path a **new** inode. That is
    /// still an edit -- the distinction from a swap is whether the new
    /// identity belongs to another book, and here it belongs to nobody.
    ///
    /// Getting this wrong means every save in such an editor sends the
    /// app hunting for a file that never went anywhere.
    #[test]
    fn an_editor_that_saves_by_replacing_the_file_is_still_an_edit() {
        let (dir, cache) = library();
        let path = dir.path().join("a.pdf");
        write(&path, b"%PDF original");
        index(dir.path(), &cache);

        // Write-temp-then-rename, the way many editors save.
        let temp = dir.path().join("a.pdf.tmp");
        write(&temp, b"%PDF saved by replacement");
        std::fs::rename(&temp, &path).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.edited.len(), 1, "{report:?}");
        assert!(report.moved.is_empty(), "{report:?}");
        assert!(report.missing.is_empty(), "{report:?}");
        assert!(report.untracked.is_empty(), "{report:?}");
    }

    /// A move across volumes necessarily changes the identity, so only
    /// content can match it. Simulated by clearing the recorded identity,
    /// which is also what a row written before identities were stored
    /// looks like.
    #[test]
    fn a_move_with_no_usable_identity_falls_back_to_content() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF distinctive content");
        index(dir.path(), &cache);

        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE checksums_db.file_checksums SET volume = NULL, file_index = NULL", ()).unwrap();
        }
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("elsewhere.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 1, "{report:?}");
        assert_eq!(report.moved[0].evidence, Evidence::Content);
        assert_eq!(report.moved[0].to, "elsewhere.pdf");
    }

    /// A file both edited *and* moved: content cannot match it, so only
    /// the identity can. This is the case that justifies storing the
    /// identity at all.
    #[test]
    fn a_file_both_edited_and_moved_is_still_matched_by_identity() {
        let (dir, cache) = library();
        let original = dir.path().join("a.pdf");
        write(&original, b"%PDF original");
        index(dir.path(), &cache);

        // Append in place, keeping the inode, then move it.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(&original).unwrap();
            file.write_all(b" plus annotations").unwrap();
        }
        std::fs::rename(&original, dir.path().join("renamed.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 1, "content alone could not have found this: {report:?}");
        assert_eq!(report.moved[0].evidence, Evidence::Identity);
        assert!(report.missing.is_empty());
    }

    #[test]
    fn a_deleted_file_is_missing() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        index(dir.path(), &cache);
        std::fs::remove_file(dir.path().join("a.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.missing.len(), 1, "{report:?}");
        assert!(report.conclusive);
    }

    #[test]
    fn a_new_file_nobody_claims_is_untracked() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        index(dir.path(), &cache);
        write(&dir.path().join("dropped in.pdf"), b"%PDF new");

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.untracked, vec!["dropped in.pdf"]);
        assert!(report.missing.is_empty());
    }

    /// A file matched to an absent record is not *also* reported as
    /// untracked -- it is a move, and saying both would have the user
    /// resolve the same thing twice in opposite directions.
    #[test]
    fn a_matched_file_is_not_also_reported_as_untracked() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("renamed.pdf")).unwrap();

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 1);
        assert!(report.untracked.is_empty(), "{report:?}");
    }

    /// E17, and the most important test here. A scan that could not read
    /// a directory must not conclude anything is gone.
    #[cfg(unix)]
    #[test]
    fn an_incomplete_scan_reports_nothing_as_missing() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, cache) = library();
        write(&dir.path().join("readable.pdf"), b"%PDF one");
        let locked = dir.path().join("locked");
        write(&locked.join("hidden.pdf"), b"%PDF two");
        index(dir.path(), &cache);

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let scanned = scan(dir.path());
        let report = detect(&cache, &scanned).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!scanned.is_complete());
        assert!(!report.conclusive, "an incomplete scan must say so");
        // `hidden.pdf` is invisible, not gone. Reporting it missing is
        // how a dropped share becomes a library of orphans.
        assert!(report.missing.is_empty(), "{report:?}");
    }

    /// Applying a relocation leaves the book reachable -- the invariant
    /// from #885 that every one of these paths has to preserve.
    #[test]
    fn a_relocated_book_still_resolves_to_its_file() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("sub/b.pdf")).unwrap_or_else(|_| {
            std::fs::create_dir_all(dir.path().join("sub")).unwrap();
            std::fs::rename(dir.path().join("a.pdf"), dir.path().join("sub/b.pdf")).unwrap();
        });

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(apply_relocations(&cache, &report).unwrap(), 1);

        let row = cache.get_data_as_dict(None, false, None, false).unwrap()[0].clone();
        let served = row["fmt_pdf"].as_str().expect("the format must still resolve");
        assert!(Path::new(served).exists(), "{served} does not exist");
        assert_eq!(std::fs::read(served).unwrap(), b"%PDF content");
    }

    /// Re-running detect after applying must find nothing left to do.
    #[test]
    fn applying_a_relocation_settles_the_drift() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF content");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("renamed.pdf")).unwrap();

        let first = detect(&cache, &scan(dir.path())).unwrap();
        apply_relocations(&cache, &first).unwrap();

        let second = detect(&cache, &scan(dir.path())).unwrap();
        assert!(second.moved.is_empty(), "{second:?}");
        assert_eq!(second.unchanged, 1);
    }
}
