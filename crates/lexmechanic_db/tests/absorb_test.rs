//! Absorbing a calibre library that sits inside a Lexmechanic library (tranche
//! 14a), against the library `common/mod.rs` builds the way calibre does
//! (calibre's own schema, `create_custom_column`, folder layout), including
//! its normalized custom columns.

mod common;

use lexmechanic_db::absorb::{absorb_library, find_absorbable, unabsorb_library, AbsorbOptions, ABSORBED_DB_NAME, LEGACY_ABSORBED_DB_NAME};
use lexmechanic_db::cache::Cache;
use lexmechanic_db::scan::{count_untracked, rescan_files};
use lexmechanic_db::scan_pipeline::{JobControl, PipelineOptions};
use common::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A Lexmechanic library with a calibre library copied into `nested` inside it.
fn setup(nested: &str) -> (tempfile::TempDir, Cache, CalibreFixture) {
    let fixture = build_calibre_library();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    copy_dir(fixture.root(), &dir.path().join(nested));
    (dir, cache, fixture)
}

fn run(cache: &Cache, folder: &str) -> lexmechanic_db::absorb::AbsorbReport {
    absorb_library(cache, folder, &AbsorbOptions::default(), &PipelineOptions::with_workers(2), &JobControl::default()).unwrap()
}

/// Every file under `dir`, with its bytes' hash, relative to `dir`, except
/// the database (whose name is the one thing absorbing changes).
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, blake3::hash(&std::fs::read(&path).unwrap()).to_hex().to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn book(id: i32) -> &'static FixtureBook {
    BOOKS.iter().find(|b| b.id == id).unwrap()
}

fn set_of(s: Option<String>, sep: &str) -> BTreeSet<String> {
    s.map(|s| s.split(sep).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()).unwrap_or_default()
}

fn much_later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

fn custom(cache: &Cache, id: i32, label: &str) -> Option<String> {
    cache.get_custom_column_value(id, label).unwrap_or_else(|e| panic!("reading #{label} of book {id} failed: {e:#}"))
}

