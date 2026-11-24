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
    hash_stream_with_identity(path, control).map(|(digest, _)| digest)
}

/// [`hash_stream`], also answering the file's filesystem identity from the
/// **same** open handle. Asking for it with a separate `identity(path)` opened
/// every file twice, which on a scanned disk or a share is most of what hashing
/// a library of small files costs.
pub fn hash_stream_with_identity(path: &Path, control: Option<&JobControl>) -> io::Result<(FileDigest, Option<lexmechanic_utils::filenames::FileIdentity>)> {
    let mut file = std::fs::File::open(path)?;
    let identity = lexmechanic_utils::filenames::identity_of_open_file(&file);
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
    Ok((FileDigest { hash: hasher.finalize().to_hex().to_string(), size }, identity))
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
    /// Which reader the PDF jobs use: the out-of-process worker pool when a
    /// worker binary is found (the default), or always the in-process reader.
    pub pdf_pool: crate::pdf_worker::PdfPoolChoice,
    /// How many PDF worker processes, overriding `LEXMECHANIC_PDF_WORKERS`
    /// and the default (half the cores, at most 4). `Some(0)` means none: the
    /// in-process reader.
    pub pdf_workers: Option<usize>,
    /// Width of covers rendered from PDFs during enrichment; `None` is
    /// [`lexmechanic_ebooks::pdf::rasterize::scan_cover_width`].
    pub cover_width: Option<u32>,
}

impl PipelineOptions {
    pub(crate) fn batch_done(&self, control: &JobControl) {
        if let Some(hook) = &self.after_batch {
            hook(control);
        }
    }
}

/// `LEXMECHANIC_SCAN_WORKERS` if set, else 3 -- or fewer on a machine
/// with fewer cores.
pub fn default_workers() -> usize {
    if let Some(n) = lexmechanic_utils::env::var("LEXMECHANIC_SCAN_WORKERS").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.clamp(1, 32);
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 3)
}

