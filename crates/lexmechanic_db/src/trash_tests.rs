use super::*;
use crate::cache::BookFiles;
use lexmechanic_ebooks::metadata::MetaInformation;

fn library() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn register(cache: &Cache, rel: &str, title: &str, author: &str) -> i32 {
    let mut meta = MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec![author.to_string()];
    cache.register_book_in_place(rel, &meta).unwrap()
}

/// The bug the whole module exists for: the wastebasket is not there, and
/// delete must still take the file out of the folder -- into `.caltrash`,
/// listed, and restorable to the same place with the same metadata and the
/// same uuid.
#[test]
fn a_book_in_calibres_layout_round_trips_through_the_trash() {
    let (dir, cache) = library();
    let folder = dir.path().join("Jane Doe").join("A Title (1)");
    write(&folder.join("A Title - Jane Doe.epub"), b"epub bytes");
    write(&folder.join("A Title - Jane Doe.pdf"), b"pdf bytes");
    write(&folder.join("cover.jpg"), b"jpeg");
    write(&folder.join("metadata.opf"), b"<opf/>");
    let id = register(&cache, "Jane Doe/A Title (1)/A Title - Jane Doe.epub", "A Title", "Jane Doe");
    cache.record_format_row(id, "PDF", "A Title - Jane Doe", 9, None).unwrap();
    cache.set_field(id, "tags", "one, two").unwrap();
    cache.set_field(id, "publisher", "Acme").unwrap();
    cache.set_field(id, "comments", "<p>hello</p>").unwrap();
    cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();
    cache.set_custom_column_value(id, "shelf", "top").unwrap();
    let uuid = cache.book_uuid(id).unwrap().unwrap();

    let outcome = cache.delete_book_with(id, BookFiles::Trash).unwrap();

    assert!(outcome.failed.is_empty(), "{outcome:?}");
    assert!(!folder.join("A Title - Jane Doe.epub").exists());
    assert!(!folder.join("A Title - Jane Doe.pdf").exists());
    assert!(cache.field_for(id, "title").unwrap().is_none(), "the row is gone");
    let listed = cache.list_trash().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, TrashKind::Book);
    assert_eq!(listed[0].title, "A Title");
    assert_eq!(listed[0].authors, "Jane Doe");
    assert_eq!(listed[0].formats.len(), 2);
    assert!(listed[0].size > 0);
    assert_eq!(listed[0].original_path, "Jane Doe/A Title (1)");
    assert!(cache.trash_dir().join("b").join(&listed[0].key).join("A Title - Jane Doe.epub").is_file());
    // Not an ignored file: it is not in its folder to be rescanned.
    assert!(cache.ignored().list().unwrap().is_empty());

    let restored = cache.restore_trashed_book(&listed[0].key).unwrap();

    assert_eq!(restored.uuid, uuid);
    assert_eq!(restored.book_id, id, "the id was free, so it is kept");
    assert_eq!(fs::read(folder.join("A Title - Jane Doe.epub")).unwrap(), b"epub bytes");
    assert_eq!(fs::read(folder.join("A Title - Jane Doe.pdf")).unwrap(), b"pdf bytes");
    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("A Title"));
    assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some("Jane Doe"));
    assert_eq!(cache.field_for(id, "tags").unwrap().as_deref(), Some("one, two"));
    assert_eq!(cache.field_for(id, "publisher").unwrap().as_deref(), Some("Acme"));
    assert_eq!(cache.field_for(id, "comments").unwrap().as_deref(), Some("<p>hello</p>"));
    assert_eq!(cache.get_custom_column_value(id, "shelf").unwrap().as_deref(), Some("top"));
    assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some("Jane Doe/A Title (1)"));
    let mut formats: Vec<String> = cache.format_file_names(id).unwrap().into_iter().map(|(f, _)| f).collect();
    formats.sort();
    assert_eq!(formats, vec!["EPUB".to_string(), "PDF".to_string()]);
    assert!(cache.list_trash().unwrap().is_empty(), "the entry is consumed");
}

