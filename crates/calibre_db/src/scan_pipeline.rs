//! The scan at the scale of a NAS: read each file once, write in
//! batches, and report progress.
//!
//! The first scan (`scan.rs`) was correct and slow. For every new file it
//! read the whole thing three times (once to hash for the ignore list,
//! once for metadata, once to hash again for the checksum), asked the
//! database about it, and wrote it with several autocommit statements and
//! several fsynced change-log files. That is fine for a folder of fifty
//! books and not for hundreds of thousands of PDFs on a network share.
//!
//! # Three jobs, one shape
//!
//! Each is a pass over a list that a [`JobControl`] can cancel and watch:
//!
//! - [`index_files`] -- *phase one of the scan*. Streams each new file
//!   **once** to get its BLAKE3 and size together, and registers it as a
//!   book titled from its filename, in batches of one transaction each.
//!   No metadata parsing and no cover rendering: a book is browsable as
//!   soon as its batch commits.
//! - [`enrich_pending`] -- *phase two*. Reads embedded metadata and
//!   renders covers for the books phase one queued, and fills in only what
//!   the user has not already changed.
//! - [`backfill_hashes`] -- gives every format file that has no content
//!   hash one, which is what makes content de-duplication see a library
//!   opened from real calibre.
//!
//! # Parallelism and determinism
//!
//! Reads (hashing, metadata extraction) run on a small bounded pool of
//! scoped threads -- [`PipelineOptions::workers`], default 3, because a
//! share does not get faster with more concurrent readers and often gets
//! slower. Every database write stays on the calling thread, in file
//! order: workers fill a slot per file, the writer walks the slots in
//! sorted order. So the same folder gives the same books with the same
//! ids whatever the thread count.
//!
//! # Cancel and resume
//!
//! Cancel is cooperative and checked between read chunks, so a 500 MB
//! file does not hold it up. A batch is committed or not at all: nothing
//! is ever half-registered. What a rerun skips is decided by the
//! database, not by anything held in memory -- a file with a book is
//! already known, a book that has left the enrichment queue is already
//! enriched, a format with a checksum record is already hashed.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::Serialize;

use crate::cache::{file_key, Cache, InPlaceFile};
use crate::scan::{DiscoveredFile, IndexReport, ScanReport};

/// How much of a file is read per call. Large enough that a network
/// share is asked for big sequential reads, small enough that cancel is
/// noticed well inside a second.
pub const HASH_BUFFER_BYTES: usize = 1 << 20;

/// Failures kept in a report. A library of 300,000 files with a dead
/// share would otherwise put 300,000 strings in one HTTP response.
pub const MAX_REPORTED: usize = 500;

/// A file's identity: its BLAKE3 and its size, from **one** read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    pub hash: String,
    pub size: u64,
}

