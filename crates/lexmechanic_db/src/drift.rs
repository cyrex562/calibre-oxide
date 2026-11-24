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

use lexmechanic_utils::filenames::FileIdentity;

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
    /// Content hashes computed for untracked files while looking for moved
    /// files, by relative path. Only files whose size equals that of a
    /// recorded file that was not found are ever read, and a rescan hands
    /// these to indexing so nothing is read twice.
    pub hashes: HashMap<String, crate::scan_pipeline::FileDigest>,
}

/// One recorded `(book, format)` and where it should be.
struct Recorded {
    book_id: i32,
    format: String,
    path: String,
    hash: Option<String>,
    size: Option<i64>,
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
    let on_disk_facts: HashMap<&str, &lexmechanic_utils::filenames::FileFacts> = scan.files.iter().map(|f| (f.relative_path.as_str(), &f.facts)).collect();
    let claimed: HashSet<&str> = recorded.iter().map(|r| r.path.as_str()).collect();

    let mut report = DriftReport { conclusive: scan.is_complete(), ..Default::default() };
    let mut absent: Vec<&Recorded> = Vec::new();

    // Which identities the library believes it owns. Needed to tell a
    // *swap* from an *edit*: when two books exchange filenames, both
    // files still exist at their recorded paths with the wrong contents,
    // which by path alone is indistinguishable from each having been
    // edited.
    let owned_identities: HashSet<FileIdentity> = recorded.iter().filter_map(|r| r.identity).collect();

    // The identity of the file at `path`: from the walk, or, when the walk left
    // identities unread, from one open of just that file (and remembered).
    let deferred = scan.identity_deferred;
    let looked_up: std::cell::RefCell<HashMap<String, Option<FileIdentity>>> = Default::default();
    let identity_at = |path: &str| -> Option<FileIdentity> {
        if !deferred {
            return on_disk_facts.get(path).map(|f| f.identity);
        }
        *looked_up.borrow_mut().entry(path.to_string()).or_insert_with(|| lexmechanic_utils::filenames::identity(&library.join(path)))
    };