/// An in-place book shares its folder with siblings and unrelated files:
/// trashing and restoring it must put back exactly its own file, at its
/// original relative path, and never touch the rest.
#[test]
fn an_in_place_book_restores_to_its_original_path_and_leaves_siblings_alone() {
    let (dir, cache) = library();
    let folder = dir.path().join("Receipts");
    write(&folder.join("a.pdf"), b"%PDF a");
    write(&folder.join("b.pdf"), b"%PDF b");
    write(&folder.join("notes.txt"), b"mine");
    write(&folder.join("cover.jpg"), b"someone else's picture");
    let a = register(&cache, "Receipts/a.pdf", "A", "Unknown");
    let b = register(&cache, "Receipts/b.pdf", "B", "Unknown");

    cache.delete_book_with(a, BookFiles::Trash).unwrap();

    assert!(!folder.join("a.pdf").exists());
    for (name, bytes) in [("b.pdf", &b"%PDF b"[..]), ("notes.txt", b"mine"), ("cover.jpg", b"someone else's picture")] {
        assert_eq!(fs::read(folder.join(name)).unwrap(), bytes, "{name} was touched");
    }
    assert!(cache.field_for(b, "title").unwrap().is_some());
    let key = cache.list_trash().unwrap().remove(0).key;

    let restored = cache.restore_trashed_book(&key).unwrap();

    assert_eq!(restored.restored, vec!["Receipts/a.pdf".to_string()]);
    assert_eq!(fs::read(folder.join("a.pdf")).unwrap(), b"%PDF a");
    assert_eq!(cache.field_for(restored.book_id, "path").unwrap().as_deref(), Some("Receipts"));
    assert_eq!(cache.book_for_file("Receipts/a.pdf").unwrap(), Some(restored.book_id));
    assert_eq!(fs::read(folder.join("b.pdf")).unwrap(), b"%PDF b");
    assert_eq!(fs::read(folder.join("notes.txt")).unwrap(), b"mine");
}

/// A restore never replaces what is there now.
#[test]
fn restoring_beside_a_file_that_took_the_name_does_not_overwrite_it() {
    let (dir, cache) = library();
    write(&dir.path().join("d.pdf"), b"original");
    let id = register(&cache, "d.pdf", "D", "Unknown");
    cache.delete_book_with(id, BookFiles::Trash).unwrap();
    write(&dir.path().join("d.pdf"), b"somebody else's");
    let key = cache.list_trash().unwrap().remove(0).key;

    let restored = cache.restore_trashed_book(&key).unwrap();

    assert_eq!(fs::read(dir.path().join("d.pdf")).unwrap(), b"somebody else's");
    assert_eq!(restored.renamed, vec![("d.pdf".to_string(), "d (restored).pdf".to_string())]);
    assert_eq!(fs::read(dir.path().join("d (restored).pdf")).unwrap(), b"original");
    assert_eq!(cache.book_for_file("d (restored).pdf").unwrap(), Some(restored.book_id));
}

/// A removal that kept its file put it on the ignore list; restoring a book
/// must clear that, or it comes back present and invisible.
#[test]
fn restoring_clears_the_ignore_entry_for_the_file() {
    let (dir, cache) = library();
    write(&dir.path().join("e.pdf"), b"%PDF e");
    let id = register(&cache, "e.pdf", "E", "Unknown");
    cache.delete_book_with(id, BookFiles::Trash).unwrap();
    cache.ignored().ignore("e.pdf", None, "E").unwrap();
    let key = cache.list_trash().unwrap().remove(0).key;

    cache.restore_trashed_book(&key).unwrap();

    assert!(cache.ignored().list().unwrap().is_empty());
}