/// Streams `path` through BLAKE3 in [`HASH_BUFFER_BYTES`] chunks.
///
/// Never holds more than one chunk. With a `control`, every byte read is
/// counted on it (which is how tests prove a file is read once) and a
/// cancel is noticed between chunks, surfacing as
/// [`io::ErrorKind::Interrupted`].
pub fn hash_stream(path: &Path, control: Option<&JobControl>) -> io::Result<FileDigest> {
    let mut file = std::fs::File::open(path)?;
    if let Some(control) = control {
        control.file_opens.fetch_add(1, Ordering::Relaxed);
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut size = 0u64;
    loop {
        if control.is_some_and(|c| c.is_cancelled()) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buffer[..n]);
        size += n as u64;
        if let Some(control) = control {
            control.bytes_hashed.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
    Ok(FileDigest { hash: hasher.finalize().to_hex().to_string(), size })
}

/// Tuning for the pipeline.
#[derive(Clone)]
pub struct PipelineOptions {
    /// Concurrent readers.
    pub workers: usize,
    /// Books per transaction (and per batch of change-log entries).
    pub batch_size: usize,
    /// Called on the writer thread after each batch is committed. A seam
    /// for tests that need to cancel at an exact point (a cancel timed by
    /// a clock is a flaky test), and for anything that wants a heartbeat.
    pub after_batch: Option<std::sync::Arc<dyn Fn(&JobControl) + Send + Sync>>,
}

impl PipelineOptions {
    pub(crate) fn batch_done(&self, control: &JobControl) {
        if let Some(hook) = &self.after_batch {
            hook(control);
        }
    }
}

/// `CALIBRE_OXIDE_SCAN_WORKERS` if set, else 3 -- or fewer on a machine
/// with fewer cores.
pub fn default_workers() -> usize {
    if let Some(n) = std::env::var("CALIBRE_OXIDE_SCAN_WORKERS").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.clamp(1, 32);
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 3)
}

impl Default for PipelineOptions {
    fn default() -> Self {
        PipelineOptions { workers: default_workers(), batch_size: 100, after_batch: None }
    }
}

impl PipelineOptions {
    pub fn with_workers(workers: usize) -> Self {
        PipelineOptions { workers: workers.clamp(1, 32), ..Default::default() }
    }
}

/// A job's progress and its cancel flag, shared between the thread doing
/// the work and whoever is watching it.
#[derive(Default)]
pub struct JobControl {
    cancelled: AtomicBool,
    phase: Mutex<String>,
    pub total: AtomicU64,
    /// Files found by the walk.
    pub seen: AtomicU64,
    /// Files whose content has been read and hashed.
    pub hashed: AtomicU64,
    /// Books created.
    pub added: AtomicU64,
    /// Files that already had a book, or (for the backfill) a hash.
    pub already_known: AtomicU64,
    /// Files skipped because the user removed the book and kept the file.
    pub ignored: AtomicU64,
    /// Books given their embedded metadata.
    pub enriched: AtomicU64,
    pub errors: AtomicU64,
    pub bytes_hashed: AtomicU64,
    /// Files opened for hashing. Equal to `hashed` unless something read
    /// a file twice.
    pub file_opens: AtomicU64,
}

/// A point-in-time copy of a [`JobControl`], for the status route.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Progress {
    pub phase: String,
    pub cancelled: bool,
    pub total: u64,
    pub seen: u64,
    pub hashed: u64,
    pub added: u64,
    pub already_known: u64,
    pub ignored: u64,
    pub enriched: u64,
    pub errors: u64,
    pub bytes_hashed: u64,
    pub file_opens: u64,
}

impl JobControl {
    pub fn new() -> Arc<JobControl> {
        Arc::new(JobControl::default())
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn set_phase(&self, phase: &str) {
        *self.phase.lock().unwrap() = phase.to_string();
    }

    pub fn snapshot(&self) -> Progress {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        Progress {
            phase: self.phase.lock().unwrap().clone(),
            cancelled: self.is_cancelled(),
            total: load(&self.total),
            seen: load(&self.seen),
            hashed: load(&self.hashed),
            added: load(&self.added),
            already_known: load(&self.already_known),
            ignored: load(&self.ignored),
            enriched: load(&self.enriched),
            errors: load(&self.errors),
            bytes_hashed: load(&self.bytes_hashed),
            file_opens: load(&self.file_opens),
        }
    }
}

/// Runs `work` over `items` on up to `workers` scoped threads, returning
/// one slot per item in item order. A slot is `None` for an item never
/// started because the job was cancelled first.
///
/// Order of the *output* is fixed; order of execution is not, which is
/// fine because `work` only reads.
pub(crate) fn parallel_map<T: Sync, R: Send>(items: &[T], workers: usize, control: &JobControl, work: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let run = || loop {
        if control.is_cancelled() {
            break;
        }
        let i = next.fetch_add(1, Ordering::SeqCst);
        if i >= items.len() {
            break;
        }
        let result = work(&items[i]);
        *slots[i].lock().unwrap() = Some(result);
    };
    let threads = workers.clamp(1, items.len().max(1));
    if threads == 1 {
        run();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(run);
            }
        });
    }
    slots.into_iter().map(|slot| slot.into_inner().unwrap()).collect()
}

fn was_cancelled(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::Interrupted
}

/// Where a format's file lives, trying the lowercase extension (which is
/// how every library writes them) and then the stored spelling.
pub fn format_file_path(library: &Path, folder: &str, name: &str, format: &str) -> Option<PathBuf> {
    let dir = if folder.is_empty() { library.to_path_buf() } else { library.join(folder) };
    [format.to_ascii_lowercase(), format.to_string()]
        .into_iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|candidate| candidate.is_file())
}

fn push_capped<T>(list: &mut Vec<T>, item: T) {
    if list.len() < MAX_REPORTED {
        list.push(item);
    }
}

