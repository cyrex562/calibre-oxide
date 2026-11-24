//! Delete and merge on a network share (#258's tier, forced here by
//! `LEXMECHANIC_STORAGE_TIER` so no real share is needed).
//!
//! A share has no recycle bin, which used to leave every deleted file in
//! place. These prove the file leaves its folder anyway -- into the
//! library's own `.caltrash` -- and that nothing reaches the OS bin. Its own
//! test binary because the tier override is process-wide.

use lexmechanic_db::cache::{BookFiles, Cache};
use lexmechanic_db::library_handle::StorageTier;
use lexmechanic_db::merge::{merge_books, MergeOptions};
use lexmechanic_ebooks::metadata::MetaInformation;
use std::fs;
use std::path::Path;

fn network_library() -> (tempfile::TempDir, Cache) {
    std::env::set_var("LEXMECHANIC_STORAGE_TIER", "network");
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    assert_eq!(cache.backend.write_handle().unwrap().tier(), StorageTier::Network, "the override did not take");
    (dir, cache)
}

fn register(cache: &Cache, dir: &Path, rel: &str, bytes: &[u8], title: &str) -> i32 {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    let mut meta = MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec!["Someone".to_string()];
    cache.register_book_in_place(rel, &meta).unwrap()
}

#[test]
fn delete_on_the_network_tier_uses_the_library_trash_and_never_the_os_bin() {
    let (dir, cache) = network_library();
    let id = register(&cache, dir.path(), "Shelf/one.pdf", b"%PDF one", "One");

    let outcome = lexmechanic_db::removal::delete_with_file(&cache, id).unwrap();

    assert_eq!(outcome.deleted, vec!["Shelf/one.pdf".to_string()]);
    assert!(outcome.failed.is_empty(), "{outcome:?}");
    assert!(!dir.path().join("Shelf/one.pdf").exists(), "the file stayed in its folder");
    let entry = cache.list_trash().unwrap().remove(0);
    assert!(dir.path().join(".caltrash").join("b").join(&entry.key).join("one.pdf").is_file());

    // And back again.
    let restored = cache.restore_trashed_book(&entry.key).unwrap();
    assert_eq!(fs::read(dir.path().join("Shelf/one.pdf")).unwrap(), b"%PDF one");
    assert_eq!(cache.book_for_file("Shelf/one.pdf").unwrap(), Some(restored.book_id));
}

#[test]
fn merge_on_the_network_tier_trashes_its_sources_into_caltrash() {
    let (dir, cache) = network_library();
    let dest = register(&cache, dir.path(), "A/keep.pdf", b"%PDF keep", "Keep");
    let source = register(&cache, dir.path(), "B/other.epub", b"epub other", "Keep");

    let report = merge_books(&cache, dest, &[source], &MergeOptions::default()).unwrap();

    let source_report = &report.sources[0];
    assert!(source_report.deleted, "{source_report:?}");
    assert!(source_report.failed.is_empty(), "{source_report:?}");
    // The format moved to the destination; the source's book sits in the trash.
    assert!(cache.field_for(source, "title").unwrap().is_none());
    let in_trash = cache.list_trash().unwrap();
    assert_eq!(in_trash.len(), 1, "{in_trash:?}");
    assert_eq!(in_trash[0].book_id, source);
    assert!(dir.path().join(".caltrash").join("b").join(&in_trash[0].key).is_dir());
    // The destination has both formats.
    let mut formats: Vec<String> = cache.format_file_names(dest).unwrap().into_iter().map(|(f, _)| f).collect();
    formats.sort();
    assert_eq!(formats, vec!["EPUB".to_string(), "PDF".to_string()]);
}

#[test]
fn delete_with_keep_on_the_network_tier_still_leaves_the_file_and_ignores_it() {
    let (dir, cache) = network_library();
    let id = register(&cache, dir.path(), "x.pdf", b"%PDF x", "X");
    let outcome = cache.delete_book_with(id, BookFiles::Keep).unwrap();
    assert_eq!(outcome.ignored, vec!["x.pdf".to_string()]);
    assert!(dir.path().join("x.pdf").exists());
    assert!(cache.list_trash().unwrap().is_empty());
}