#[test]
fn a_removed_format_goes_to_the_trash_and_comes_back() {
    let (dir, cache) = library();
    write(&dir.path().join("f.epub"), b"epub");
    write(&dir.path().join("f.pdf"), b"pdf");
    let id = register(&cache, "f.epub", "F", "Unknown");
    cache.record_format_row(id, "PDF", "f", 3, None).unwrap();

    cache.remove_format(id, "pdf").unwrap();

    assert!(!dir.path().join("f.pdf").exists());
    assert!(dir.path().join("f.epub").exists());
    let listed = cache.list_trash().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, TrashKind::Format);
    assert_eq!(listed[0].formats, vec!["PDF".to_string()]);
    assert_eq!(listed[0].book_id, id);
    assert!(cache.trash_dir().join("f").join(id.to_string()).join("pdf").is_file(), "calibre's layout: f/<id>/<ext>");

    let back = cache.restore_trashed_format(&listed[0].key, "PDF").unwrap();

    assert_eq!(back.path, "f.pdf");
    assert_eq!(fs::read(dir.path().join("f.pdf")).unwrap(), b"pdf");
    assert!(cache.format_file_names(id).unwrap().iter().any(|(f, _)| f == "PDF"));
    assert!(cache.list_trash().unwrap().is_empty());
}

#[test]
fn restoring_a_format_the_book_already_has_is_refused() {
    let (dir, cache) = library();
    write(&dir.path().join("g.pdf"), b"one");
    let id = register(&cache, "g.pdf", "G", "Unknown");
    cache.remove_format(id, "pdf").unwrap();
    write(&dir.path().join("g2.pdf"), b"two");
    cache.record_format_row(id, "PDF", "g2", 3, None).unwrap();
    let key = cache.list_trash().unwrap().remove(0).key;

    let err = cache.restore_trashed_format(&key, "PDF").unwrap_err();

    assert!(format!("{err:#}").contains("already has"), "{err:#}");
    assert_eq!(cache.list_trash().unwrap().len(), 1, "nothing was lost");
}

#[test]
fn entries_expire_after_the_configured_age_and_not_before() {
    let (dir, cache) = library();
    write(&dir.path().join("h.pdf"), b"h");
    write(&dir.path().join("i.pdf"), b"i");
    let h = register(&cache, "h.pdf", "H", "Unknown");
    let i = register(&cache, "i.pdf", "I", "Unknown");
    cache.delete_book_with(h, BookFiles::Trash).unwrap();
    cache.remove_format(i, "pdf").unwrap();
    let now = now_secs();

    assert_eq!(cache.trash_expiry_seconds(), DEFAULT_EXPIRY_SECONDS, "calibre's 14 days");
    let early = cache.expire_old_trash_at(now + 13 * 86400, DEFAULT_EXPIRY_SECONDS).unwrap();
    assert!(early.removed.is_empty());
    assert_eq!(cache.list_trash().unwrap().len(), 2);

    let late = cache.expire_old_trash_at(now + 15 * 86400, DEFAULT_EXPIRY_SECONDS).unwrap();
    assert_eq!(late.removed.len(), 2, "{late:?}");
    assert!(cache.list_trash().unwrap().is_empty());
    assert!(fs::read_dir(cache.trash_dir().join("b")).unwrap().next().is_none());
    assert!(fs::read_dir(cache.trash_dir().join("f")).unwrap().next().is_none());
}

#[test]
fn the_expiry_is_a_library_preference_and_never_under_a_day() {
    let (_dir, cache) = library();
    cache.set_preference(EXPIRE_PREF, "3600").unwrap();
    assert_eq!(cache.trash_expiry_seconds(), MIN_EXPIRY_SECONDS);
    cache.set_preference(EXPIRE_PREF, &(3 * 86400).to_string()).unwrap();
    assert_eq!(cache.trash_expiry_seconds(), 3 * 86400);
}

#[test]
fn emptying_the_trash_removes_everything_and_deleting_one_entry_only_that() {
    let (dir, cache) = library();
    for name in ["j", "k", "l"] {
        write(&dir.path().join(format!("{name}.pdf")), name.as_bytes());
    }
    let ids: Vec<i32> = ["j", "k", "l"].iter().map(|n| register(&cache, &format!("{n}.pdf"), n, "Unknown")).collect();
    for id in &ids {
        cache.delete_book_with(*id, BookFiles::Trash).unwrap();
    }
    let listed = cache.list_trash().unwrap();
    assert_eq!(listed.len(), 3);

    cache.delete_trash_entry(listed[0].kind, &listed[0].key, None).unwrap();
    assert_eq!(cache.list_trash().unwrap().len(), 2);

    assert_eq!(cache.empty_trash().unwrap(), 2);
    assert!(cache.list_trash().unwrap().is_empty());
}