#[test]
fn everything_the_database_knows_is_carried() {
    let (_dir, cache, _fixture) = setup("Shelf/Calibre");
    let report = run(&cache, "Shelf/Calibre");
    assert_eq!(report.absorbed, 4, "{report:?}");
    assert!(report.failed.is_empty() && report.missing_files.is_empty(), "{report:?}");
    assert!(report.renamed, "{report:?}");
    assert_eq!(report.formats, 5);
    assert_eq!(report.covers, 3);
    assert_eq!(report.custom_columns_created.len(), CUSTOM_COLUMNS.len());

    // Source ids 1..4 became 1..4 in the fresh library.
    let mut ids = cache.all_book_ids().unwrap();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3, 4]);

    for b in BOOKS {
        let id = b.id;
        let path = format!("Shelf/Calibre/{}", b.rel_dir());
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some(path.as_str()), "path of {id}");
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some(b.title));
        assert_eq!(cache.field_for(id, "sort").unwrap().as_deref(), Some(b.sort));
        assert_eq!(cache.field_for(id, "author_sort").unwrap().as_deref(), Some(b.author_sort));
        assert_eq!(cache.field_for(id, "comments").unwrap().as_deref(), b.comments);
        assert_eq!(cache.field_for(id, "publisher").unwrap().as_deref(), b.publisher);
        let authors: Vec<&str> = b.authors.iter().map(|a| a.0).collect();
        assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some(authors.join(" & ").as_str()));
        assert_eq!(set_of(cache.field_for(id, "tags").unwrap(), ","), b.tags.iter().map(|t| t.to_string()).collect::<BTreeSet<_>>());
        let langs = cache.field_for(id, "languages").unwrap().map(|s| s.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(langs, b.languages.to_vec(), "languages of {id}, in order");
        assert_eq!(set_of(cache.field_for(id, "identifiers").unwrap(), ","), b.identifiers.iter().map(|(k, v)| format!("{k}:{v}")).collect::<BTreeSet<_>>());
        assert_eq!(cache.field_for(id, "rating").unwrap().map(|r| r.parse::<f64>().unwrap() as i32), b.rating);
        assert_eq!(cache.field_for(id, "series").unwrap().as_deref(), b.series.map(|s| s.0));
        if let Some((_, idx)) = b.series {
            assert_eq!(cache.field_for(id, "series_index").unwrap().unwrap().parse::<f64>().unwrap(), idx);
        }
        // The date added is the source's, not "now".
        let ts = cache.field_for(id, "timestamp").unwrap().unwrap();
        assert!(ts.starts_with(&b.timestamp[..19]), "timestamp of {id}: {ts} vs {}", b.timestamp);
        let pd = cache.field_for(id, "pubdate").unwrap().unwrap();
        assert!(pd.starts_with(&b.pubdate[..19]), "pubdate of {id}: {pd}");
        let last_modified: String = cache.backend.conn.lock().unwrap().query_row("SELECT last_modified FROM books WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert!(last_modified.starts_with(&b.timestamp[..19]), "last_modified of {id}: {last_modified}");

        // Formats: real names and sizes.
        let infos = cache.format_file_info(id).unwrap();
        assert_eq!(infos.iter().map(|i| i.fmt.to_uppercase()).collect::<BTreeSet<_>>(), b.formats.iter().map(|f| f.to_string()).collect::<BTreeSet<_>>(), "formats of {id}");
        for info in &infos {
            assert!(info.path.is_file(), "{}", info.path.display());
            assert_eq!(info.size as usize, b.format_bytes(&info.fmt.to_uppercase()).len());
            assert_eq!(info.path.file_stem().unwrap().to_string_lossy(), b.file_stem());
        }

        // The cover is the calibre file beside the book.
        assert_eq!(cache.has_cover(id).unwrap(), b.has_cover, "has_cover of {id}");
        if b.has_cover {
            let cover = lexmechanic_db::covers::cover_path(&cache, id).unwrap();
            assert!(cover.ends_with(format!("{}/cover.jpg", b.rel_dir())), "{}", cover.display());
            assert_eq!(std::fs::read(&cover).unwrap(), b.cover_bytes());
        }

        // Custom columns, every kind.
        assert_eq!(custom(&cache, id, "notes").as_deref(), b.custom.notes);
        assert_eq!(custom(&cache, id, "pages"), b.custom.pages.map(|p| p.to_string()));
        assert_eq!(custom(&cache, id, "score").map(|s| s.parse::<f64>().unwrap()), b.custom.score);
        assert_eq!(custom(&cache, id, "read"), b.custom.read.map(|v| (v as i32).to_string()));
        assert_eq!(custom(&cache, id, "finished").as_deref(), b.custom.finished);
        assert_eq!(set_of(custom(&cache, id, "genre"), ","), b.custom.genre.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
        assert_eq!(custom(&cache, id, "mood").as_deref(), b.custom.mood);
        assert_eq!(custom(&cache, id, "stars").map(|s| s.parse::<f64>().unwrap() as i64), b.custom.stars);
        match b.custom.saga {
            None => assert_eq!(custom(&cache, id, "saga"), None),
            Some((name, extra)) => {
                assert_eq!(custom(&cache, id, "saga").as_deref(), Some(name));
                assert_eq!(cache.get_custom_series_index(id, "saga").unwrap(), Some(extra), "series index of #saga on {id}");
            }
        }
    }
    // The columns were created with the same definition.
    let map = cache.custom_column_label_map().unwrap();
    for c in CUSTOM_COLUMNS {
        assert_eq!(map[c.label]["datatype"], c.datatype);
        assert_eq!(map[c.label]["name"], c.name);
        assert_eq!(map[c.label]["is_multiple"], c.is_multiple);
    }
    assert_eq!(map["mood"]["display"]["enum_values"], serde_json::json!(["Happy", "Sad", "Tense"]));

    // Authors keep their sort.
    let sort: String = cache.backend.conn.lock().unwrap().query_row("SELECT sort FROM authors WHERE name = 'Terry Pratchett'", [], |r| r.get(0)).unwrap();
    assert_eq!(sort, "Pratchett, Terry");
    // Two authors on one book, in order.
    assert_eq!(cache.field_for(3, "authors").unwrap().as_deref(), Some("Terry Pratchett & Neil Gaiman"));
}

#[test]
fn files_are_untouched_and_the_database_is_only_renamed() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    let nested = dir.path().join("Shelf/Calibre");
    let db_before = std::fs::read(nested.join("metadata.db")).unwrap();
    let mut before = snapshot(&nested);
    before.remove("metadata.db");

    let report = run(&cache, "Shelf/Calibre");
    assert!(report.renamed);

    assert!(!nested.join("metadata.db").exists());
    assert_eq!(std::fs::read(nested.join(ABSORBED_DB_NAME)).unwrap(), db_before, "the nested database was never written");
    let mut after = snapshot(&nested);
    assert_eq!(after.remove(ABSORBED_DB_NAME).is_some(), true);
    assert_eq!(after, before, "no file was added, removed, moved or changed");
}

