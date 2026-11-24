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