impl Default for PipelineOptions {
    fn default() -> Self {
        PipelineOptions { workers: default_workers(), batch_size: 100, after_batch: None, pdf_pool: Default::default(), pdf_workers: None, cover_width: None }
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
    /// PDFs given a page/text fingerprint.
    pub fingerprinted: AtomicU64,
    pub bytes_hashed: AtomicU64,
    /// Files opened for hashing. Equal to `hashed` unless something read
    /// a file twice.
    pub file_opens: AtomicU64,
    /// When non-zero, the walk stops once it has seen this many files:
    /// a bounded look, for "how many untracked files are there, roughly".
    pub stop_after_seen: AtomicU64,
    /// Wall time spent per named phase, in milliseconds (see
    /// [`JobControl::add_time`]).
    timings: Mutex<std::collections::BTreeMap<String, u64>>,
    /// When set, a bounded walk also stops at this instant.
    walk_deadline: Mutex<Option<std::time::Instant>>,
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
    pub fingerprinted: u64,
    pub bytes_hashed: u64,
    pub file_opens: u64,
    /// Where the time went, in milliseconds, by phase: `walk`, `drift`,
    /// `index_hash`, `index_write`, `enrich_read`, `enrich_write`, `covers`,
    /// `fingerprint`, `hash_backfill`. A phase that did not run is absent.
    pub timings_ms: std::collections::BTreeMap<String, u64>,
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

    /// Whether a bounded walk ([`JobControl::stop_after_seen`]) has seen
    /// as many files as it was allowed.
    pub fn walk_limit_reached(&self) -> bool {
        let limit = self.stop_after_seen.load(Ordering::Relaxed);
        (limit > 0 && self.seen.load(Ordering::Relaxed) >= limit) || self.walk_deadline.lock().unwrap().is_some_and(|d| std::time::Instant::now() >= d)
    }

    /// Makes a bounded walk stop after `budget` of wall time as well.
    pub fn set_walk_budget(&self, budget: std::time::Duration) {
        *self.walk_deadline.lock().unwrap() = Some(std::time::Instant::now() + budget);
    }

    /// Adds `elapsed` to the named phase's total.
    pub fn add_time(&self, phase: &str, elapsed: std::time::Duration) {
        *self.timings.lock().unwrap().entry(phase.to_string()).or_insert(0) += elapsed.as_millis() as u64;
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
            fingerprinted: load(&self.fingerprinted),
            bytes_hashed: load(&self.bytes_hashed),
            file_opens: load(&self.file_opens),
            timings_ms: self.timings.lock().unwrap().clone(),
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
        let hash_started = std::time::Instant::now();
        let digests = parallel_map(chunk, options.workers, control, |file| match report.precomputed.get(&file.relative_path) {
            // Hashed by the drift pass, which had to read it to look for a
            // moved file. Valid only for the size the walk saw.
            Some(known) if known.size == file.facts.size => {
                Ok(known.clone())
            }
            _ => hash_stream(&library_path.join(&file.relative_path), Some(control)),
        });
        control.add_time("index_hash", hash_started.elapsed());

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
            let mut metadata = lexmechanic_ebooks::metadata::MetaInformation::default();
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

        let write_started = std::time::Instant::now();
        let results = register_batch_resilient(&items, |batch| cache.register_books_in_place(batch));
        control.add_time("index_write", write_started.elapsed());
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

/// Registers `items` as one batch; if every book of a multi-book batch comes
/// back failed (a batch-level failure such as the commit, rather than one bad
/// book -- those are isolated by their own savepoints), retries them one at a
/// time so a single poisonous book is reported on its own and the rest of the
/// library still gets indexed.
fn register_batch_resilient(items: &[InPlaceFile], register: impl Fn(&[InPlaceFile]) -> Vec<Result<i32>>) -> Vec<Result<i32>> {
    let results = register(items);
    if items.len() > 1 && results.iter().all(|r| r.is_err()) {
        return items.iter().map(|item| register(std::slice::from_ref(item)).remove(0)).collect();
    }
    results
}

/// What the metadata pass did.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct EnrichReport {
    /// `(book path, error)` for books whose metadata could not be written.
    /// One bad book is recorded here and the pass carries on; it used to
    /// abort the whole job.
    pub failed: Vec<(String, String)>,
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
///
/// # PDFs and the worker pool
///
/// A PDF is read by PDFium, native code that can crash or hang on a bad file.
/// When a PDF worker binary is available ([`crate::pdf_worker`]) PDFs are read
/// in worker processes, in parallel: a crash or a timeout is recorded against
/// that one book (it keeps its filename title and leaves the queue) and the
/// pass goes on. If the content hash is known and the PDF has no fingerprint
/// yet, the worker computes the fingerprint in the same open of the file.
/// Without a worker binary the PDF is read in-process, as before.
///
/// Either way a PDF is marked *in flight* in the database before it is
/// handed over. A run that died while reading one (a crash of this process
/// rather than of a worker) finds the mark on its next start and records the
/// book as crashed instead of trying the same file again.
///
/// # Writes
///
/// Reading is in small chunks, so a cancel is prompt; writing is in batches of
/// [`PipelineOptions::batch_size`] books, one transaction and one change-log
/// entry each ([`Cache::write_enrichment_batch`]).
pub fn enrich_pending(cache: &Cache, options: &PipelineOptions, control: &JobControl) -> Result<EnrichReport> {
    use crate::enrich_batch::EnrichmentWrite;

    control.set_phase("enriching");
    let library = cache.backend.library_path.clone();
    let store = cache.checksums();
    let mut report = EnrichReport::default();

    // Books whose isolated retry killed the process: crashed, not tried again.
    for key in store.take_inflight("enrich-suspect")? {
        if let Ok(book_id) = key.parse::<i32>() {
            store.finish_enrichment(book_id)?;
            control.errors.fetch_add(1, Ordering::Relaxed);
            push_capped(&mut report.failed, (format!("book {book_id}"), "crashed: this PDF was being read alone when the process died, so it was not tried again".to_string()));
        }
    }
    // Books a previous run was reading when it died are suspects, not
    // failures: read first, alone, in a fresh worker.
    let suspects: std::collections::HashSet<i32> = store.take_inflight("enrich")?.into_iter().filter_map(|k| k.parse().ok()).collect();

    let mut pending = store.pending_enrichment()?;
    pending.sort_by_key(|(book_id, ..)| !suspects.contains(book_id));
    control.total.store(pending.len() as u64, Ordering::Relaxed);
    let pool = if pending.iter().any(|(_, format, _, _)| format.eq_ignore_ascii_case("pdf")) { options.pdf_pool.resolve(options.pdf_workers) } else { None };
    let read_threads = pool.as_ref().map_or(options.workers, |p| p.workers().max(options.workers));
    let cover_width = options.cover_width.unwrap_or_else(lexmechanic_ebooks::pdf::rasterize::scan_cover_width);

    enum Read {
        /// The file is not on disk.
        Missing,
        /// What the file says (possibly nothing).
        Meta(Box<lexmechanic_ebooks::metadata::MetaInformation>),
        /// The reader died or hung on it.
        Crashed(String),
    }

    let mut staged: Vec<EnrichmentWrite> = Vec::new();
    let mut crashed: std::collections::HashSet<i32> = std::collections::HashSet::new();

    let flush = |staged: &mut Vec<EnrichmentWrite>, crashed: &std::collections::HashSet<i32>, report: &mut EnrichReport| {
        if staged.is_empty() {
            return;
        }
        let results = cache.write_enrichment_batch(staged, Some(control));
        for (write, result) in staged.iter().zip(results) {
            if !crashed.contains(&write.book_id) {
                control.enriched.fetch_add(1, Ordering::Relaxed);
            }
            match result {
                Ok(true) => report.enriched += 1,
                Ok(false) => {
                    if !crashed.contains(&write.book_id) {
                        report.nothing_to_add += 1
                    }
                }
                Err(e) => {
                    // One book's metadata must not stop the library's scan.
                    // Its filename title stays; it leaves the queue so a
                    // rerun does not trip on it forever.
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut report.failed, (write.label.clone(), format!("{e:#}")));
                    // The batch failed as a whole if the commit did; make sure
                    // none of these stay queued to fail again.
                    let _ = cache.checksums().finish_enrichment(write.book_id);
                }
            }
        }
        staged.clear();
    };

    // Suspects one to a batch (they are sorted first), then small chunks: a
    // cancel should not wait for a hundred PDF renders.
    let suspect_count = pending.iter().take_while(|(book_id, ..)| suspects.contains(book_id)).count();
    let (suspect_part, rest_part) = pending.split_at(suspect_count);
    let mut batches: Vec<(&[(i32, String, String, String)], bool)> = suspect_part.chunks(1).map(|c| (c, true)).collect();
    batches.extend(rest_part.chunks((read_threads * 4).max(4)).map(|c| (c, false)));
    for (chunk, suspect) in batches {
        if control.is_cancelled() {
            break;
        }
        let (enrich_kind, fp_kind) = if suspect { ("enrich-suspect", "fingerprint-suspect") } else { ("enrich", "fingerprint") };
        let read_threads = if suspect { 1 } else { read_threads };
        let pool: Option<std::sync::Arc<crate::pdf_worker::Pool>> = if suspect { pool.as_ref().map(|p| std::sync::Arc::new(p.isolated())) } else { pool.clone() };
        let read_started = std::time::Instant::now();
        let extracted = parallel_map(chunk, read_threads, control, |(book_id, format, name, folder)| -> Read {
            let Some(path) = format_file_path(&library, folder, name, format) else { return Read::Missing };
            if !format.eq_ignore_ascii_case("pdf") {
                return match lexmechanic_ebooks::metadata::get_metadata(&path) {
                    Ok(mi) => Read::Meta(Box::new(mi)),
                    // Unreadable metadata: the filename title is final.
                    Err(_) => Read::Meta(Box::default()),
                };
            }
            let key = book_id.to_string();
            let hash = store.recorded_identity(*book_id, "format", format).ok().and_then(|(hash, _)| hash);
            let wants_fingerprint = pool.is_some() && hash.as_deref().is_some_and(|h| !store.has_pdf_fingerprint(h).unwrap_or(true));
            let _ = store.mark_inflight(enrich_kind, &key);
            if wants_fingerprint {
                let _ = store.mark_inflight(fp_kind, hash.as_deref().unwrap_or_default());
            }
            let finish = || {
                let _ = store.clear_inflight(enrich_kind, &key);
                if let Some(h) = hash.as_deref().filter(|_| wants_fingerprint) {
                    let _ = store.clear_inflight(fp_kind, h);
                }
            };
            let in_process = || lexmechanic_ebooks::metadata::pdf::get_metadata_from_path_at(&path, Some(cover_width)).map(Box::new).map(Read::Meta).unwrap_or_else(|_| Read::Meta(Box::default()));
            let Some(pool) = pool.as_ref() else {
                let out = in_process();
                finish();
                return out;
            };
            let reply = pool.run(crate::pdf_worker::Request { id: 0, path: path.to_string_lossy().to_string(), cover_width: Some(cover_width), fingerprint: wants_fingerprint });
            let out = match reply {
                crate::pdf_worker::Reply::Done(resp) => {
                    if resp.unavailable || resp.error.is_some() {
                        // PDFium could not read it (or is not there). The
                        // stream reader may still find a title; PDFium is not
                        // tried again in this process.
                        let why = resp.error.clone().unwrap_or_default();
                        Read::Meta(lexmechanic_ebooks::metadata::pdf::get_metadata_without_pdfium(&path, &why).map(Box::new).unwrap_or_default())
                    } else {
                        if let (Some(h), true) = (hash.as_deref(), wants_fingerprint) {
                            if let Some(fp) = &resp.fingerprint {
                                let _ = store.record_pdf_fingerprint(h, &Ok(fp.clone()));
                            } else if let Some(why) = &resp.fingerprint_error {
                                let _ = store.record_pdf_fingerprint(h, &Err(why.clone()));
                            }
                        }
                        let info = resp.info.clone().unwrap_or_default().into();
                        Read::Meta(Box::new(lexmechanic_ebooks::metadata::pdf::metadata_from_pdfium_info(info, resp.cover())))
                    }
                }
                reply @ (crate::pdf_worker::Reply::Crashed(_) | crate::pdf_worker::Reply::TimedOut) => {
                    let why = reply.failure_reason().unwrap_or_default();
                    if let (Some(h), true) = (hash.as_deref(), wants_fingerprint) {
                        let _ = store.record_pdf_fingerprint(h, &Err(why.clone()));
                    }
                    Read::Crashed(why)
                }
                crate::pdf_worker::Reply::SpawnFailed(why) => {
                    log::warn!("the PDF worker could not be started ({why}); reading in-process");
                    in_process()
                }
            };
            finish();
            out
        });
        control.add_time("enrich_read", read_started.elapsed());

        for ((book_id, _, name, folder), result) in chunk.iter().zip(extracted) {
            let Some(result) = result else { continue }; // cancelled
            let display = format!("{folder}/{name}").trim_start_matches('/').to_string();
            match result {
                Read::Missing => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut report.missing, display);
                }
                Read::Crashed(why) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    push_capped(&mut report.failed, (display.clone(), why));
                    crashed.insert(*book_id);
                    staged.push(EnrichmentWrite { book_id: *book_id, label: display, stem: name.clone(), ..Default::default() });
                }
                Read::Meta(metadata) => {
                    // Junk (`... print job`, `Microsoft Word - x.doc`, a bare
                    // filename) is treated as absent so the filename title
                    // stays; see `junk_metadata`.
                    let title = crate::junk_metadata::clean_title(&metadata.title);
                    let authors: Vec<String> = crate::junk_metadata::clean_authors(&metadata.authors).into_iter().filter(|a| a != "Unknown").collect();
                    let (_, cover) = &metadata.cover_data;
                    staged.push(EnrichmentWrite { book_id: *book_id, label: display, stem: name.clone(), title, authors, cover: (!cover.is_empty()).then(|| cover.clone()) });
                }
            }
        }
        if staged.len() >= options.batch_size.max(1) {
            flush(&mut staged, &crashed, &mut report);
        }
        options.batch_done(control);
    }
    flush(&mut staged, &crashed, &mut report);
    report.cancelled = control.is_cancelled();
    report.remaining = cache.checksums().pending_enrichment()?.len();
    Ok(report)
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
    let started = std::time::Instant::now();
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
                // One open for both the content and the identity.
                hash_stream_with_identity(&path, Some(control)).map_err(Some)
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
    control.add_time("hash_backfill", started.elapsed());
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
        PipelineOptions { workers, batch_size, after_batch: None, ..Default::default() }
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
            let mut metadata = lexmechanic_ebooks::metadata::MetaInformation::default();
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
            ..Default::default()
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

    /// A whole-batch failure falls back to one book at a time, so the one
    /// poisonous book is the only casualty.
    #[test]
    fn a_failed_batch_is_retried_book_by_book() {
        let item = |name: &str| {
            let mut metadata = lexmechanic_ebooks::metadata::MetaInformation::default();
            metadata.title = name.to_string();
            InPlaceFile { relative_path: name.to_string(), metadata, size: 1, hash: "00".repeat(32), identity: None, queue_enrichment: false }
        };
        let items = [item("a.pdf"), item("poison.pdf"), item("c.pdf")];
        let register = |batch: &[InPlaceFile]| -> Vec<Result<i32>> {
            let poisoned = batch.iter().any(|i| i.relative_path == "poison.pdf");
            batch
                .iter()
                .map(|i| if poisoned && (batch.len() > 1 || i.relative_path == "poison.pdf") { Err(anyhow::anyhow!("boom")) } else { Ok(1) })
                .collect()
        };
        let results = register_batch_resilient(&items, register);
        assert!(results[0].is_ok() && results[2].is_ok());
        assert!(results[1].is_err());
    }

    /// One book whose metadata cannot be written is a per-file error; the
    /// enrichment pass still finishes every other book.
    #[test]
    fn a_book_that_cannot_be_enriched_is_reported_and_the_pass_continues() {
        let (dir, cache) = library();
        let html = |title: &str, author: &str| format!("<html><head><title>{title}</title><meta name=\"author\" content=\"{author}\"/></head><body>x</body></html>");
        write(&dir.path().join("a1.html"), html("Good One", "Fine Author").as_bytes());
        write(&dir.path().join("a2.html"), html("Bad One", "Boom Author").as_bytes());
        write(&dir.path().join("a3.html"), html("Good Three", "Fine Author").as_bytes());
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        cache
            .backend
            .conn
            .lock()
            .unwrap()
            .execute_batch("CREATE TRIGGER boom BEFORE INSERT ON books_authors_link WHEN (SELECT name FROM authors WHERE id = NEW.author) = 'Boom Author' BEGIN SELECT RAISE(ABORT, 'boom'); END;")
            .unwrap();
        let control = JobControl::default();
        let report = enrich_pending(&cache, &options(1, 100), &control).unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report.failed[0].0.contains("a2"), "{report:?}");
        assert_eq!(control.errors.load(Ordering::Relaxed), 1);
        assert_eq!(report.remaining, 0);
        let three = cache.book_for_file("a3.html").unwrap().unwrap();
        assert_eq!(cache.field_for(three, "title").unwrap().as_deref(), Some("Good Three"));
    }

    /// The scan-level face of the duplicate-author crash: embedded metadata
    /// listing the same author twice enriches cleanly.
    #[test]
    fn enrichment_with_a_repeated_author_does_not_fail() {
        let (dir, cache) = library();
        write(&dir.path().join("d.html"), b"<html><head><title>Dup Title</title><meta name=\"author\" content=\"Same Person &amp; same person\"/></head><body>x</body></html>");
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        let report = enrich_pending(&cache, &options(1, 100), &JobControl::default()).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        let id = cache.book_for_file("d.html").unwrap().unwrap();
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Dup Title"));
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

    // -----------------------------------------------------------------
    // Tranche 13: worker pool, batched enrichment, compaction, timings
    // -----------------------------------------------------------------

    fn pool_options(workers: usize, batch_size: usize) -> PipelineOptions {
        PipelineOptions {
            workers,
            batch_size,
            pdf_pool: crate::pdf_worker::PdfPoolChoice::Pool(std::sync::Arc::new(crate::pdf_worker::tests::fake_pool(workers, std::time::Duration::from_secs(60)))),
            ..PipelineOptions::default()
        }
    }

    fn inflight_rows(cache: &Cache) -> i64 {
        count(cache, "SELECT COUNT(*) FROM checksums_db.inflight")
    }

    fn change_files(dir: &Path) -> usize {
        std::fs::read_dir(dir.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes")).unwrap().count()
    }

    /// PDFs are read by worker processes; a file that kills its worker costs
    /// that one book (it keeps its filename title and leaves the queue, and
    /// its fingerprint is recorded as crashed), everything else is enriched,
    /// the PDFs' fingerprints are computed in the same read, and the whole
    /// chunk is written as one change-log entry that a rebuild reproduces.
    #[test]
    fn a_pool_enriches_pdfs_and_a_crash_costs_one_book() {
        let (dir, cache) = library();
        for name in ["good-a", "good-b", "good-c", "crash-d", "plain-e"] {
            write(&dir.path().join(format!("shelf/{name}.pdf")), format!("%PDF-1.4 {name}").as_bytes());
        }
        index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 100), &JobControl::default());
        let entries_after_indexing = change_files(dir.path());

        let control = JobControl::default();
        let report = enrich_pending(&cache, &pool_options(2, 2), &control).unwrap();
        assert_eq!(report.remaining, 0, "{report:?}");
        assert_eq!(report.enriched, 3, "{report:?}");
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report.failed[0].0.contains("crash-d") && report.failed[0].1.starts_with("crashed:"), "{report:?}");

        let title_of = |name: &str| {
            let id = cache.book_for_file(&format!("shelf/{name}.pdf")).unwrap().unwrap();
            (id, cache.field_for(id, "title").unwrap().unwrap(), cache.field_for(id, "authors").unwrap().unwrap(), cache.has_cover(id).unwrap())
        };
        assert_eq!(title_of("good-a").1, "Title of good-a");
        assert_eq!((title_of("good-b").2, title_of("good-b").3), ("An Author".to_string(), true));
        let (crash_id, crash_title, crash_author, crash_cover) = title_of("crash-d");
        assert_eq!((crash_title.as_str(), crash_author.as_str(), crash_cover), ("crash-d", "Unknown", false), "the poison file keeps its placeholders");
        let (_, plain_title, _, _) = title_of("plain-e");
        assert_eq!(plain_title, "plain-e", "a file with nothing to say keeps its filename title");

        // The fingerprints came out of the same reads.
        let store = cache.checksums();
        for name in ["good-a", "good-b", "good-c"] {
            let id = cache.book_for_file(&format!("shelf/{name}.pdf")).unwrap().unwrap();
            let (hash, _) = store.recorded_identity(id, "format", "PDF").unwrap();
            assert!(store.has_pdf_fingerprint(&hash.unwrap()).unwrap(), "{name} was fingerprinted by the enrichment read");
        }
        let (crash_hash, _) = store.recorded_identity(crash_id, "format", "PDF").unwrap();
        assert!(store.pdf_fingerprint_error(&crash_hash.unwrap()).unwrap().unwrap().starts_with("crashed:"));
        assert_eq!(inflight_rows(&cache), 0, "nothing is left marked");

        // Batched writes: the 5 books are one chunk, so one log entry beyond
        // the indexing batch -- not a file per field.
        assert_eq!(change_files(dir.path()), entries_after_indexing + 1);
        assert!(control.snapshot().timings_ms.contains_key("enrich_read") && control.snapshot().timings_ms.contains_key("enrich_write") && control.snapshot().timings_ms.contains_key("covers"));
        let before = snapshot(&cache);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before, "a rebuild from the change log reproduces the batched enrichment");
        assert!(cache.backend.change_log().unwrap().verify().unwrap().is_clean());
    }

    /// A run that died while PDFs were being read leaves them marked; they are
    /// suspects, read first and alone. The innocent one is enriched; the
    /// poison one crashes on its own retry and is recorded.
    #[test]
    fn books_left_in_flight_by_a_dead_run_are_suspects_retried_alone() {
        let (dir, cache) = library();
        write(&dir.path().join("good-a.pdf"), b"%PDF-1.4 a");
        write(&dir.path().join("crash-b.pdf"), b"%PDF-1.4 b");
        write(&dir.path().join("good-c.pdf"), b"%PDF-1.4 c");
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        let id = |name: &str| cache.book_for_file(name).unwrap().unwrap();
        let (a, b, c) = (id("good-a.pdf"), id("crash-b.pdf"), id("good-c.pdf"));
        cache.checksums().mark_inflight("enrich", &a.to_string()).unwrap();
        cache.checksums().mark_inflight("enrich", &b.to_string()).unwrap();

        let report = enrich_pending(&cache, &pool_options(2, 100), &JobControl::default()).unwrap();
        assert_eq!(report.remaining, 0);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report.failed[0].0.contains("crash-b") && report.failed[0].1.starts_with("crashed:"), "{report:?}");
        assert_eq!(cache.field_for(a, "title").unwrap().as_deref(), Some("Title of good-a"), "an innocent suspect is enriched");
        assert_eq!(cache.field_for(c, "title").unwrap().as_deref(), Some("Title of good-c"));
        assert_eq!(inflight_rows(&cache), 0);
    }

    /// Marked from its isolated retry: it killed the process alone, so it is
    /// recorded as crashed without another try.
    #[test]
    fn an_enrich_suspect_that_killed_the_process_on_its_retry_is_not_tried_again() {
        let (dir, cache) = library();
        write(&dir.path().join("good-a.pdf"), b"%PDF-1.4 a");
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        let a = cache.book_for_file("good-a.pdf").unwrap().unwrap();
        cache.checksums().mark_inflight("enrich-suspect", &a.to_string()).unwrap();
        let report = enrich_pending(&cache, &pool_options(1, 100), &JobControl::default()).unwrap();
        assert_eq!(report.remaining, 0);
        assert!(report.failed.len() == 1 && report.failed[0].1.starts_with("crashed:"), "{report:?}");
        assert_eq!(cache.field_for(a, "title").unwrap().as_deref(), Some("good-a"), "not read");
    }

    /// Without a worker, a PDF is read in-process, and it is marked in flight
    /// around the read all the same (a crash of this very process is what the
    /// mark exists for).
    #[test]
    fn in_process_pdf_reads_are_marked_and_cleared() {
        let (dir, cache) = library();
        write(&dir.path().join("junk.pdf"), b"%PDF-1.4 not really a pdf");
        index_files(&cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        let in_process = PipelineOptions { pdf_pool: crate::pdf_worker::PdfPoolChoice::InProcess, ..options(1, 100) };
        let report = enrich_pending(&cache, &in_process, &JobControl::default()).unwrap();
        assert_eq!((report.remaining, report.failed.len()), (0, 0), "{report:?}");
        assert_eq!(inflight_rows(&cache), 0);
    }

    /// The batched write must leave the library exactly as writing field by
    /// field did.
    #[test]
    fn batched_enrichment_equals_per_field_writes() {
        let html = |title: &str, author: &str| format!("<html><head><title>{title}</title><meta name=\"author\" content=\"{author}\"/></head><body>x</body></html>");
        let files = [("a1.html", html("Alpha", "Ann Author")), ("a2.html", html("Beta", "Bob & Cy")), ("a3.html", html("Gamma", "Ann Author")), ("a4.html", html("scan0004", "Unknown"))];

        let (dir_batched, batched) = library();
        let (dir_fields, fields) = library();
        for (dir, _) in [(&dir_batched, 0), (&dir_fields, 1)] {
            for (name, body) in &files {
                write(&dir.path().join(name), body.as_bytes());
            }
        }
        for (dir, cache) in [(&dir_batched, &batched), (&dir_fields, &fields)] {
            index_files(cache, dir.path(), &walked(dir.path()), &options(1, 100), &JobControl::default());
        }
        // The user edits one book between the passes: it must be left alone.
        for cache in [&batched, &fields] {
            let id = cache.book_for_file("a3.html").unwrap().unwrap();
            cache.set_field(id, "title", "Mine").unwrap();
        }

        // Batched: the pass itself, two books to a batch.
        enrich_pending(&batched, &options(1, 2), &JobControl::default()).unwrap();

        // Field by field: what the pass used to do.
        for (name, _) in &files {
            let id = fields.book_for_file(name).unwrap().unwrap();
            let stem = name.trim_end_matches(".html");
            let meta = lexmechanic_ebooks::metadata::get_metadata(&dir_fields.path().join(name)).unwrap();
            if let Some(title) = crate::junk_metadata::clean_title(&meta.title).filter(|t| t != "Unknown" && t != stem) {
                if fields.field_for(id, "title").unwrap().as_deref() == Some(stem) {
                    fields.set_field(id, "title", &title).unwrap();
                }
            }
            let authors: Vec<String> = crate::junk_metadata::clean_authors(&meta.authors).into_iter().filter(|a| a != "Unknown").collect();
            if !authors.is_empty() && fields.field_for(id, "authors").unwrap().as_deref().is_none_or(|a| a == "Unknown") {
                fields.set_field(id, "authors", &authors.join(" & ")).unwrap();
            }
            fields.checksums().finish_enrichment(id).unwrap();
        }

        let normalise = |lines: Vec<String>| -> Vec<String> {
            // uuids and paths differ between two libraries; compare the rest.
            let mut out: Vec<String> = lines.into_iter().map(|l| l.splitn(2, '|').nth(1).unwrap().to_string()).collect();
            out.sort();
            out
        };
        assert_eq!(normalise(snapshot(&batched)), normalise(snapshot(&fields)));
        assert_eq!(count(&batched, "SELECT COUNT(*) FROM checksums_db.enrichment_queue"), 0);

        let before = snapshot(&batched);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir_batched.path());
        assert_eq!(snapshot(&rebuilt), before);
    }

    /// A log compacted after a big scan, reopened from its snapshot and tail,
    /// still rebuilds the library -- including edits made after the reopen.
    #[test]
    fn a_compacted_and_reopened_log_still_rebuilds_the_library() {
        let (dir, cache) = library();
        fill(dir.path(), 40);
        index_files(&cache, dir.path(), &walked(dir.path()), &options(2, 10), &JobControl::default());
        enrich_pending(&cache, &PipelineOptions { pdf_pool: crate::pdf_worker::PdfPoolChoice::InProcess, ..options(2, 10) }, &JobControl::default()).unwrap();
        let id = cache.book_for_file("shelf1/book0001.pdf").unwrap().unwrap();
        cache.set_field(id, "title", "Before Compaction").unwrap();
        cache.set_field(id, "title", "Edited Twice").unwrap();

        let log = cache.backend.change_log().unwrap();
        let loose_before = log.entry_count();
        // Everything is past the horizon at this "now".
        let report = log.compact_if_needed(0, u64::MAX / 2).unwrap().expect("past the threshold");
        assert!(report.dropped >= 1, "the superseded title write is collapsed: {report:?}");
        assert_eq!(log.entry_count(), 0, "{loose_before} loose entries became a snapshot");
        drop(log);
        drop(cache);

        // Reopened: the tip comes from the snapshot header alone.
        let reopened = Cache::new(dir.path()).unwrap();
        reopened.set_field(id, "title", "After Reopen").unwrap();
        let log = reopened.backend.change_log().unwrap();
        assert!(log.verify().unwrap().is_clean(), "{:?}", log.verify().unwrap());

        let before = snapshot(&reopened);
        let (_rebuilt_dir, rebuilt) = rebuild_from_log_only(dir.path());
        assert_eq!(snapshot(&rebuilt), before);
        assert!(before.iter().any(|l| l.contains("After Reopen")));
    }

    /// Every phase of a scan reports where its time went.
    #[test]
    fn a_scan_reports_per_phase_timings() {
        let (dir, cache) = library();
        write(&dir.path().join("a.html"), b"<html><head><title>T</title></head><body>x</body></html>");
        let control = JobControl::default();
        let options = PipelineOptions { pdf_pool: crate::pdf_worker::PdfPoolChoice::InProcess, ..options(1, 10) };
        crate::scan::rescan_files(&cache, SystemTime::now() + Duration::from_secs(3600), &options, &control).unwrap();
        enrich_pending(&cache, &options, &control).unwrap();
        let timings = control.snapshot().timings_ms;
        for phase in ["walk", "drift", "index_hash", "index_write", "enrich_read", "enrich_write", "covers"] {
            assert!(timings.contains_key(phase), "no {phase} timing in {timings:?}");
        }
    }

    /// One open gives the content hash and the identity: the same answers the two calls it
    /// replaces gave (each of which opened the file).
    #[test]
    fn hashing_reads_the_identity_from_the_same_open_handle() {
        let (dir, _cache) = library();
        write(&dir.path().join("a.pdf"), b"%PDF identity");
        let path = dir.path().join("a.pdf");
        let control = JobControl::default();
        let (digest, identity) = hash_stream_with_identity(&path, Some(&control)).unwrap();
        assert_eq!(digest, hash_stream(&path, None).unwrap());
        assert!(identity.is_some());
        assert_eq!(identity, lexmechanic_utils::filenames::identity(&path));
        assert_eq!(control.file_opens.load(Ordering::Relaxed), 1);
    }
}
