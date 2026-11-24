//! The cross-library copy as a job body (tranche 14a): one duplicate index per
//! run, a streamed verify, progress, cancel, and books filed under
//! `Author/Title (id)`.

use lexmechanic_db::cache::Cache;
use lexmechanic_db::copy_to_library::{copy_books, verify_copy, CopyOptions};
use lexmechanic_db::dedupe::exact_index_builds_on_this_thread;
use lexmechanic_db::scan_pipeline::JobControl;
use std::sync::atomic::Ordering;

fn add(cache: &Cache, dir: &std::path::Path, file: &str, bytes: &[u8], title: &str, author: &str) -> i32 {
    let source = dir.join(file);
    std::fs::write(&source, bytes).unwrap();
    let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
    meta.title = title.to_string();
    meta.authors = vec![author.to_string()];
    cache.add_book(&source, &meta).unwrap()
}

fn libraries(count: usize) -> (tempfile::TempDir, tempfile::TempDir, Cache, Cache, Vec<i32>) {
    let (src_dir, dest_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = Cache::new(src_dir.path()).unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    let ids = (0..count).map(|i| add(&src, src_dir.path(), &format!("book {i}.txt"), format!("contents of book {i}").as_bytes(), &format!("Title {i}"), &format!("Author {i}"))).collect();
    (src_dir, dest_dir, src, dest, ids)
}

#[test]
fn a_bulk_copy_builds_the_duplicate_index_once() {
    let (_s, _d, src, dest, ids) = libraries(12);
    let before = exact_index_builds_on_this_thread();
    let control = JobControl::default();
    let report = copy_books(&src, &dest, &ids, CopyOptions { check_duplicates: true, move_books: false }, &control).unwrap();
    assert_eq!(report.copied, 12, "{report:?}");
    assert_eq!(exact_index_builds_on_this_thread() - before, 1, "one build for the whole run, not one per book");
    assert_eq!(dest.all_book_ids().unwrap().len(), 12);

    // Without duplicate checking there is no index at all.
    let before = exact_index_builds_on_this_thread();
    let (_s2, _d2, src2, dest2, ids2) = libraries(3);
    copy_books(&src2, &dest2, &ids2, CopyOptions::default(), &JobControl::default()).unwrap();
    assert_eq!(exact_index_builds_on_this_thread() - before, 0);
}

#[test]
fn the_index_is_kept_current_so_a_repeat_within_one_run_is_a_duplicate() {
    let (_s, _d, src, dest, mut ids) = libraries(2);
    // The same book twice in the request (two source books, same title and author).
    let src_dir = _s.path();
    let twin = add(&src, src_dir, "twin.txt", b"another file", "Title 0", "Author 0");
    ids.push(twin);
    let report = copy_books(&src, &dest, &ids, CopyOptions { check_duplicates: true, move_books: false }, &JobControl::default()).unwrap();
    assert_eq!((report.copied, report.duplicates), (2, 1), "{report:?}");
    assert!(report.entries.last().unwrap().duplicate);
    assert_eq!(dest.all_book_ids().unwrap().len(), 2);

    // And running it again copies nothing.
    let again = copy_books(&src, &dest, &ids, CopyOptions { check_duplicates: true, move_books: false }, &JobControl::default()).unwrap();
    assert_eq!((again.copied, again.duplicates), (0, 3));
}

#[test]
fn progress_counts_what_was_copied_and_skipped() {
    let (_s, _d, src, dest, ids) = libraries(5);
    copy_books(&src, &dest, &ids[..2], CopyOptions::default(), &JobControl::default()).unwrap();
    let control = JobControl::default();
    copy_books(&src, &dest, &ids, CopyOptions { check_duplicates: true, move_books: false }, &control).unwrap();
    let progress = control.snapshot();
    assert_eq!(progress.total, 5);
    assert_eq!(progress.added, 3);
    assert_eq!(progress.already_known, 2);
    assert_eq!(progress.errors, 0);
    assert_eq!(progress.phase, "copying");
}

#[test]
fn a_cancelled_copy_stops_between_books_and_keeps_what_was_copied() {
    let (_s, _d, src, dest, ids) = libraries(4);
    let control = JobControl::default();
    control.cancel();
    let report = copy_books(&src, &dest, &ids, CopyOptions::default(), &control).unwrap();
    assert!(report.cancelled);
    assert_eq!(report.entries.len(), 0);
    assert!(dest.all_book_ids().unwrap().is_empty());
}

#[test]
fn a_bad_book_is_reported_and_the_rest_go_on() {
    let (_s, _d, src, dest, ids) = libraries(3);
    // Book 2's file is gone: nothing of it is copied, the others are.
    let gone = src.format_file_info(ids[1]).unwrap().remove(0).path;
    std::fs::remove_file(gone).unwrap();
    let control = JobControl::default();
    let report = copy_books(&src, &dest, &ids, CopyOptions::default(), &control).unwrap();
    assert_eq!((report.copied, report.failed), (2, 1), "{report:?}");
    assert!(report.entries[1].error.as_deref().unwrap().contains("missing"));
    assert_eq!(control.errors.load(Ordering::Relaxed), 1);
    assert_eq!(dest.all_book_ids().unwrap().len(), 2, "no half-copied book is left behind");
}

#[test]
fn copied_books_are_filed_under_author_and_title() {
    let (_s, dest_dir, src, dest, ids) = libraries(2);
    let report = copy_books(&src, &dest, &ids, CopyOptions::default(), &JobControl::default()).unwrap();
    for entry in &report.entries {
        let id = entry.copied_as.unwrap();
        let path = dest.field_for(id, "path").unwrap().unwrap();
        let title = dest.field_for(id, "title").unwrap().unwrap();
        let author = dest.field_for(id, "authors").unwrap().unwrap();
        assert_eq!(path, format!("{author}/{title} ({id})"));
        let file = dest.format_file_info(id).unwrap().remove(0).path;
        assert!(file.is_file());
        assert_eq!(file.parent().unwrap(), dest_dir.path().join(&path));
    }
    // Nothing but the library's own files in the root.
    let strays: Vec<String> = std::fs::read_dir(dest_dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".txt")).collect();
    assert!(strays.is_empty(), "{strays:?}");
}