/// Phase one: registers every file in `report` that has no book yet.
///
/// What it deliberately does **not** do is read the file for anything
/// but its hash. The book is titled from the filename and queued for
/// [`enrich_pending`].
///
/// Returns when every file has been handled or the job is cancelled;
/// `IndexReport::cancelled` says which.
pub fn index_files(cache: &Cache, library_path: &Path, report: &ScanReport, options: &PipelineOptions, control: &JobControl) -> IndexReport {
    let mut indexed = IndexReport::default();
    control.set_phase("indexing");

    // One query each, instead of one or two per file.
    let known = match cache.indexed_file_keys() {
        Ok(known) => known,
        Err(e) => {
            indexed.failed.push((String::new(), format!("could not read the library's existing files: {e}")));
            control.errors.fetch_add(1, Ordering::Relaxed);
            return indexed;
        }
    };
    let (ignored_paths, ignored_hashes) = match cache.ignored().snapshot() {
        Ok(snapshot) => snapshot,
        Err(e) => {
            // Adding a book the user removed is the worse direction to be
            // wrong in, so an unreadable ignore list stops the scan.
            indexed.failed.push((String::new(), format!("could not read the ignore list: {e}")));
            control.errors.fetch_add(1, Ordering::Relaxed);
            return indexed;
        }
    };

    let mut candidates: Vec<&DiscoveredFile> = Vec::new();
    for file in &report.files {
        if known.contains(&file_key(&file.folder, &file.stem, &file.extension)) {
            indexed.already_known += 1;
            control.already_known.fetch_add(1, Ordering::Relaxed);
        } else if ignored_paths.contains(&file.relative_path) {
            // Skipped without being read: the path alone settles it.
            indexed.ignored.push(file.relative_path.clone());
            control.ignored.fetch_add(1, Ordering::Relaxed);
        } else {
            candidates.push(file);
        }
    }
    control.total.store(candidates.len() as u64, Ordering::Relaxed);

    for chunk in candidates.chunks(options.batch_size.max(1)) {
        if control.is_cancelled() {
            break;
        }
        let digests = parallel_map(chunk, options.workers, control, |file| hash_stream(&library_path.join(&file.relative_path), Some(control)));

        // Sorted (the walk sorts) and walked in order by this one thread,
        // so ids do not depend on which worker finished first.
        let mut items: Vec<InPlaceFile> = Vec::new();
        for (file, digest) in chunk.iter().zip(digests) {
            let digest = match digest {
                None => continue, // cancelled before it started
                Some(Err(e)) if was_cancelled(&e) => continue,
                Some(Err(e)) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut indexed.failed, (file.relative_path.clone(), e.to_string()));
                    continue;
                }
                Some(Ok(digest)) => digest,
            };
            control.hashed.fetch_add(1, Ordering::Relaxed);
            if ignored_hashes.contains(&digest.hash) {
                indexed.ignored.push(file.relative_path.clone());
                control.ignored.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if digest.size != file.facts.size {
                // Changed between the walk's stat and the read: still being
                // written, or replaced. A hash of a moving target is a lie.
                control.errors.fetch_add(1, Ordering::Relaxed);
                push_capped(&mut indexed.failed, (file.relative_path.clone(), "changed while it was being read; the next scan will pick it up".to_string()));
                continue;
            }
            let mut metadata = calibre_ebooks::metadata::MetaInformation::default();
            metadata.title = file.stem.clone();
            metadata.authors = vec!["Unknown".to_string()];
            items.push(InPlaceFile {
                relative_path: file.relative_path.clone(),
                metadata,
                size: digest.size as i64,
                hash: digest.hash,
                identity: Some(file.facts.identity),
                queue_enrichment: true,
            });
        }

        let results = cache.register_books_in_place(&items);
        for (item, result) in items.iter().zip(results) {
            match result {
                Ok(id) => {
                    indexed.added.push(id);
                    control.added.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut indexed.failed, (item.relative_path.clone(), e.to_string()));
                }
            }
        }
        options.batch_done(control);
    }
    indexed.cancelled = control.is_cancelled();
    indexed
}

/// What the metadata pass did.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct EnrichReport {
    /// Books that gained a title, authors or a cover.
    pub enriched: usize,
    /// Books whose file has nothing more to say (unreadable metadata, or
    /// the user had already changed everything).
    pub nothing_to_add: usize,
    /// Books whose file was not found. Left queued, so a rerun retries.
    pub missing: Vec<String>,
    pub cancelled: bool,
    /// Books still queued when the pass ended.
    pub remaining: usize,
}