#[test]
fn a_rescan_afterwards_finds_nothing_untracked_and_no_duplicates() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    // Before: the scan steps around the folder.
    let before = count_untracked(&cache, much_later(), 1000).unwrap();
    assert_eq!(before.untracked, 0, "a live nested library is not scanned: {before:?}");

    run(&cache, "Shelf/Calibre");

    let counted = count_untracked(&cache, much_later(), 1000).unwrap();
    assert_eq!(counted.untracked, 0, "{counted:?}");
    assert!(counted.tracked >= 4, "the absorbed files are seen, and known: {counted:?}");

    let rescan = rescan_files(&cache, much_later(), &PipelineOptions::with_workers(1), &JobControl::default()).unwrap();
    assert!(rescan.scanned.nested_libraries.is_empty(), "{:?}", rescan.scanned.nested_libraries);
    assert!(rescan.indexed.added.is_empty(), "{:?}", rescan.indexed);
    assert!(rescan.indexed.already_known >= 4);
    assert_eq!(rescan.relocated, 0);
    assert_eq!(cache.all_book_ids().unwrap().len(), 4);
    let groups = lexmechanic_db::dedupe::scan_library(&cache, lexmechanic_db::dedupe::Strength::Exact).unwrap();
    assert!(groups.is_empty(), "no book is a duplicate of itself: {groups:?}");
    drop(dir);
}

#[test]
fn content_hashes_are_recorded_by_the_job_itself() {
    let (_dir, cache, _fixture) = setup("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");
    assert!(cache.checksums().formats_without_hash().unwrap().is_empty(), "the hash backfill has nothing left to do");
    let control = JobControl::default();
    let backfill = lexmechanic_db::scan_pipeline::backfill_hashes(&cache, &PipelineOptions::with_workers(1), &control).unwrap();
    assert_eq!(backfill.total, 0);
}

#[test]
fn a_file_duplicated_between_an_absorbed_library_and_a_loose_folder_is_a_content_duplicate() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");
    // A loose copy of The Hobbit's EPUB, with another name, elsewhere.
    std::fs::create_dir_all(dir.path().join("Loose")).unwrap();
    std::fs::write(dir.path().join("Loose/hobbit scan copy.epub"), book(1).format_bytes("EPUB")).unwrap();
    let rescan = rescan_files(&cache, much_later(), &PipelineOptions::with_workers(1), &JobControl::default()).unwrap();
    assert_eq!(rescan.indexed.added.len(), 1, "{:?}", rescan.indexed);
    let loose = rescan.indexed.added[0];

    let groups = lexmechanic_db::dedupe::scan_library(&cache, lexmechanic_db::dedupe::Strength::Content).unwrap();
    let group = groups.iter().find(|g| g.book_ids.contains(&loose)).expect("the loose copy is grouped");
    assert!(group.book_ids.contains(&1), "{group:?}");
    assert_eq!(group.strength, lexmechanic_db::dedupe::Strength::Content);
}