    for record in &recorded {
        if !on_disk.contains(record.path.as_str()) {
            absent.push(record);
            continue;
        }

        let full = library.join(&record.path);
        // With identities unread, a file whose size still matches the record is
        // taken as ours without opening it; only a changed size is a candidate
        // for the swap check below. (Two files of one size that swapped names
        // are not noticed by a check that does not open every file.)
        let size_matches_early = matches!((record.size, on_disk_facts.get(record.path.as_str())), (Some(recorded), Some(actual)) if recorded as u64 == actual.size);
        if deferred && size_matches_early {
            report.unchanged += 1;
            continue;
        }
        let actual_identity = identity_at(record.path.as_str());

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
        //
        // Answered from the size the walk already read off the directory
        // entry. Hashing every recorded file here meant every rescan read
        // the whole library -- hundreds of gigabytes on a NAS -- to learn
        // that nothing had changed. A file whose size differs is read and
        // compared; one whose size matches is taken as unchanged. That
        // misses an edit that keeps the exact byte count, which is the
        // price: nothing in a rescan acts on `edited`, and Check Library's
        // checksum verification is the full-content check.
        let size_matches = matches!((record.size, on_disk_facts.get(record.path.as_str())), (Some(recorded), Some(actual)) if recorded as u64 == actual.size);
        if size_matches {
            report.unchanged += 1;
            continue;
        }
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
    //
    // By path only, from one snapshot of the list -- not by hashing every
    // unclaimed file, which on a first scan is every file in the folder.
    // The content half of the ignore check is applied below, but only to
    // files that are about to be hashed anyway.
    let (ignored_paths, ignored_hashes) = cache.ignored().snapshot().unwrap_or_default();
    let mut untracked: Vec<&str> = scan
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .filter(|p| !claimed.contains(p) || displaced.contains(p))
        .filter(|p| !ignored_paths.contains(*p))
        .collect();
    untracked.sort();

    // Strongest evidence first, and both passes run over the whole set:
    // two books that swapped filenames are two identity matches, where
    // matching them one at a time could pair them backwards.
    let mut taken: HashSet<&str> = HashSet::new();
    let mut still_absent: Vec<&Recorded> = Vec::new();

    let mut by_identity: HashMap<FileIdentity, &str> = HashMap::new();
    if !deferred {
        for path in &untracked {
            if let Some(facts) = on_disk_facts.get(*path) {
                by_identity.entry(facts.identity).or_insert(path);
            }
        }
    } else if absent.iter().any(|r| r.identity.is_some()) {
        // Only when something went missing does an untracked file's identity matter (it
        // may be that thing, moved or renamed -- even edited since); a folder with
        // nothing missing, which is the usual check, opens none of them.
        for path in &untracked {
            if let Some(identity) = identity_at(path) {
                by_identity.entry(identity).or_insert(path);
            }
        }
    }

    // Content matching reads files, so it reads as few as it can: only the
    // untracked files whose size equals the recorded size of an absent record
    // that identity could not match and that has a hash to match on (a file
    // of another size cannot have its content). A first scan of a folder
    // (nothing absent) and a rescan where every move kept its identity read
    // nothing here; one absent file used to make this read -- whole, into
    // memory, one at a time -- every untracked file in the library, which
    // indexing then hashed again.
    let wanted_sizes: HashSet<u64> = absent
        .iter()
        .filter(|r| r.hash.is_some() && !r.identity.is_some_and(|id| by_identity.contains_key(&id)))
        .filter_map(|r| r.size.and_then(|s| u64::try_from(s).ok()))
        .collect();
    let mut by_hash: HashMap<String, Vec<&str>> = HashMap::new();
    if !wanted_sizes.is_empty() {
        let candidates: Vec<&str> = untracked.iter().copied().filter(|p| on_disk_facts.get(*p).is_some_and(|f| wanted_sizes.contains(&f.size))).collect();
        let control = crate::scan_pipeline::JobControl::default();
        let digests = crate::scan_pipeline::parallel_map(&candidates, crate::scan_pipeline::default_workers(), &control, |path| crate::scan_pipeline::hash_stream(&library.join(path), None).ok());
        for (path, digest) in candidates.iter().zip(digests) {
            let Some(Some(digest)) = digest else { continue };
            report.hashes.insert(path.to_string(), digest.clone());
            // A file the user removed and kept, found by content after
            // being renamed, is not a candidate for anything.
            if ignored_hashes.contains(&digest.hash) {
                continue;
            }
            by_hash.entry(digest.hash).or_default().push(path);
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
        // Through `set_format_name` rather than raw SQL so the rename
        // reaches the change log: the log is the authority (#899), and a
        // relocation it never saw is one the next rebuild undoes.
        cache.set_format_name(relocation.book_id, &relocation.format, stem)?;
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

    // One query for every record, not one per file.
    let mut records = cache.checksums().all_format_records().unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|(book_id, folder, format, name)| {
            let file = format!("{name}.{}", format.to_lowercase());
            let path = if folder.is_empty() { file } else { format!("{folder}/{file}") };
            let (hash, size, identity) = match records.remove(&(book_id, format.clone())) {
                Some((hash, size, identity)) => (Some(hash), Some(size), identity),
                None => (None, None, None),
            };
            Recorded { book_id, format, path, hash, size, identity }
        })
        .collect())
}

fn hash_of(path: &std::path::Path) -> Option<String> {
    crate::scan_pipeline::hash_stream(path, None).ok().map(|digest| digest.hash)
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

    /// One absent file used to make the drift pass read *every* untracked file
    /// (whole, into memory, one at a time) to look for it by content, and
    /// indexing then hashed them all again. Now only untracked files of the
    /// absent file's size are read, and indexing reuses what was read.
    #[test]
    fn one_absent_file_does_not_make_the_drift_pass_read_every_untracked_file() {
        let (dir, cache) = library();
        let original = b"%PDF the original content, a bit long".to_vec();
        write(&dir.path().join("keeper.pdf"), b"%PDF keeper content stays");
        write(&dir.path().join("lost.pdf"), &original);
        index(dir.path(), &cache);

        // `lost.pdf` is copied elsewhere (a new identity, so only its content
        // can find it) and removed; a decoy has its size but other bytes; and
        // many unrelated files turn up, all of other sizes.
        std::fs::remove_file(dir.path().join("lost.pdf")).unwrap();
        write(&dir.path().join("elsewhere/lost-copy.pdf"), &original);
        let mut decoy = original.clone();
        *decoy.last_mut().unwrap() ^= 0x55;
        write(&dir.path().join("decoy.pdf"), &decoy);
        for i in 0..25 {
            write(&dir.path().join(format!("new/unrelated{i:02}.pdf")), format!("%PDF unrelated {i} {}", "x".repeat(100 + i)).as_bytes());
        }

        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert_eq!(report.moved.len(), 1, "{report:?}");
        assert_eq!((report.moved[0].to.as_str(), report.moved[0].evidence), ("elsewhere/lost-copy.pdf", Evidence::Content));
        let mut read: Vec<&str> = report.hashes.keys().map(String::as_str).collect();
        read.sort();
        assert_eq!(read, vec!["decoy.pdf", "elsewhere/lost-copy.pdf"], "only files of the absent file's size were read");
        assert!(report.hashes.values().all(|d| d.size == original.len() as u64));

        // And a rescan indexes the rest without reading the decoy again: 25
        // unrelated files are opened once each, the decoy not at all.
        let control = crate::scan_pipeline::JobControl::default();
        let options = crate::scan_pipeline::PipelineOptions { workers: 1, batch_size: 10, ..Default::default() };
        let rescan = crate::scan::rescan_files(&cache, SystemTime::now() + Duration::from_secs(3600), &options, &control).unwrap();
        assert_eq!(rescan.relocated, 1);
        assert_eq!(rescan.indexed.added.len(), 26, "{:?}", rescan.indexed.failed);
        assert_eq!(control.file_opens.load(std::sync::atomic::Ordering::Relaxed), 25, "the decoy's hash came from the drift pass");
        let decoy_id = cache.book_for_file("decoy.pdf").unwrap().unwrap();
        let (hash, _) = cache.checksums().recorded_identity(decoy_id, "format", "PDF").unwrap();
        assert_eq!(hash.unwrap(), blake3::hash(&decoy).to_hex().to_string());
    }

    /// Nothing absent, nothing to look for: no untracked file is read.
    #[test]
    fn with_nothing_absent_the_drift_pass_reads_no_untracked_file() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        index(dir.path(), &cache);
        for i in 0..5 {
            write(&dir.path().join(format!("new{i}.pdf")), format!("%PDF new {i}").as_bytes());
        }
        let report = detect(&cache, &scan(dir.path())).unwrap();
        assert!(report.hashes.is_empty());
        assert_eq!(report.untracked.len(), 5);
    }