#[test]
fn verify_copy_streams_and_catches_a_difference_past_the_first_chunk() {
    let (src_dir, dest_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = Cache::new(src_dir.path()).unwrap();
    let dest = Cache::new(dest_dir.path()).unwrap();
    // Larger than two read chunks, so a streamed hash crosses boundaries.
    let big: Vec<u8> = (0..(2 * lexmechanic_db::scan_pipeline::HASH_BUFFER_BYTES + 12345)).map(|i| (i % 251) as u8).collect();
    let id = add(&src, src_dir.path(), "big.bin", &big, "Big", "Author");
    let copied = copy_books(&src, &dest, &[id], CopyOptions::default(), &JobControl::default()).unwrap().entries[0].copied_as.unwrap();
    verify_copy(&src, &dest, id, copied).unwrap();

    // One byte flipped near the end, same size.
    let dest_file = dest.format_file_info(copied).unwrap().remove(0).path;
    let mut bytes = std::fs::read(&dest_file).unwrap();
    let last = bytes.len() - 5;
    bytes[last] ^= 0xFF;
    std::fs::write(&dest_file, bytes).unwrap();
    let err = verify_copy(&src, &dest, id, copied).unwrap_err().to_string();
    assert!(err.contains("differs"), "{err}");

    // A truncated file is caught by size.
    std::fs::write(&dest_file, &big[..100]).unwrap();
    assert!(verify_copy(&src, &dest, id, copied).unwrap_err().to_string().contains("bytes"));
}

#[test]
fn a_move_removes_the_source_only_after_a_verified_copy() {
    let (_s, _d, src, dest, ids) = libraries(2);
    let report = copy_books(&src, &dest, &ids, CopyOptions { check_duplicates: false, move_books: true }, &JobControl::default()).unwrap();
    assert_eq!((report.copied, report.removed_from_source), (2, 2), "{report:?}");
    assert!(src.all_book_ids().unwrap().is_empty());
    assert_eq!(dest.all_book_ids().unwrap().len(), 2);
}