#[test]
fn a_cancelled_absorb_resumes_where_it_stopped() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    let mut options = PipelineOptions::with_workers(1);
    options.batch_size = 1;
    options.after_batch = Some(Arc::new(|c: &JobControl| {
        if c.added.load(std::sync::atomic::Ordering::Relaxed) >= 2 {
            c.cancel();
        }
    }));
    let first = absorb_library(&cache, "Shelf/Calibre", &AbsorbOptions::default(), &options, &JobControl::default()).unwrap();
    assert!(first.cancelled);
    assert_eq!(first.absorbed, 2, "{first:?}");
    assert!(!first.renamed, "an unfinished absorb leaves the database alone");
    assert!(dir.path().join("Shelf/Calibre/metadata.db").exists());
    assert_eq!(cache.all_book_ids().unwrap().len(), 2);

    let progress = JobControl::default();
    let second = absorb_library(&cache, "Shelf/Calibre", &AbsorbOptions::default(), &PipelineOptions::with_workers(1), &progress).unwrap();
    assert!(!second.cancelled);
    assert_eq!(second.already_absorbed, 2);
    assert_eq!(second.absorbed, 2);
    assert!(second.renamed);
    let snap = progress.snapshot();
    assert_eq!((snap.total, snap.added, snap.already_known), (4, 2, 2));
    let mut ids = cache.all_book_ids().unwrap();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3, 4]);

    // And a third run has nothing to do (the folder is absorbed).
    let third = absorb_library(&cache, "Shelf/Calibre", &AbsorbOptions::default(), &PipelineOptions::with_workers(1), &JobControl::default());
    assert!(third.unwrap_err().to_string().contains("absorbed already"));
}

#[test]
fn what_cannot_be_absorbed_is_refused() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    let outside = tempfile::tempdir().unwrap();
    copy_dir(_fixture.root(), &outside.path().join("Elsewhere"));

    let err = |folder: &str| -> String { absorb_library(&cache, folder, &AbsorbOptions::default(), &PipelineOptions::with_workers(1), &JobControl::default()).unwrap_err().to_string() };

    // Outside the library, by absolute path and by `..`.
    assert!(err(&outside.path().join("Elsewhere").to_string_lossy()).contains("outside the library"));
    assert!(err("../nowhere").contains("not a folder"));
    // The library itself.
    assert!(err(&dir.path().to_string_lossy()).contains("the library itself"));
    // A folder with no database.
    std::fs::create_dir_all(dir.path().join("Loose")).unwrap();
    assert!(err("Loose").contains("does not hold a calibre library"));
    // Something that is not a calibre database.
    std::fs::create_dir_all(dir.path().join("Junk")).unwrap();
    std::fs::write(dir.path().join("Junk/metadata.db"), b"this is not sqlite at all, but long enough to be looked at").unwrap();
    assert!(!err("Junk").is_empty());
    // A Lexmechanic library of its own.
    std::fs::create_dir_all(dir.path().join("Other/.lexmechanic")).unwrap();
    std::fs::write(dir.path().join("Other/metadata.db"), b"x").unwrap();
    assert!(err("Other").contains("not a calibre library"));
    // Calibre mid-write: a journal with content.
    std::fs::write(dir.path().join("Shelf/Calibre/metadata.db-journal"), b"hot").unwrap();
    assert!(err("Shelf/Calibre").contains("appears to be using"));
    // Nothing was registered by any of the refusals.
    assert!(cache.all_book_ids().unwrap().is_empty());
}

#[cfg(windows)]
#[test]
fn a_database_another_program_holds_open_is_refused() {
    use std::os::windows::fs::OpenOptionsExt;
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    // What calibre holding the file looks like: an open handle that allows no
    // other opener.
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.path().join("Shelf/Calibre/metadata.db")).unwrap();
    let err = absorb_library(&cache, "Shelf/Calibre", &AbsorbOptions::default(), &PipelineOptions::with_workers(1), &JobControl::default()).unwrap_err().to_string();
    assert!(err.contains("appears to be using") || err.contains("open"), "{err}");
    drop(held);
    assert_eq!(run(&cache, "Shelf/Calibre").absorbed, 4);
}

#[test]
fn a_second_copy_of_a_library_gets_new_uuids_and_a_recorded_mapping() {
    let (dir, cache, fixture) = setup("A");
    copy_dir(fixture.root(), &dir.path().join("B"));
    let a = run(&cache, "A");
    assert!(a.uuid_remapped.is_empty());
    let b = run(&cache, "B");
    assert_eq!(b.absorbed, 4);
    assert_eq!(b.uuid_remapped.len(), 4, "{b:?}");
    for r in &b.uuid_remapped {
        assert_ne!(r.source_uuid, r.new_uuid);
        assert_eq!(cache.book_uuid(r.book_id).unwrap().as_deref(), Some(r.new_uuid.as_str()));
    }
    // The first copy kept the calibre uuids.
    let uuids: BTreeSet<String> = (1..=4).map(|id| cache.book_uuid(id).unwrap().unwrap()).collect();
    assert_eq!(uuids.len(), 4);
    assert!(uuids.iter().all(|u| u.starts_with("00000000-0000-4000-8000-")));
    // The mapping is on disk too.
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join(".lexmechanic/absorbed-libraries.json")).unwrap()).unwrap();
    assert_eq!(state["B"]["uuid_remapped"].as_object().unwrap().len(), 4);
    assert_eq!(state["B"]["state"], "absorbed");
    // The duplicated books are now visible as duplicates of each other.
    let groups = lexmechanic_db::dedupe::scan_library(&cache, lexmechanic_db::dedupe::Strength::Content).unwrap();
    assert!(!groups.is_empty());
}