/// Phase two: embedded metadata and covers for the books phase one
/// queued.
///
/// Fills in only what is still the placeholder: a title that is still
/// the filename, authors that are still "Unknown", a book with no cover.
/// A book the user has edited in the meantime keeps their edits.
pub fn enrich_pending(cache: &Cache, options: &PipelineOptions, control: &JobControl) -> Result<EnrichReport> {
    control.set_phase("enriching");
    let library = cache.backend.library_path.clone();
    let pending = cache.checksums().pending_enrichment()?;
    control.total.store(pending.len() as u64, Ordering::Relaxed);
    let mut report = EnrichReport::default();

    // Small chunks: a cancel should not wait for a hundred PDF renders.
    for chunk in pending.chunks((options.workers * 4).max(4)) {
        if control.is_cancelled() {
            break;
        }
        let extracted = parallel_map(chunk, options.workers, control, |(_, format, name, folder)| match format_file_path(&library, folder, name, format) {
            None => Err(None),
            Some(path) => calibre_ebooks::metadata::get_metadata(&path).map_err(Some),
        });

        for ((book_id, _, name, folder), result) in chunk.iter().zip(extracted) {
            let Some(result) = result else { continue }; // cancelled
            match result {
                Err(None) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut report.missing, format!("{folder}/{name}").trim_start_matches('/').to_string());
                }
                Err(Some(_)) => {
                    // Unreadable metadata: the filename title is final.
                    cache.checksums().finish_enrichment(*book_id)?;
                    report.nothing_to_add += 1;
                    control.enriched.fetch_add(1, Ordering::Relaxed);
                }
                Ok(metadata) => {
                    if apply_enrichment(cache, *book_id, name, &metadata)? {
                        report.enriched += 1;
                    } else {
                        report.nothing_to_add += 1;
                    }
                    cache.checksums().finish_enrichment(*book_id)?;
                    control.enriched.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        options.batch_done(control);
    }
    report.cancelled = control.is_cancelled();
    report.remaining = cache.checksums().pending_enrichment()?.len();
    Ok(report)
}

/// Writes what the file knows over what is still a placeholder. Through
/// `set_field` and `set_cover`, so every change reaches the change log.
fn apply_enrichment(cache: &Cache, book_id: i32, stem: &str, metadata: &calibre_ebooks::metadata::MetaInformation) -> Result<bool> {
    let mut changed = false;

    let title = metadata.title.trim();
    if !title.is_empty() && title != "Unknown" && title != stem && cache.field_for(book_id, "title")?.as_deref() == Some(stem) {
        cache.set_field(book_id, "title", title)?;
        changed = true;
    }

    let real_authors: Vec<&str> = metadata.authors.iter().map(|a| a.trim()).filter(|a| !a.is_empty() && *a != "Unknown").collect();
    if !real_authors.is_empty() && cache.field_for(book_id, "authors")?.as_deref().is_none_or(|a| a == "Unknown") {
        cache.set_field(book_id, "authors", &real_authors.join(" & "))?;
        changed = true;
    }

    let (_, cover) = &metadata.cover_data;
    if !cover.is_empty() && !cache.has_cover(book_id)? {
        match crate::covers::set_cover(cache, book_id, cover) {
            Ok(()) => changed = true,
            Err(e) => log::warn!("book {book_id} was enriched but its cover could not be stored: {e}"),
        }
    }
    Ok(changed)
}

/// What the hash backfill did.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct BackfillReport {
    /// Files that now have a content hash.
    pub hashed: usize,
    /// Format files with no hash when the job started.
    pub total: usize,
    /// Rows whose file is not on disk. Reported, not fatal: a library
    /// with one missing file still gets every other hash.
    pub missing: Vec<String>,
    pub missing_count: usize,
    /// `(path, error)` for files that exist but could not be read.
    pub unreadable: Vec<(String, String)>,
    pub unreadable_count: usize,
    pub cancelled: bool,
}

