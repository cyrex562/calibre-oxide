//! The hash backfill over a library real calibre made, and a modest
//! performance smoke test for the folder scan.
//!
//! The unit tests in `scan_pipeline.rs` cover the single read, batching,
//! cancel/resume and thread-count determinism against Lexmechanic's own
//! libraries. What they cannot show is the case the backfill exists for:
//! a library built by something else, which has no checksum sidecar at
//! all.

mod common;

use lexmechanic_db::cache::Cache;
use lexmechanic_db::copy_to_library::scan_library_for_content_duplicates;
use lexmechanic_db::scan::rescan;
use lexmechanic_db::scan_pipeline::{backfill_hashes, JobControl, PipelineOptions};
use common::*;

fn open() -> (CalibreFixture, Cache) {
    let fixture = build_calibre_library();
    let cache = Cache::new(fixture.root()).expect("Lexmechanic could not open a calibre-made library");
    (fixture, cache)
}

fn options(workers: usize, batch_size: usize) -> PipelineOptions {
    PipelineOptions { workers, batch_size, after_batch: None, ..Default::default() }
}

fn format_rows(cache: &Cache) -> i64 {
    cache.backend.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM data", [], |r| r.get(0)).unwrap()
}

fn hash_rows(cache: &Cache) -> i64 {
    cache.checksums().initialize().unwrap();
    cache.backend.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM checksums_db.file_checksums WHERE kind = 'format'", [], |r| r.get(0)).unwrap()
}

/// A library from real calibre has no hashes; afterwards every format file
/// has the right one.
#[test]
fn backfill_gives_every_format_file_of_a_calibre_library_its_hash() {
    let (fixture, cache) = open();
    let formats = format_rows(&cache);
    assert!(formats >= 3, "the fixture should have several format files, got {formats}");
    assert_eq!(hash_rows(&cache), 0, "real calibre never writes a checksum sidecar");

    let control = JobControl::default();
    let report = backfill_hashes(&cache, &options(2, 2), &control).unwrap();

    assert_eq!(report.total as i64, formats);
    assert_eq!(report.hashed as i64, formats);
    assert!(report.missing.is_empty() && report.unreadable.is_empty(), "{report:?}");
    assert_eq!(hash_rows(&cache), formats);
    assert!(cache.checksums().formats_without_hash().unwrap().is_empty());

    // The recorded hash is the file's own.
    for book in BOOKS {
        for format in book.formats {
            let bytes = std::fs::read(book.format_path(fixture.root(), format)).unwrap();
            let (hash, _) = cache.checksums().recorded_identity(book.id, "format", format).unwrap();
            assert_eq!(hash.as_deref(), Some(blake3::hash(&bytes).to_hex().to_string().as_str()), "{} {format}", book.title);
        }
    }
    assert_eq!(control.snapshot().file_opens as i64, formats, "each file is read once");

    // Idempotent: a second run has nothing to do and reads nothing.
    let again = JobControl::default();
    let second = backfill_hashes(&cache, &options(2, 2), &again).unwrap();
    assert_eq!((second.total, second.hashed), (0, 0));
    assert_eq!(again.snapshot().file_opens, 0);
}

/// The reason for doing it: two books holding the same bytes are invisible
/// to content de-duplication until both have hashes.
#[test]
fn backfill_makes_content_duplicates_in_a_calibre_library_visible() {
    let (fixture, cache) = open();
    // Make the first two books' EPUB files byte-identical.
    let (a, b) = (&BOOKS[0], &BOOKS[1]);
    let source = std::fs::read(a.format_path(fixture.root(), "EPUB")).unwrap();
    std::fs::write(b.format_path(fixture.root(), "EPUB"), &source).unwrap();

    assert!(scan_library_for_content_duplicates(&cache).unwrap().is_empty(), "blind before the backfill");

    backfill_hashes(&cache, &options(3, 100), &JobControl::default()).unwrap();

    let groups = scan_library_for_content_duplicates(&cache).unwrap();
    assert_eq!(groups, vec![vec![a.id, b.id]], "the identical files are found once they have hashes");
}