#[test]
fn a_custom_column_with_another_type_here_is_reported_and_skipped() {
    let (_dir, cache, _fixture) = setup("Shelf/Calibre");
    cache.add_custom_column("pages", "Pages", "text", false).unwrap();
    let report = run(&cache, "Shelf/Calibre");
    assert_eq!(report.absorbed, 4);
    assert_eq!(report.custom_column_conflicts.len(), 1, "{report:?}");
    assert!(report.custom_column_conflicts[0].starts_with("#pages"));
    assert_eq!(custom(&cache, 1, "pages"), None);
    assert_eq!(custom(&cache, 1, "mood").as_deref(), Some("Happy"));
    assert!(!report.custom_columns_created.contains(&"pages".to_string()));
}

#[test]
fn missing_files_are_reported_and_the_rest_of_the_book_is_kept() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    let b = book(1);
    std::fs::remove_file(b.format_path(&dir.path().join("Shelf/Calibre"), "TXT")).unwrap();
    std::fs::remove_file(dir.path().join("Shelf/Calibre").join(b.rel_dir()).join("cover.jpg")).unwrap();
    let report = run(&cache, "Shelf/Calibre");
    assert_eq!(report.absorbed, 4);
    assert_eq!(report.missing_count, 2, "{report:?}");
    assert_eq!(cache.format_file_info(1).unwrap().len(), 1);
    assert!(!cache.has_cover(1).unwrap());
}

#[test]
fn unabsorbing_puts_the_database_back_and_removes_only_rows() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    let nested = dir.path().join("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");
    let before = snapshot(&nested);

    let report = unabsorb_library(&cache, "Shelf/Calibre", &JobControl::default()).unwrap();
    assert_eq!(report.removed, 4);
    assert!(report.database_restored);
    assert!(cache.all_book_ids().unwrap().is_empty());
    assert!(nested.join("metadata.db").exists() && !nested.join(ABSORBED_DB_NAME).exists());
    let mut after = snapshot(&nested);
    let mut expected = before;
    // Same files, the database under its own name again.
    let db = expected.remove(ABSORBED_DB_NAME).unwrap();
    expected.insert("metadata.db".to_string(), db);
    after.retain(|_, _| true);
    assert_eq!(after, expected);
    // The scan steps around it again, and it can be absorbed once more.
    assert_eq!(count_untracked(&cache, much_later(), 100).unwrap().untracked, 0);
    assert_eq!(run(&cache, "Shelf/Calibre").absorbed, 4);
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join(".lexmechanic/absorbed-libraries.json")).unwrap()).unwrap();
    assert_eq!(state["Shelf/Calibre"]["state"], "absorbed");
}

#[test]
fn renaming_the_database_back_by_hand_is_the_manual_undo() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");
    let nested = dir.path().join("Shelf/Calibre");
    std::fs::rename(nested.join(ABSORBED_DB_NAME), nested.join("metadata.db")).unwrap();
    // The folder is a calibre library again: the scan steps around it, and
    // it is listed as available (its four books are in already).
    let list = find_absorbable(&cache, Duration::from_secs(5)).unwrap();
    assert_eq!(list.libraries.len(), 1);
    assert_eq!(list.libraries[0].state, "available");
    assert_eq!(list.libraries[0].already_absorbed, 4);
    // Absorbing again finishes at once and does not duplicate anything.
    let report = run(&cache, "Shelf/Calibre");
    assert_eq!((report.absorbed, report.already_absorbed), (0, 4));
    assert_eq!(cache.all_book_ids().unwrap().len(), 4);
}