    // -----------------------------------------------------------------
    // A walk that leaves identities unread (Check Library, tranche 29c)
    // -----------------------------------------------------------------

    fn lazy_scan(dir: &Path) -> ScanReport {
        walk(dir, &ScanOptions { lazy_identity: true, ..ScanOptions::default() }, SystemTime::now() + Duration::from_secs(3600)).unwrap()
    }

    /// The same library, eagerly and lazily, must give the same answer: that is what makes
    /// the cheap walk safe to use for the check.
    fn same_answer(cache: &Cache, dir: &Path) -> DriftReport {
        let eager = detect(cache, &scan(dir)).unwrap();
        let lazy = detect(cache, &lazy_scan(dir)).unwrap();
        assert_eq!(eager.unchanged, lazy.unchanged);
        assert_eq!(eager.edited, lazy.edited);
        assert_eq!(eager.moved, lazy.moved);
        assert_eq!(eager.untracked, lazy.untracked);
        assert_eq!(eager.missing, lazy.missing);
        assert_eq!(eager.conclusive, lazy.conclusive);
        lazy
    }

    #[test]
    fn a_lazy_walk_reads_no_identity_and_says_so() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF one");
        index(dir.path(), &cache);
        let report = lazy_scan(dir.path());
        assert!(report.identity_deferred);
        assert!(report.files.iter().all(|f| f.facts.identity == FileIdentity { volume: 0, index: 0 } && f.facts.size > 0));
        assert!(!scan(dir.path()).identity_deferred);
    }

    /// Every kind of change at once: untouched, edited (new size), renamed, renamed and
    /// edited, deleted, and a new file.
    #[test]
    fn a_lazy_walk_finds_what_an_eager_one_finds() {
        let (dir, cache) = library();
        for name in ["same", "edited", "renamed", "renamed-edited", "deleted"] {
            write(&dir.path().join(format!("{name}.pdf")), format!("%PDF original {name} content").as_bytes());
        }
        write(&dir.path().join("sub/deep.pdf"), b"%PDF deep original");
        index(dir.path(), &cache);

        write(&dir.path().join("edited.pdf"), b"%PDF edited with annotations added");
        std::fs::rename(dir.path().join("renamed.pdf"), dir.path().join("sub/renamed elsewhere.pdf")).unwrap();
        std::fs::rename(dir.path().join("renamed-edited.pdf"), dir.path().join("renamed-edited-new.pdf")).unwrap();
        write(&dir.path().join("renamed-edited-new.pdf"), b"%PDF this one was renamed and edited as well");
        std::fs::remove_file(dir.path().join("deleted.pdf")).unwrap();
        write(&dir.path().join("brand new.pdf"), b"%PDF new");

        let lazy = same_answer(&cache, dir.path());
        assert_eq!(lazy.moved.len(), 2, "{lazy:?}");
        assert_eq!(lazy.edited.len(), 1);
        assert_eq!(lazy.missing.len(), 1);
        assert_eq!(lazy.untracked, vec!["brand new.pdf".to_string()]);
    }

    /// Files that differ in size are still caught when two swap names.
    #[test]
    fn a_lazy_walk_still_sees_a_swap_of_files_of_different_sizes() {
        let (dir, cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF the first, short");
        write(&dir.path().join("b.pdf"), b"%PDF the second one is a good deal longer than the first");
        index(dir.path(), &cache);
        std::fs::rename(dir.path().join("a.pdf"), dir.path().join("tmp.pdf")).unwrap();
        std::fs::rename(dir.path().join("b.pdf"), dir.path().join("a.pdf")).unwrap();
        std::fs::rename(dir.path().join("tmp.pdf"), dir.path().join("b.pdf")).unwrap();

        let lazy = same_answer(&cache, dir.path());
        assert!(lazy.edited.is_empty() && lazy.missing.is_empty(), "{lazy:?}");
        assert_eq!(lazy.moved.len(), 2);
    }

    /// With nothing missing no file needs its identity, however many there are (the eager
    /// walk opened every one); the answer is the same.
    #[test]
    fn a_lazy_check_of_an_unchanged_library_finds_nothing_to_report() {
        let (dir, cache) = library();
        for i in 0..20 {
            write(&dir.path().join(format!("f{i}.pdf")), format!("%PDF number {i}").as_bytes());
        }
        index(dir.path(), &cache);
        let report = detect(&cache, &lazy_scan(dir.path())).unwrap();
        assert_eq!(report.unchanged, 20);
        assert!(report.moved.is_empty() && report.missing.is_empty() && report.untracked.is_empty());
    }
}