/// The scanner must not walk the trash: a deleted book it finds there would
/// be added straight back.
#[test]
fn a_scan_does_not_see_trashed_books() {
    let (dir, cache) = library();
    write(&dir.path().join("m.pdf"), b"%PDF m");
    let id = register(&cache, "m.pdf", "M", "Unknown");
    cache.delete_book_with(id, BookFiles::Trash).unwrap();
    assert!(cache.trash_dir().join("b").join(id.to_string()).join("m.pdf").is_file());

    let report = crate::scan::walk(dir.path(), &crate::scan::ScanOptions::default(), SystemTime::now() + std::time::Duration::from_secs(3600)).unwrap();

    assert!(report.files.is_empty(), "{:?}", report.files);
}

/// A rebuild from the log alone must land on the right state after trash,
/// restore, and a restored book's later edits.
#[test]
fn a_rebuild_from_the_log_agrees_after_trash_and_restore() {
    let (dir, cache) = library();
    write(&dir.path().join("n.pdf"), b"n");
    write(&dir.path().join("o.pdf"), b"o");
    let n = register(&cache, "n.pdf", "N", "Unknown");
    let o = register(&cache, "o.pdf", "O", "Unknown");
    let o_uuid = cache.book_uuid(o).unwrap().unwrap();
    cache.set_field(n, "tags", "x").unwrap();
    cache.delete_book_with(n, BookFiles::Trash).unwrap();
    cache.delete_book_with(o, BookFiles::Trash).unwrap();
    let key = cache.list_trash().unwrap().into_iter().find(|e| e.book_id == o).unwrap().key;
    let restored = cache.restore_trashed_book(&key).unwrap();
    cache.set_field(restored.book_id, "tags", "after-restore").unwrap();

    let rebuilt_dir = tempfile::tempdir().unwrap();
    let state = crate::constants::LIBRARY_HANDLE_DIR_NAME;
    for sub in ["changes", "snapshots"] {
        let to = rebuilt_dir.path().join(state).join(sub);
        fs::create_dir_all(&to).unwrap();
        if let Ok(read) = fs::read_dir(dir.path().join(state).join(sub)) {
            for e in read.flatten() {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    fs::copy(dir.path().join(state).join("install-id"), rebuilt_dir.path().join(state).join("install-id")).ok();
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();

    let ids = rebuilt.all_book_ids().unwrap();
    assert_eq!(ids.len(), 1, "only the restored book exists: {ids:?}");
    assert_eq!(rebuilt.book_uuid(ids[0]).unwrap().as_deref(), Some(o_uuid.as_str()));
    assert_eq!(rebuilt.field_for(ids[0], "tags").unwrap().as_deref(), Some("after-restore"));
    assert_eq!(rebuilt.field_for(ids[0], "path").unwrap().as_deref(), Some(""));
    // The file is back at its path, so the rebuilt library must not hold an
    // ignore entry for it from replaying the removal.
    assert!(rebuilt.ignored().list().unwrap().is_empty());
}

#[test]
fn a_trash_entry_cannot_name_a_path_outside_the_library() {
    let (dir, cache) = library();
    write(&dir.path().join("p.pdf"), b"p");
    let id = register(&cache, "p.pdf", "P", "Unknown");
    cache.delete_book_with(id, BookFiles::Trash).unwrap();
    let key = cache.list_trash().unwrap().remove(0).key;
    let manifest_path = cache.trash_dir().join("b").join(&key).join(BOOK_MANIFEST);
    let evil = fs::read_to_string(&manifest_path).unwrap().replace("\"rel\": \"p.pdf\"", "\"rel\": \"../escaped.pdf\"");
    fs::write(&manifest_path, evil).unwrap();

    assert!(cache.restore_trashed_book(&key).is_err());
    assert!(!dir.path().parent().unwrap().join("escaped.pdf").exists());
}

#[test]
fn entry_keys_cannot_climb_out_of_the_trash() {
    let (_dir, cache) = library();
    assert!(cache.restore_trashed_book("../..").is_err());
    assert!(cache.delete_trash_entry(TrashKind::Book, "..", None).is_err());
}

// ---------------------------------------------------------------------
// Batch deletion (tranche 29c): one transaction and one log entry per run
// ---------------------------------------------------------------------

/// Every file under `root` (relative, sorted) with its size, leaving out the library's own state.
fn tree(root: &Path) -> Vec<(String, u64)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if rel.starts_with(".lexmechanic") || rel == "metadata.db" || rel.starts_with("metadata.db-") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
                out.push((format!("{rel}/"), 0));
            } else if !rel.ends_with(BOOK_MANIFEST) {
                out.push((rel, path.metadata().unwrap().len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// A library with books in a shared folder, a folder of their own and the root.
fn mixed_library() -> (tempfile::TempDir, Cache, Vec<i32>) {
    let (dir, cache) = library();
    let mut ids = Vec::new();
    for (rel, title) in [("Receipts/a.pdf", "A"), ("Receipts/b.pdf", "B"), ("Receipts/c.pdf", "C"), ("Own/Dune (7)/d.pdf", "D"), ("root.pdf", "R"), ("Reports/e.pdf", "E"), ("Reports/f.pdf", "F")] {
        write(&dir.path().join(rel), format!("%PDF {title}").as_bytes());
        ids.push(register(&cache, rel, title, "Unknown"));
    }
    (dir, cache, ids)
}

/// The batch is the one-by-one deletion, done in one transaction: the same books gone,
/// the same files in the same places, the same folders removed.
#[test]
fn deleting_many_books_at_once_leaves_what_deleting_them_one_by_one_does() {
    let (one_dir, one, one_ids) = mixed_library();
    let (many_dir, many, many_ids) = mixed_library();
    // Both of a shared folder's books, a book of its own, and one more.
    let pick = |ids: &[i32]| vec![ids[0], ids[1], ids[2], ids[3], ids[5], ids[6]];

    for id in pick(&one_ids) {
        one.delete_book_with(id, BookFiles::Trash).unwrap();
    }
    let results = many.delete_books_with(&pick(&many_ids), BookFiles::Trash);
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");

    assert_eq!(tree(one_dir.path()), tree(many_dir.path()));
    assert_eq!(one.all_book_ids().unwrap(), many.all_book_ids().unwrap());
    let titles = |c: &Cache| {
        let mut v: Vec<(String, i64)> = c.list_trash().unwrap().into_iter().map(|e| (e.title, e.size)).collect();
        v.sort();
        v
    };
    assert_eq!(titles(&one), titles(&many));
    // The folders the books were alone in went with the last of them.
    assert!(!many_dir.path().join("Reports").exists());
    assert!(!many_dir.path().join("Own").exists());
    assert!(many_dir.path().join("root.pdf").exists());
}

/// What a batch trashed comes back, with the same uuids.
#[test]
fn what_a_batch_trashed_can_be_restored() {
    let (dir, cache, ids) = mixed_library();
    let mut expected: Vec<Option<String>> = ids.iter().map(|id| cache.book_uuid(*id).unwrap()).collect();
    let results = cache.delete_books_with(&ids[..3], BookFiles::Trash);
    assert!(results.iter().all(|r| r.is_ok()));
    assert!(!dir.path().join("Receipts/a.pdf").exists());

    for entry in cache.list_trash().unwrap() {
        cache.restore_trashed_book(&entry.key).unwrap();
    }
    for rel in ["Receipts/a.pdf", "Receipts/b.pdf", "Receipts/c.pdf"] {
        assert!(dir.path().join(rel).exists(), "{rel}");
    }
    let mut restored: Vec<Option<String>> = cache.all_book_ids().unwrap().into_iter().map(|id| cache.book_uuid(id).unwrap()).collect();
    restored.sort();
    expected.sort();
    assert_eq!(restored, expected);
}

/// One change-log entry for the batch, however many books: that is the saving (an
/// entry is its own file and two fsyncs).
#[test]
fn a_batch_deletion_writes_one_change_log_entry() {
    let (_dir, cache, ids) = mixed_library();
    let before = cache.backend.change_log().unwrap().next_sequence();
    cache.delete_books_with(&ids[..4], BookFiles::Trash);
    let after = cache.backend.change_log().unwrap().next_sequence();
    assert_eq!(after - before, 1, "one batch entry for four books");
}

/// A book that is not there is a no-op for itself and harms nothing else.
#[test]
fn a_batch_with_an_unknown_id_and_a_repeat_deletes_the_rest() {
    let (dir, cache, ids) = mixed_library();
    let results = cache.delete_books_with(&[ids[0], 9999, ids[0], ids[1]], BookFiles::Trash);
    assert_eq!(results.len(), 4);
    assert!(results.iter().all(|r| r.is_ok()));
    assert!(!dir.path().join("Receipts/a.pdf").exists());
    assert!(!dir.path().join("Receipts/b.pdf").exists());
    assert!(dir.path().join("Receipts/c.pdf").exists());
}

/// A run's restore reports progress, and stops between books when cancelled.
#[test]
fn restoring_a_run_counts_its_books_and_stops_when_cancelled() {
    let (_dir, cache, ids) = mixed_library();
    crate::trash::with_trash_run("run-test", || cache.delete_books_with(&ids[..3], BookFiles::Trash));
    assert_eq!(cache.trash_entries_for_run("run-test").unwrap().len(), 3);

    let control = crate::scan_pipeline::JobControl::new();
    control.cancel();
    let none = cache.restore_trash_run_controlled("run-test", &control).unwrap();
    assert!(none.restored.is_empty(), "a cancelled restore starts nothing");
    assert_eq!(control.snapshot().total, 3);

    let control = crate::scan_pipeline::JobControl::new();
    let all = cache.restore_trash_run_controlled("run-test", &control).unwrap();
    assert_eq!(all.restored.len(), 3, "{:?}", all.failed);
    assert_eq!(control.snapshot().seen, 3);
}

/// Windows strips a trailing dot when it creates a folder, so the folder a book
/// was recorded in (`Joseph P. J.`) is on disk as `Joseph P. J`. A path spelled with
/// the dot and made extended-length (as a canonical library path is) names nothing.
/// The trash must move the file all the same.
#[cfg(windows)]
#[test]
fn a_folder_recorded_with_a_trailing_dot_is_trashed_by_the_name_it_has_on_disk() {
    let (dir, cache) = library();
    write(&dir.path().join("Joseph P. J.").join("a.pdf"), b"%PDF dot");
    assert!(dir.path().join("Joseph P. J").join("a.pdf").is_file(), "Windows made the folder without its dot");
    let id = register(&cache, "Joseph P. J./a.pdf", "Dot", "Unknown");

    let outcome = cache.delete_book_with(id, BookFiles::Trash).unwrap();
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(outcome.trashed, vec!["Joseph P. J./a.pdf".to_string()]);
    assert!(!dir.path().join("Joseph P. J").join("a.pdf").exists());
    let key = cache.list_trash().unwrap().remove(0).key;
    cache.restore_trashed_book(&key).unwrap();
    assert!(dir.path().join("Joseph P. J").join("a.pdf").is_file());
}

/// The raw Win32 rename the journaled handle makes has no long-path support of its own.
#[cfg(windows)]
#[test]
fn a_book_in_a_very_deep_folder_is_trashed_and_restored() {
    let (dir, cache) = library();
    let mut rel = String::new();
    for i in 0..9 {
        rel.push_str(&format!("a-fairly-long-folder-name-number-{i:02}-xxxxxxxxxxxx/"));
    }
    rel.push_str("deep.pdf");
    assert!(dir.path().join(&rel).to_string_lossy().len() > 300);
    write(&dir.path().join(&rel), b"%PDF deep");
    let id = register(&cache, &rel, "Deep", "Unknown");

    let outcome = cache.delete_book_with(id, BookFiles::Trash).unwrap();
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(!dir.path().join(&rel).exists());
    let key = cache.list_trash().unwrap().remove(0).key;
    cache.restore_trashed_book(&key).unwrap();
    assert!(dir.path().join(&rel).is_file());
}

/// The rename itself, on the spelling the library path takes when it is canonical
/// (`\\?\`): there the dot is taken literally.
#[cfg(windows)]
#[test]
fn an_extended_length_path_with_a_trailing_dot_is_found_by_the_stripped_name() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("Joseph P. J.").join("a.pdf"), b"x");
    let root = dir.path().canonicalize().unwrap();
    assert!(root.to_string_lossy().starts_with(r"\\?\"));
    let from = root.join("Joseph P. J.").join("a.pdf");
    assert!(!from.exists(), "an extended-length path is not normalised");
    crate::library_handle::rename_no_replace(&from, &root.join("moved.pdf")).unwrap();
    assert!(root.join("moved.pdf").is_file());
}

/// A batch deletion and a restore (with its one-transaction field write) must leave a log that
/// rebuilds the same library: the books deleted together stay deleted, the one restored is back
/// with the fields it had.
#[test]
fn a_rebuild_from_the_log_agrees_after_a_batch_deletion_and_a_restore() {
    let (dir, cache, ids) = mixed_library();
    let uuid_of = |id: i32| cache.book_uuid(id).unwrap().unwrap();
    let restored_uuid = uuid_of(ids[1]);
    cache.set_field(ids[1], "publisher", "Bench House").unwrap();
    cache.set_field(ids[1], "tags", "kept, and, restored").unwrap();
    cache.set_field(ids[1], "rating", "6").unwrap();
    cache.delete_books_with(&[ids[0], ids[1], ids[2]], BookFiles::Trash);
    let key = cache.list_trash().unwrap().into_iter().find(|e| e.book_id == ids[1]).unwrap().key;
    let back = cache.restore_trashed_book(&key).unwrap();

    let rebuilt_dir = tempfile::tempdir().unwrap();
    let state = crate::constants::LIBRARY_HANDLE_DIR_NAME;
    for sub in ["changes", "snapshots"] {
        let to = rebuilt_dir.path().join(state).join(sub);
        fs::create_dir_all(&to).unwrap();
        if let Ok(read) = fs::read_dir(dir.path().join(state).join(sub)) {
            for e in read.flatten() {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    fs::copy(dir.path().join(state).join("install-id"), rebuilt_dir.path().join(state).join("install-id")).ok();
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();

    // Seven books, three deleted together, one of them brought back.
    let mut live: Vec<String> = cache.all_book_ids().unwrap().into_iter().map(|id| uuid_of(id)).collect();
    let mut rebuilt_live: Vec<String> = rebuilt.all_book_ids().unwrap().into_iter().map(|id| rebuilt.book_uuid(id).unwrap().unwrap()).collect();
    live.sort();
    rebuilt_live.sort();
    assert_eq!(live.len(), 5);
    assert_eq!(rebuilt_live, live);
    assert!(rebuilt_live.contains(&restored_uuid));
    let id = rebuilt.book_id_for_uuid(&restored_uuid).unwrap().unwrap();
    assert_eq!(rebuilt.field_for(id, "publisher").unwrap().as_deref(), Some("Bench House"));
    assert_eq!(rebuilt.field_for(id, "tags").unwrap().as_deref(), Some("kept, and, restored"));
    assert_eq!(rebuilt.field_for(id, "rating").unwrap().as_deref(), Some("6"));
    assert_eq!(cache.field_for(back.book_id, "rating").unwrap().as_deref(), Some("6"));
}