/// A file that is not there is reported, and every other file still gets
/// its hash.
#[test]
fn backfill_reports_a_missing_file_and_carries_on() {
    let (fixture, cache) = open();
    let formats = format_rows(&cache);
    let victim = &BOOKS[0];
    std::fs::remove_file(victim.format_path(fixture.root(), "EPUB")).unwrap();

    let report = backfill_hashes(&cache, &options(2, 100), &JobControl::default()).unwrap();

    assert_eq!(report.missing_count, 1, "{report:?}");
    assert!(report.missing[0].ends_with(".epub"), "{:?}", report.missing);
    assert_eq!(report.hashed as i64, formats - 1);
    assert_eq!(hash_rows(&cache), formats - 1);
}

/// Cancel part-way: what was hashed is kept, and a rerun does exactly the
/// remainder.
#[test]
fn backfill_cancels_and_resumes_where_it_stopped() {
    let (_fixture, cache) = open();
    let formats = format_rows(&cache);

    let control = JobControl::new();
    let hook = control.clone();
    let cancelling = PipelineOptions {
        workers: 2,
        batch_size: 2,
        after_batch: Some(std::sync::Arc::new(move |_: &JobControl| hook.cancel())),
        ..Default::default()
    };
    let first = backfill_hashes(&cache, &cancelling, &control).unwrap();
    assert!(first.cancelled);
    assert_eq!(first.hashed, 2, "the one batch that committed before the cancel");
    assert_eq!(hash_rows(&cache), 2);

    let second = backfill_hashes(&cache, &options(2, 2), &JobControl::default()).unwrap();
    assert!(!second.cancelled);
    assert_eq!(second.total as i64, formats - 2, "the rerun starts from what is left");
    assert_eq!(hash_rows(&cache), formats);
}

/// A rescan of a calibre library must not re-add its books, and must not
/// read the files it already knows to find that out.
#[test]
fn rescanning_a_calibre_library_adds_nothing_new() {
    let (_fixture, cache) = open();
    let books = cache.all_book_ids().unwrap().len();
    let formats = format_rows(&cache) as usize;

    let result = rescan(&cache, std::time::SystemTime::now() + std::time::Duration::from_secs(3600)).unwrap();

    // Every format file the library lists is already known.
    assert!(result.indexed.added.is_empty(), "re-added books that were already there: {:?}", result.indexed);
    assert_eq!(result.indexed.already_known, formats);
    assert_eq!(cache.all_book_ids().unwrap().len(), books);
}

/// A generous bound, not a benchmark: 2,000 small files must scan in
/// seconds, where the old per-file path (three reads, a query and several
/// fsynced writes each) took minutes on a slow disk. The timings are
/// printed so a regression is visible long before the bound trips.
#[test]
fn two_thousand_small_files_scan_in_a_modest_time() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    for i in 0..2_000 {
        let folder = dir.path().join(format!("shelf{:02}", i % 20));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("book{i:05}.pdf")), format!("%PDF-1.4 smoke test {i}").as_bytes()).unwrap();
    }
    let cache = Cache::new(dir.path()).unwrap();
    let later = std::time::SystemTime::now() + Duration::from_secs(3600);

    // Phase one alone -- the part that makes the books appear -- and then
    // the metadata pass, timed apart so a slow one cannot hide in a sum.
    let options = PipelineOptions::default();
    let control = JobControl::default();
    let started = Instant::now();
    let result = lexmechanic_db::scan::rescan_files(&cache, later, &options, &control).unwrap();
    let phase_one = started.elapsed();
    assert_eq!(result.indexed.added.len(), 2_000, "{:?}", result.indexed.failed.first());
    let started = Instant::now();
    lexmechanic_db::scan_pipeline::enrich_pending(&cache, &options, &control).unwrap();
    let phase_two = started.elapsed();
    let first = phase_one + phase_two;
    println!("scan-at-scale smoke: phase one (walk, hash once, register) {phase_one:?}; metadata pass {phase_two:?}");

    let started = Instant::now();
    let again = rescan(&cache, later).unwrap();
    let second = started.elapsed();
    assert_eq!(again.indexed.already_known, 2_000);
    assert!(again.indexed.added.is_empty());

    println!("scan-at-scale smoke: 2000 files first scan (walk + register + metadata) {first:?}; rescan of the same folder {second:?}");
    assert!(first < Duration::from_secs(300), "first scan took {first:?}");
    assert!(second < Duration::from_secs(60), "rescan took {second:?}");
}