#[test]
fn nested_libraries_are_listed_with_their_book_counts() {
    let (dir, cache, fixture) = setup("Shelf/Calibre");
    copy_dir(fixture.root(), &dir.path().join("Deep/Er/Another"));
    std::fs::create_dir_all(dir.path().join("Loose/Books")).unwrap();
    std::fs::write(dir.path().join("Loose/Books/a.pdf"), b"%PDF").unwrap();

    let list = find_absorbable(&cache, Duration::from_secs(5)).unwrap();
    assert!(list.conclusive);
    let listed: Vec<(&str, &str, u64)> = list.libraries.iter().map(|l| (l.path.as_str(), l.state, l.books)).collect();
    assert_eq!(listed, vec![("Deep/Er/Another", "available", 4), ("Shelf/Calibre", "available", 4)]);

    run(&cache, "Shelf/Calibre");
    let list = find_absorbable(&cache, Duration::from_secs(5)).unwrap();
    let listed: Vec<(&str, &str, u64)> = list.libraries.iter().map(|l| (l.path.as_str(), l.state, l.books)).collect();
    assert_eq!(listed, vec![("Deep/Er/Another", "available", 4), ("Shelf/Calibre", "absorbed", 4)]);

    // A walk with no time left says it is not conclusive.
    let hurried = find_absorbable(&cache, Duration::ZERO).unwrap();
    assert!(!hurried.conclusive);
}

#[test]
fn the_change_log_rebuilds_the_absorbed_books() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");

    let rebuilt_dir = tempfile::tempdir().unwrap();
    for sub in ["changes", "snapshots"] {
        let from = dir.path().join(".lexmechanic").join(sub);
        let to = rebuilt_dir.path().join(".lexmechanic").join(sub);
        std::fs::create_dir_all(&to).unwrap();
        if let Ok(rd) = std::fs::read_dir(from) {
            for e in rd.flatten() {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    std::fs::copy(dir.path().join(".lexmechanic/install-id"), rebuilt_dir.path().join(".lexmechanic/install-id")).ok();
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();
    let mut ids = rebuilt.all_book_ids().unwrap();
    ids.sort();
    assert_eq!(ids.len(), 4);
    for b in BOOKS {
        let rb = rebuilt.book_id_for_uuid(&cache.book_uuid(b.id).unwrap().unwrap()).unwrap().expect("same uuid");
        for field in ["title", "authors", "path", "series", "publisher", "comments", "rating", "formats", "timestamp", "has_cover"] {
            assert_eq!(rebuilt.field_for(rb, field).unwrap(), cache.field_for(b.id, field).unwrap(), "{field} of {}", b.id);
        }
        for label in ["genre", "saga", "saga_index", "mood", "stars", "pages", "notes", "finished", "read", "score"] {
            assert_eq!(custom(&rebuilt, rb, label), custom(&cache, b.id, label), "#{label} of {}", b.id);
        }
    }
}

#[test]
fn annotations_and_reading_positions_come_along() {
    let fixture = build_calibre_library();
    {
        let conn = rusqlite::Connection::open(fixture.root().join("metadata.db")).unwrap();
        conn.execute(
            "INSERT INTO last_read_positions(book, format, user, device, cfi, epoch, pos_frac) VALUES (1, 'EPUB', 'viewer', 'phone', 'epubcfi(/6/4)', 1.5, 0.25)",
            [],
        )
        .unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    copy_dir(fixture.root(), &dir.path().join("Shelf/Calibre"));
    let report = run(&cache, "Shelf/Calibre");
    assert_eq!(report.absorbed, 4);
    let positions = cache.get_last_read_positions(1, "EPUB", "viewer").unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["cfi"], "epubcfi(/6/4)");
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}

#[test]
fn a_library_absorbed_before_the_rename_can_still_be_undone() {
    let (dir, cache, _fixture) = setup("Shelf/Calibre");
    run(&cache, "Shelf/Calibre");
    let nested = dir.path().join("Shelf/Calibre");
    // What an older build left behind: the database parked under its old name.
    std::fs::rename(nested.join(ABSORBED_DB_NAME), nested.join(LEGACY_ABSORBED_DB_NAME)).unwrap();

    let report = unabsorb_library(&cache, "Shelf/Calibre", &JobControl::default()).unwrap();

    assert!(report.database_restored);
    assert!(nested.join("metadata.db").exists());
    assert!(!nested.join(LEGACY_ABSORBED_DB_NAME).exists());
}