/// Gives every format file with no recorded hash one.
///
/// Resumable by construction: the list of work is "formats with no
/// hash", so each hash written shortens it, and a rerun after a cancel
/// or a crash starts where the last run stopped. On cancel the files
/// already read in the current chunk are still recorded -- a hash is
/// valid on its own, so there is nothing half-done to discard.
pub fn backfill_hashes(cache: &Cache, options: &PipelineOptions, control: &JobControl) -> Result<BackfillReport> {
    control.set_phase("hashing");
    let library = cache.backend.library_path.clone();
    let todo = cache.checksums().formats_without_hash()?;
    control.total.store(todo.len() as u64, Ordering::Relaxed);
    let mut report = BackfillReport { total: todo.len(), ..Default::default() };

    for chunk in todo.chunks(options.batch_size.max(1)) {
        if control.is_cancelled() {
            break;
        }
        let digests = parallel_map(chunk, options.workers, control, |(_, format, name, folder)| match format_file_path(&library, folder, name, format) {
            None => Err(None),
            Some(path) => {
                let identity = calibre_utils::filenames::identity(&path);
                hash_stream(&path, Some(control)).map(|digest| (digest, identity)).map_err(Some)
            }
        });

        let mut records = Vec::new();
        for ((book_id, format, name, folder), result) in chunk.iter().zip(digests) {
            let display = if folder.is_empty() { format!("{name}.{}", format.to_ascii_lowercase()) } else { format!("{folder}/{name}.{}", format.to_ascii_lowercase()) };
            match result {
                None => {}
                Some(Err(Some(e))) if was_cancelled(&e) => {}
                Some(Err(None)) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.missing_count += 1;
                    push_capped(&mut report.missing, display);
                }
                Some(Err(Some(e))) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.unreadable_count += 1;
                    push_capped(&mut report.unreadable, (display, e.to_string()));
                }
                Some(Ok((digest, identity))) => {
                    control.hashed.fetch_add(1, Ordering::Relaxed);
                    records.push(crate::checksums::HashRecord { book_id: *book_id, kind: "format".to_string(), key: format.clone(), hash: digest.hash, size: digest.size as i64, identity });
                }
            }
        }
        cache.checksums().record_many(&records)?;
        report.hashed += records.len();
        options.batch_done(control);
    }
    report.cancelled = control.is_cancelled();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
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

    /// `count` distinct small PDFs `book0000.pdf`.. in a few folders.
    fn fill(dir: &Path, count: usize) {
        for i in 0..count {
            write(&dir.join(format!("shelf{}/book{i:04}.pdf", i % 4)), format!("%PDF-1.4 distinct content {i}").as_bytes());
        }
    }

    fn walked(dir: &Path) -> ScanReport {
        walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap()
    }

    fn options(workers: usize, batch_size: usize) -> PipelineOptions {
        PipelineOptions { workers, batch_size, after_batch: None }
    }

    /// Everything a rebuild has to reproduce, as sorted lines.
    fn snapshot(cache: &Cache) -> Vec<String> {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT b.uuid, b.title, b.path, b.author_sort, IFNULL(d.format, ''), IFNULL(d.name, ''), IFNULL(d.uncompressed_size, -1),
                        IFNULL((SELECT group_concat(a.name, '&') FROM books_authors_link l JOIN authors a ON a.id = l.author WHERE l.book = b.id), '')
                 FROM books b LEFT JOIN data d ON d.book = b.id",
            )
            .unwrap();
        let mut lines: Vec<String> = stmt
            .query_map([], |r| Ok(format!("{}|{}|{}|{}|{}|{}|{}|{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, i64>(6)?, r.get::<_, String>(7)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        lines.sort();
        lines
    }

    /// A second library holding only the first one's change log, rebuilt.
    fn rebuild_from_log_only(source: &Path) -> (tempfile::TempDir, Cache) {
        let fresh = tempfile::tempdir().unwrap();
        let (from, to) = (source.join(crate::constants::LIBRARY_HANDLE_DIR_NAME), fresh.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME));
        for sub in ["changes", "snapshots"] {
            std::fs::create_dir_all(to.join(sub)).unwrap();
            if let Ok(entries) = std::fs::read_dir(from.join(sub)) {
                for entry in entries.flatten() {
                    std::fs::copy(entry.path(), to.join(sub).join(entry.file_name())).unwrap();
                }
            }
        }
        if let Ok(id) = std::fs::read(from.join("install-id")) {
            std::fs::write(to.join("install-id"), id).unwrap();
        }
        let cache = Cache::new(fresh.path()).unwrap();
        cache.rebuild_from_change_log().unwrap();
        (fresh, cache)
    }

    fn count(cache: &Cache, sql: &str) -> i64 {
        cache.backend.conn.lock().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn hash_stream_matches_a_whole_file_hash_across_chunk_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        // Three chunks and a bit, so a bug at a boundary would show.
        let bytes: Vec<u8> = (0..(3 * HASH_BUFFER_BYTES + 7)).map(|i| (i % 251) as u8).collect();
        write(&dir.path().join("big.bin"), &bytes);

        let digest = hash_stream(&dir.path().join("big.bin"), None).unwrap();
        assert_eq!(digest.size, bytes.len() as u64);
        assert_eq!(digest.hash, blake3::hash(&bytes).to_hex().to_string());
    }

    /// The point of the whole change: a new file is opened and read to the
    /// end exactly once -- not once for the ignore check, once for the
    /// metadata and once for the checksum.
    #[test]
    fn a_new_file_is_read_exactly_once() {
        let (dir, cache) = library();
        let sizes = [10usize, 1_000, HASH_BUFFER_BYTES + 5, 3 * HASH_BUFFER_BYTES];
        for (i, size) in sizes.iter().enumerate() {
            write(&dir.path().join(format!("f{i}.pdf")), &vec![b'x'; *size]);
        }

        let control = JobControl::default();
        let indexed = index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 100), &control);

        assert_eq!(indexed.added.len(), 4, "{indexed:?}");
        let progress = control.snapshot();
        assert_eq!(progress.file_opens, 4, "each file must be opened once, not once per question asked of it");
        assert_eq!(progress.bytes_hashed, sizes.iter().sum::<usize>() as u64, "each byte must be read once");
        assert_eq!(progress.hashed, 4);
    }

    /// A file that is already indexed is not read at all on a rescan.
    #[test]
    fn a_rescan_reads_nothing_it_already_knows() {
        let (dir, cache) = library();
        fill(dir.path(), 20);
        let report = walked(dir.path());
        index_files(&cache, dir.path(), &report, &options(2, 100), &JobControl::default());

        let control = JobControl::default();
        let again = index_files(&cache, dir.path(), &report, &options(2, 100), &control);
        assert_eq!(again.already_known, 20);
        assert!(again.added.is_empty());
        assert_eq!(control.snapshot().file_opens, 0, "a known file must not be opened");
    }

    /// A path the user removed-and-kept is skipped without being read.
    #[test]
    fn a_path_ignored_file_is_skipped_without_being_read() {
        let (dir, cache) = library();
        write(&dir.path().join("kept.pdf"), b"%PDF kept");
        cache.ignored().ignore("kept.pdf", None, "Kept").unwrap();

        let control = JobControl::default();
        let indexed = index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 100), &control);
        assert_eq!(indexed.ignored, vec!["kept.pdf"]);
        assert_eq!(control.snapshot().file_opens, 0);
    }

    /// 250 books in batches of 100: three transactions, and every book has
    /// all of its rows, its hash, its queue entry -- and its place in the
    /// log, which is proven by throwing the database away and rebuilding.
    #[test]
    fn a_batched_scan_is_complete_and_survives_a_rebuild_from_the_log() {
        let (dir, cache) = library();
        fill(dir.path(), 250);

        let indexed = index_files(&cache, dir.path(), &walked(dir.path()), &options(3, 100), &JobControl::default());
        assert_eq!(indexed.added.len(), 250, "{:?}", indexed.failed);
        assert!(indexed.failed.is_empty());

        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books"), 250);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM data"), 250);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM checksums_db.file_checksums WHERE kind = 'format'"), 250);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM checksums_db.enrichment_queue"), 250);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books_authors_link"), 250);

        // The hash is the file's, from the one read.
        let bytes = std::fs::read(dir.path().join("shelf1/book0001.pdf")).unwrap();
        let id = cache.book_for_file("shelf1/book0001.pdf").unwrap().unwrap();
        let (hash, _) = cache.checksums().recorded_identity(id, "format", "PDF").unwrap();
        assert_eq!(hash.unwrap(), blake3::hash(&bytes).to_hex().to_string());

        let before = snapshot(&cache);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before, "a rebuild from the change log must reproduce the batched scan");

        // 250 books are three batches, and a batch is one log entry -- one
        // fsync -- not nine per book. That is what makes this viable on a
        // network share.
        let log = cache.backend.change_log().unwrap();
        let entries = std::fs::read_dir(dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes")).unwrap().count();
        assert_eq!(entries, 3, "one change-log file per batch");
        assert!(log.verify().unwrap().is_clean(), "the hash chain must still verify");
    }

    /// Compaction keeps a batch whole, and a rebuild from the compacted
    /// log still reproduces the library, including a later edit.
    #[test]
    fn a_compacted_log_with_batches_still_rebuilds() {
        let (dir, cache) = library();
        fill(dir.path(), 30);
        index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 10), &JobControl::default());
        let id = cache.book_for_file("shelf1/book0001.pdf").unwrap().unwrap();
        cache.set_field(id, "title", "Edited After The Scan").unwrap();

        let log = cache.backend.change_log().unwrap();
        // Far in the future, so everything is past the retention horizon.
        log.compact(std::time::Duration::from_secs(0), u64::MAX / 2).unwrap();

        let before = snapshot(&cache);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before);
        assert!(before.iter().any(|l| l.contains("Edited After The Scan")));
    }

    /// A book in the batch that cannot be inserted is reported on its own
    /// and does not take the batch down with it.
    #[test]
    fn one_bad_item_in_a_batch_does_not_lose_the_others() {
        let (dir, cache) = library();
        write(&dir.path().join("good.pdf"), b"%PDF good");
        write(&dir.path().join("also-good.pdf"), b"%PDF also good");
        let item = |name: &str| {
            let mut metadata = calibre_ebooks::metadata::MetaInformation::default();
            metadata.title = name.to_string();
            metadata.authors = vec!["Unknown".to_string()];
            InPlaceFile { relative_path: name.to_string(), metadata, size: 5, hash: "00".repeat(32), identity: None, queue_enrichment: true }
        };
        // No extension: `register_books_in_place` refuses it.
        let results = cache.register_books_in_place(&[item("good.pdf"), item("no-extension"), item("also-good.pdf")]);
        assert!(results[0].is_ok() && results[2].is_ok());
        assert!(results[1].is_err());
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books"), 2);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM data"), 2);
    }

    /// Cancel part-way, and what is there is whole books only; a rerun
    /// finishes the rest without duplicating anything.
    #[test]
    fn a_cancelled_scan_leaves_whole_books_and_a_rerun_finishes_it() {
        let (dir, cache) = library();
        fill(dir.path(), 100);
        let report = walked(dir.path());

        let control = JobControl::new();
        let hook_control = control.clone();
        let cancelling = PipelineOptions {
            workers: 3,
            batch_size: 10,
            // After the second committed batch, and not before.
            after_batch: Some(std::sync::Arc::new(move |c: &JobControl| {
                if c.added.load(Ordering::Relaxed) >= 20 {
                    hook_control.cancel();
                }
            })),
        };
        let first = index_files(&cache, dir.path(), &report, &cancelling, &control);
        assert!(first.cancelled);
        assert_eq!(first.added.len(), 20, "exactly the committed batches survive");

        // Nothing half-registered: every book has its data row, hash,
        // author link and queue entry.
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books"), 20);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM data"), 20);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM checksums_db.file_checksums"), 20);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books_authors_link"), 20);

        // Resume: a fresh control, the same folder.
        let second = index_files(&cache, dir.path(), &report, &options(3, 10), &JobControl::default());
        assert!(!second.cancelled);
        assert_eq!(second.already_known, 20, "the first run's books are skipped, not redone");
        assert_eq!(second.added.len(), 80);
        assert_eq!(count(&cache, "SELECT COUNT(*) FROM books"), 100);
        assert_eq!(count(&cache, "SELECT COUNT(DISTINCT path || '/' || (SELECT name FROM data WHERE book = books.id)) FROM books"), 100);

        // And the interrupted-then-resumed library is still fully in the log.
        let before = snapshot(&cache);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before);
    }

    /// Cancelling during a read stops it promptly even inside one file.
    #[test]
    fn cancel_interrupts_a_read_between_chunks() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("big.bin"), &vec![1u8; 4 * HASH_BUFFER_BYTES]);
        let control = JobControl::default();
        control.cancel();
        let err = hash_stream(&dir.path().join("big.bin"), Some(&control)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }

    /// The same folder gives the same books in the same order whatever the
    /// thread count: ids are assigned by the one writer, in sorted order.
    #[test]
    fn the_result_does_not_depend_on_the_thread_count() {
        let mut results = Vec::new();
        for workers in [1usize, 2, 8] {
            let (dir, cache) = library();
            fill(dir.path(), 60);
            index_files(&cache, dir.path(), &walked(dir.path()), &options(workers, 7), &JobControl::default());
            let conn = cache.backend.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT b.id, b.path, d.name, d.format, d.uncompressed_size, c.blake3_hex FROM books b JOIN data d ON d.book = b.id JOIN checksums_db.file_checksums c ON c.book_id = b.id ORDER BY b.id")
                .unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |r| Ok(format!("{}|{}|{}|{}|{}|{}", r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, String>(5)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            results.push(rows);
        }
        assert_eq!(results[0].len(), 60);
        assert_eq!(results[0], results[1], "1 worker and 2 workers disagree");
        assert_eq!(results[0], results[2], "1 worker and 8 workers disagree");
    }

    /// Phase two fills in what the file knows, only where the book still
    /// has the placeholder, and a finished book is never touched again.
    #[test]
    fn enrichment_fills_placeholders_skips_edited_books_and_is_resumable() {
        let (dir, cache) = library();
        let html = |title: &str| format!("<html><head><title>{title}</title><meta name=\"author\" content=\"Real Author\"/></head><body>x</body></html>");
        write(&dir.path().join("scan001.html"), html("Real Title One").as_bytes());
        write(&dir.path().join("scan002.html"), html("Real Title Two").as_bytes());
        index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 100), &JobControl::default());

        let one = cache.book_for_file("scan001.html").unwrap().unwrap();
        let two = cache.book_for_file("scan002.html").unwrap().unwrap();
        assert_eq!(cache.field_for(one, "title").unwrap().as_deref(), Some("scan001"), "phase one titles from the filename");
        // The user renames one before the metadata pass gets to it.
        cache.set_field(two, "title", "My Own Title").unwrap();

        // Cancelled before it starts: nothing changes and everything is
        // still queued.
        let cancelled = JobControl::default();
        cancelled.cancel();
        let report = enrich_pending(&cache, &options(2, 100), &cancelled).unwrap();
        assert!(report.cancelled);
        assert_eq!(report.remaining, 2);

        let report = enrich_pending(&cache, &options(2, 100), &JobControl::default()).unwrap();
        assert_eq!(report.remaining, 0);
        assert_eq!(cache.field_for(one, "title").unwrap().as_deref(), Some("Real Title One"));
        assert_eq!(cache.field_for(two, "title").unwrap().as_deref(), Some("My Own Title"), "an edit the user made must survive enrichment");

        // Already enriched: nothing is queued, so nothing is read.
        let again = enrich_pending(&cache, &options(2, 100), &JobControl::default()).unwrap();
        assert_eq!((again.enriched, again.nothing_to_add, again.remaining), (0, 0, 0));

        // The enrichment is in the log too.
        let before = snapshot(&cache);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before);
    }

    /// A queued book whose file has vanished is reported and stays queued,
    /// so the next run retries it; it is not silently dropped.
    #[test]
    fn enrichment_keeps_a_book_queued_when_its_file_is_missing() {
        let (dir, cache) = library();
        write(&dir.path().join("gone.pdf"), b"%PDF gone");
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        std::fs::remove_file(dir.path().join("gone.pdf")).unwrap();

        let report = enrich_pending(&cache, &options(1, 100), &JobControl::default()).unwrap();
        assert_eq!(report.missing, vec!["gone"]);
        assert_eq!(report.remaining, 1);
    }

    #[test]
    fn a_file_that_changed_while_being_read_is_left_for_the_next_scan() {
        let (dir, cache) = library();
        write(&dir.path().join("moving.pdf"), b"%PDF twelve bytes");
        let mut report = walked(dir.path());
        // The walk saw a different size than the read will.
        report.files[0].facts.size += 1;
        let indexed = index_files(&cache, dir.path(), &report, &options(1, 100), &JobControl::default());
        assert!(indexed.added.is_empty());
        assert_eq!(indexed.failed.len(), 1);
        assert!(indexed.failed[0].1.contains("changed"));
    }
}
