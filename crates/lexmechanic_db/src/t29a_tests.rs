//! Tranche 29a regression tests: the engine findings of the PR #1001 review.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::cache::Cache;
use crate::scan::{walk, ScanOptions};

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

fn index(dir: &Path, cache: &Cache) {
    let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
    crate::scan::index_scan(cache, dir, &report);
}

fn id_of(cache: &Cache, title: &str) -> i32 {
    cache.all_book_ids().unwrap().into_iter().find(|id| cache.field_for(*id, "title").unwrap().as_deref() == Some(title)).unwrap_or_else(|| panic!("no book titled {title}"))
}

// --- #8: another book nested deeper under `<A>/data/...` -------------------

#[test]
fn a_book_nested_deep_under_data_keeps_its_attachments_out_of_the_book_above() {
    let (dir, cache) = library();
    write(&dir.path().join("Reports/report.pdf"), b"%PDF report");
    write(&dir.path().join("Reports/data/sub/deep.pdf"), b"%PDF deep");
    index(dir.path(), &cache);
    let (a, b) = (id_of(&cache, "report"), id_of(&cache, "deep"));
    assert_eq!(cache.field_for(b, "path").unwrap().as_deref(), Some("Reports/data/sub"));
    // B's own attachment lives in B's `data/`, which is also below A's `data/`.
    write(&dir.path().join("Reports/data/sub/data/x.txt"), b"b's notes");
    // And A has one of its own.
    write(&dir.path().join("Reports/data/note.txt"), b"a's notes");

    let listed: Vec<String> = crate::extra_files::list_extra_files(&cache, a, crate::constants::DATA_FILE_PATTERN).unwrap().into_iter().map(|f| f.relpath).collect();
    assert_eq!(listed, vec!["data/note.txt".to_string()], "only A's own attachment");
    let b_listed: Vec<String> = crate::extra_files::list_extra_files(&cache, b, crate::constants::DATA_FILE_PATTERN).unwrap().into_iter().map(|f| f.relpath).collect();
    assert_eq!(b_listed, vec!["data/x.txt".to_string()]);

    let errors = crate::extra_files::remove_extra_files(&cache, a, &["data/sub/data/x.txt".to_string()], true).unwrap();
    assert!(errors.contains_key("data/sub/data/x.txt"), "{errors:?}");
    assert!(dir.path().join("Reports/data/sub/data/x.txt").is_file(), "B's attachment was removed through A");
}
