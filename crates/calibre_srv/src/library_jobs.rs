//! Background jobs over one library: the folder scan, the metadata pass
//! that follows it, and the content-hash backfill.
//!
//! # Why not [`crate::jobs::JobsManager`]
//!
//! That manager was built for conversions and reader renders, and three of
//! its properties are wrong for these:
//!
//! - it kills a job after `max_job_time` (an hour, as `AppState` builds
//!   it), and a scan of a NAS with hundreds of thousands of files is not
//!   an hour;
//! - cancelling is `JoinHandle::abort`, which only lands at an `.await` --
//!   a scan is synchronous disk and database work, so it would report
//!   "aborted" while carrying on;
//! - a job there is a closure returning a string, with nowhere to publish
//!   progress.
//!
//! So these jobs get their own small registry: one `spawn_blocking` task
//! each, a [`JobControl`] it counts on and polls for cancellation, and a
//! final JSON report. It is process-wide rather than a field of
//! `AppState` because a job belongs to a library on disk, not to a
//! request, and there are forty places that build an `AppState` in tests.
//!
//! # One job per library
//!
//! Two scans (or a scan and a backfill) writing the same library at once
//! gain nothing: each already runs flat out against one writer. Starting a
//! job while another of the same kind is active returns the running one;
//! a different kind is refused with 409 and the id of what is running.
//!
//! # Routes
//!
//! - `POST /scan-library/{library_id}` -- scan, then enrich ([`crate::scan_library`]);
//!   `?enrich=false` stops after the scan and leaves the metadata and covers
//!   queued for `/enrich-library`
//! - `POST /enrich-library/{library_id}` -- the metadata pass alone (resume)
//! - `POST /hash-backfill/{library_id}` -- hash every file with no hash
//! - `POST /pdf-fingerprint/{library_id}` -- page/text fingerprints of PDFs ([`calibre_db::pdf_similarity`])
//! - `POST /prepare-library/{library_id}` -- the whole chain for duplicate
//!   detection: scan, hash backfill, PDF fingerprints, duplicate scan, with
//!   the stage persisted so a rerun resumes (`?restart=true` starts over);
//!   `GET /prepare-library/{library_id}` says which stage it stands at
//! - `POST /import-folder/{library_id}` -- copy a folder tree in ([`crate::import_folder`])
//! - `GET /absorbable-libraries/{library_id}`, `POST /absorb-library/{library_id}`,
//!   `POST /unabsorb-library/{library_id}` -- bring a calibre library nested in
//!   this one in, in place, and undo it ([`crate::absorb_library`])
//! - `POST /cdb/copy-to-library/{target}/{source}?job=true` -- the cross-library
//!   copy as a job ([`crate::cdb`])
//! - `POST /duplicates/scan/{library_id}?job=true` -- the duplicate scan ([`crate::duplicates`])
//! - `GET /library-jobs`, `GET /library-jobs/{id}`, `POST /library-jobs/{id}/cancel`
//!
//! # Surviving a restart
//!
//! The last state of each job (kind, summarized params, state, report,
//! timestamps) is written to `<library>/.calibre-oxide/jobs.json` on every
//! state change. A job recorded as `running` by a process that is gone is read
//! back as `interrupted` ([`restore`]); for the kinds that resume by
//! themselves (`resume_route`) the status JSON says `resumable`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_db::scan_pipeline::{JobControl, PipelineOptions};

use crate::errors::ServerError;
use crate::AppState;

/// Finished jobs kept for status queries.
const KEEP_FINISHED: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Scan,
    Enrich,
    HashBackfill,
    /// Fingerprint every PDF that has a hash and no fingerprint
    /// (`POST /pdf-fingerprint`), for the scan-vs-OCR and re-save matching.
    PdfFingerprint,
    /// Merge exact-content duplicate groups (a plan the user confirmed).
    MergeExact,
    /// Copy a folder tree into the library (`POST /import-folder`).
    Import,
    /// The duplicate scan (`POST /duplicates/scan/{id}?job=true`).
    Dedupe,
    /// A bulk duplicate resolve (`POST /duplicates/resolve`, executing).
    Resolve,
    /// Scan, hash backfill, PDF fingerprints, duplicate scan, in that order
    /// (`POST /prepare-library`).
    Prepare,
    /// Register the books of a calibre library nested in this one, in place
    /// (`POST /absorb-library`).
    Absorb,
    /// Undo an absorb (`POST /unabsorb-library`).
    Unabsorb,
    /// Copy or move books from another library into this one
    /// (`POST /cdb/copy-to-library/...?job=true`).
    Copy,
}

impl JobKind {
    pub fn name(self) -> &'static str {
        match self {
            JobKind::Scan => "scan",
            JobKind::Enrich => "enrich",
            JobKind::HashBackfill => "hash_backfill",
            JobKind::PdfFingerprint => "pdf_fingerprint",
            JobKind::MergeExact => "merge_exact",
            JobKind::Import => "import",
            JobKind::Dedupe => "dedupe",
            JobKind::Resolve => "resolve",
            JobKind::Prepare => "prepare",
            JobKind::Absorb => "absorb",
            JobKind::Unabsorb => "unabsorb",
            JobKind::Copy => "copy",
        }
    }

    pub fn from_name(name: &str) -> Option<JobKind> {
        [
            JobKind::Scan,
            JobKind::Enrich,
            JobKind::HashBackfill,
            JobKind::PdfFingerprint,
            JobKind::MergeExact,
            JobKind::Import,
            JobKind::Dedupe,
            JobKind::Resolve,
            JobKind::Prepare,
            JobKind::Absorb,
            JobKind::Unabsorb,
            JobKind::Copy,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }

    /// The `POST /<route>/{library_id}` that continues this kind of job after
    /// it was interrupted. Only the jobs that resume from what the database
    /// says is left have one: they need no parameters to pick up where they
    /// stopped. (An absorb resumes too, but needs its folder -- see `params`.)
    pub fn resume_route(self) -> Option<&'static str> {
        match self {
            JobKind::Scan => Some("scan-library"),
            JobKind::Enrich => Some("enrich-library"),
            JobKind::HashBackfill => Some("hash-backfill"),
            JobKind::PdfFingerprint => Some("pdf-fingerprint"),
            JobKind::Prepare => Some("prepare-library"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Finished,
    /// Stopped on request. What was done is kept; a rerun continues.
    Cancelled,
    Failed,
    /// Was running when the server process ended (read back from
    /// `.calibre-oxide/jobs.json`). What it did is kept; for a resumable kind
    /// a rerun continues.
    Interrupted,
}

impl JobState {
    fn name(self) -> &'static str {
        match self {
            JobState::Interrupted => "interrupted",
            JobState::Running => "running",
            JobState::Finished => "finished",
            JobState::Cancelled => "cancelled",
            JobState::Failed => "failed",
        }
    }
}

struct Outcome {
    state: JobState,
    report: Option<Value>,
    error: Option<String>,
    finished_at: Option<u64>,
}

pub struct LibraryJob {
    pub id: u64,
    pub kind: JobKind,
    /// The library folder, which is what "one job per library" keys on.
    library: String,
    started_at: u64,
    /// The small, serializable part of the job's parameters.
    params: Value,
    pub control: Arc<JobControl>,
    outcome: Mutex<Outcome>,
    done: tokio::sync::watch::Sender<bool>,
}

impl LibraryJob {
    pub fn state(&self) -> JobState {
        self.outcome.lock().unwrap().state
    }

    /// The final report, once the job has ended with one.
    pub fn report(&self) -> Option<Value> {
        self.outcome.lock().unwrap().report.clone()
    }

    /// Resolves once the job has finished, however it ended.
    pub async fn wait(&self) {
        let mut rx = self.done.subscribe();
        let _ = rx.wait_for(|finished| *finished).await;
    }

    /// The job as the status routes show it. `report` is present only
    /// once the job has ended.
    pub fn to_json(&self) -> Value {
        let outcome = self.outcome.lock().unwrap();
        json!({
            "id": self.id,
            "kind": self.kind.name(),
            "library": self.library,
            "state": outcome.state.name(),
            "progress": self.control.snapshot(),
            "report": outcome.report,
            "error": outcome.error,
            "started_at": self.started_at,
            "finished_at": outcome.finished_at,
            "params": self.params,
            // Offered for a job that was cut off: POST to
            // `/<resume_route>/{library_id}` continues it.
            "resumable": outcome.state == JobState::Interrupted && self.kind.resume_route().is_some(),
            "resume_route": self.kind.resume_route(),
        })
    }

    /// The record written to `jobs.json`: the job's state as of now.
    fn record(&self) -> Value {
        let outcome = self.outcome.lock().unwrap();
        // A report can be large (a scan lists every failed file); the file is
        // rewritten on each state change, so cap what it keeps.
        let report = outcome.report.as_ref().filter(|r| r.to_string().len() <= MAX_PERSISTED_REPORT);
        json!({
            "id": self.id,
            "kind": self.kind.name(),
            "library": self.library,
            "state": outcome.state.name(),
            "report": report,
            "report_truncated": outcome.report.is_some() && report.is_none(),
            "error": outcome.error,
            "started_at": self.started_at,
            "finished_at": outcome.finished_at,
            "params": self.params,
        })
    }
}

/// Reports larger than this (serialized) are not kept in `jobs.json`.
const MAX_PERSISTED_REPORT: usize = 256 * 1024;

struct Registry {
    next_id: AtomicU64,
    jobs: Mutex<Vec<Arc<LibraryJob>>>,
    /// Libraries whose `jobs.json` has been read back already.
    restored: Mutex<std::collections::HashSet<String>>,
    /// Serializes writes of `jobs.json`.
    persist_lock: Mutex<()>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry { next_id: AtomicU64::new(1), jobs: Mutex::new(Vec::new()), restored: Mutex::new(Default::default()), persist_lock: Mutex::new(()) })
}

/// `<library>/.calibre-oxide/jobs.json`: the last state of this library's
/// jobs (kind, parameters, state, report, timestamps), rewritten on every
/// state change.
pub fn jobs_file(library: &str) -> std::path::PathBuf {
    std::path::Path::new(library).join(calibre_db::constants::LIBRARY_HANDLE_DIR_NAME).join("jobs.json")
}

/// Writes every job of `library` that the registry holds to its `jobs.json`
/// (temp file then rename, so a crash mid-write leaves the old file).
fn persist(library: &str) {
    let reg = registry();
    let _guard = reg.persist_lock.lock().unwrap();
    let records: Vec<Value> = reg.jobs.lock().unwrap().iter().filter(|j| j.library == library).map(|j| j.record()).collect();
    let path = jobs_file(library);
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("json.partial");
        std::fs::write(&temp, serde_json::to_vec_pretty(&json!({"version": 1, "jobs": records}))?)?;
        std::fs::rename(&temp, &path)
    };
    if let Err(e) = write() {
        eprintln!("could not record the library jobs in {}: {e}", path.display());
    }
}

/// Reads `library`'s `jobs.json` back into the registry, once per process. A
/// job recorded as `running` belongs to a process that no longer exists, so it
/// comes back `interrupted` (and the file is rewritten to say so). Restored
/// jobs get new ids -- the old ones may collide with this process's -- and
/// keep the old one as `params.previous_id`.
pub fn restore(library: &str) {
    let reg = registry();
    if !reg.restored.lock().unwrap().insert(library.to_string()) {
        return;
    }
    let Some(doc) = std::fs::read_to_string(jobs_file(library)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
    let mut interrupted_any = false;
    let mut restored = Vec::new();
    for rec in doc["jobs"].as_array().into_iter().flatten() {
        let Some(kind) = rec["kind"].as_str().and_then(JobKind::from_name) else { continue };
        let state = match rec["state"].as_str() {
            Some("running") | Some("interrupted") => JobState::Interrupted,
            Some("finished") => JobState::Finished,
            Some("cancelled") => JobState::Cancelled,
            Some("failed") => JobState::Failed,
            _ => continue,
        };
        interrupted_any |= rec["state"].as_str() == Some("running");
        let mut params = if rec["params"].is_object() { rec["params"].clone() } else { json!({}) };
        params["previous_id"] = rec["id"].clone();
        let (done, _) = tokio::sync::watch::channel(true);
        restored.push(Arc::new(LibraryJob {
            id: reg.next_id.fetch_add(1, Ordering::SeqCst),
            kind,
            library: library.to_string(),
            started_at: rec["started_at"].as_u64().unwrap_or(0),
            params,
            control: JobControl::new(),
            outcome: Mutex::new(Outcome {
                state,
                report: rec["report"].as_object().map(|_| rec["report"].clone()),
                error: rec["error"].as_str().map(str::to_string),
                finished_at: rec["finished_at"].as_u64(),
            }),
            done,
        }));
    }
    if restored.is_empty() {
        return;
    }
    reg.jobs.lock().unwrap().extend(restored);
    if interrupted_any {
        persist(library);
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What starting a job produced.
pub enum Started {
    /// A new job is running.
    New(Arc<LibraryJob>),
    /// The same kind of job was already running on this library.
    AlreadyRunning(Arc<LibraryJob>),
    /// A different kind of job is running on this library.
    Busy(Arc<LibraryJob>),
}

/// Starts `kind` over `cache`'s library on a blocking thread.
pub fn start(cache: Arc<Cache>, kind: JobKind, workers: Option<usize>) -> Started {
    start_with(cache, kind, workers, JobParams::default())
}

/// What a job needs beyond its kind. Only the merge uses any.
#[derive(Default, Clone)]
pub struct JobParams {
    pub merge_plan: Vec<calibre_db::merge::PlannedGroup>,
    pub keep_duplicate_formats: bool,
    /// The folder to import and how, for [`JobKind::Import`].
    pub import: Option<(std::path::PathBuf, calibre_db::import::ImportOptions)>,
    /// The minimum strength of a duplicate scan ([`JobKind::Dedupe`]).
    pub dedupe_min: Option<calibre_db::dedupe::Strength>,
    /// The plan of a [`JobKind::Resolve`] job.
    pub resolve: Option<crate::duplicates_review::ResolveJob>,
    /// For [`JobKind::Scan`]: stop after indexing, leaving the metadata and
    /// covers queued for [`JobKind::Enrich`] (`?enrich=false`).
    pub defer_enrichment: bool,
    /// PDF worker processes for the jobs that read PDFs (`?pdf_workers=`);
    /// `0` reads in-process. `None`: the environment's setting or the default.
    pub pdf_workers: Option<usize>,
    /// Width of the covers rendered from PDFs (`?cover_width=`); `None`: 600.
    pub cover_width: Option<u32>,
    /// For [`JobKind::Prepare`]: begin again at the scan instead of resuming
    /// at the stage a previous run reached.
    pub restart: bool,
    /// The nested library folder and how to absorb it ([`JobKind::Absorb`]; for
    /// [`JobKind::Unabsorb`] only the folder).
    pub absorb: Option<(String, calibre_db::absorb::AbsorbOptions)>,
    /// The books to copy and where from ([`JobKind::Copy`]).
    pub copy: Option<CopyJob>,
}

/// A cross-library copy: the source library, its books and the options. The
/// destination is the library the job runs on.
#[derive(Clone)]
pub struct CopyJob {
    pub source: Arc<Cache>,
    pub book_ids: Vec<i32>,
    pub options: calibre_db::copy_to_library::CopyOptions,
}

impl JobParams {
    /// The parts of the parameters worth keeping in `jobs.json` (the plans and
    /// source caches are not serializable and not needed to describe a job).
    fn summary(&self) -> Value {
        let mut map = serde_json::Map::new();
        if self.restart {
            map.insert("restart".into(), json!(true));
        }
        if self.defer_enrichment {
            map.insert("defer_enrichment".into(), json!(true));
        }
        if let Some(n) = self.pdf_workers {
            map.insert("pdf_workers".into(), json!(n));
        }
        if let Some(w) = self.cover_width {
            map.insert("cover_width".into(), json!(w));
        }
        if let Some((folder, _)) = &self.import {
            map.insert("folder".into(), json!(folder.to_string_lossy()));
        }
        if let Some((folder, _)) = &self.absorb {
            map.insert("folder".into(), json!(folder));
        }
        if let Some(copy) = &self.copy {
            map.insert("book_count".into(), json!(copy.book_ids.len()));
        }
        Value::Object(map)
    }
}

pub fn start_with(cache: Arc<Cache>, kind: JobKind, workers: Option<usize>, params: JobParams) -> Started {
    let library = cache.backend.library_path.to_string_lossy().to_string();
    restore(&library);
    let reg = registry();
    let mut jobs = reg.jobs.lock().unwrap();

    if let Some(active) = jobs.iter().find(|j| j.library == library && j.state() == JobState::Running) {
        return if active.kind == kind { Started::AlreadyRunning(active.clone()) } else { Started::Busy(active.clone()) };
    }

    let (done, _) = tokio::sync::watch::channel(false);
    let job = Arc::new(LibraryJob {
        id: reg.next_id.fetch_add(1, Ordering::SeqCst),
        kind,
        library,
        started_at: now_secs(),
        params: params.summary(),
        control: JobControl::new(),
        outcome: Mutex::new(Outcome { state: JobState::Running, report: None, error: None, finished_at: None }),
        done,
    });
    jobs.push(job.clone());
    // Bounded history: drop the oldest finished jobs, never a running one.
    while jobs.iter().filter(|j| j.state() != JobState::Running).count() > KEEP_FINISHED {
        if let Some(pos) = jobs.iter().position(|j| j.state() != JobState::Running) {
            jobs.remove(pos);
        }
    }
    drop(jobs);
    persist(&job.library);

    let mut options = match workers {
        Some(n) => PipelineOptions::with_workers(n),
        None => PipelineOptions::default(),
    };
    options.pdf_workers = params.pdf_workers;
    options.cover_width = params.cover_width;
    let worker_job = job.clone();
    tokio::task::spawn_blocking(move || {
        // A panic in the pipeline must end the job, not leave it
        // "running" for ever.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&cache, kind, &options, &worker_job.control, &params)));
        let (state, report, error) = match result {
            Ok(Ok(mut report)) => {
                let state = if worker_job.control.is_cancelled() { JobState::Cancelled } else { JobState::Finished };
                // Where the time went, by phase, on every job's report.
                if let Some(map) = report.as_object_mut() {
                    map.insert("timings_ms".into(), json!(worker_job.control.snapshot().timings_ms));
                }
                (state, Some(report), None)
            }
            Ok(Err(e)) => (JobState::Failed, None, Some(format!("{e:#}"))),
            Err(_) => (JobState::Failed, None, Some("the job panicked".to_string())),
        };
        // Library jobs are what write the change log in bulk, so this is where
        // a log grown past its threshold is compacted (entries inside the
        // retention horizon are never touched; see `ChangeLog::compact`).
        compact_change_log(&cache);
        *worker_job.outcome.lock().unwrap() = Outcome { state, report, error, finished_at: Some(now_secs()) };
        persist(&worker_job.library);
        let _ = worker_job.done.send(true);
    });
    Started::New(job)
}

/// Loose change-log entries past which a finished job compacts the log:
/// `CALIBRE_OXIDE_CHANGE_LOG_COMPACT_AT`, else
/// [`calibre_db::change_log::DEFAULT_COMPACT_THRESHOLD`].
fn compact_threshold() -> usize {
    std::env::var("CALIBRE_OXIDE_CHANGE_LOG_COMPACT_AT").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(calibre_db::change_log::DEFAULT_COMPACT_THRESHOLD)
}

fn compact_change_log(cache: &Cache) {
    let Ok(log) = cache.backend.change_log() else { return };
    match log.compact_if_needed(compact_threshold(), now_secs() * 1000) {
        Ok(Some(report)) if report.dropped > 0 => eprintln!("compacted the change log: {} superseded entries dropped, {} kept", report.dropped, report.retained),
        Ok(_) => {}
        Err(e) => eprintln!("could not compact the change log: {e:#}"),
    }
}

fn run(cache: &Cache, kind: JobKind, options: &PipelineOptions, control: &JobControl, params: &JobParams) -> anyhow::Result<Value> {
    match kind {
        JobKind::Scan => {
            let rescan = calibre_db::scan::rescan_files(cache, SystemTime::now(), options, control)?;
            let mut report = scan_report(&rescan, control.is_cancelled());
            // The books exist now; fill in their metadata and covers as a
            // second stage, so the first is not held up by page renders.
            if params.defer_enrichment {
                report["enrichment_deferred"] = json!(true);
            } else if !control.is_cancelled() {
                let enriched = calibre_db::scan_pipeline::enrich_pending(cache, options, control)?;
                report["enrichment"] = serde_json::to_value(enriched)?;
            }
            Ok(report)
        }
        JobKind::Prepare => run_prepare(cache, options, control, params),
        JobKind::Enrich => Ok(serde_json::to_value(calibre_db::scan_pipeline::enrich_pending(cache, options, control)?)?),
        JobKind::HashBackfill => Ok(serde_json::to_value(calibre_db::scan_pipeline::backfill_hashes(cache, options, control)?)?),
        JobKind::PdfFingerprint => Ok(serde_json::to_value(calibre_db::pdf_similarity::fingerprint_pdfs(cache, options, control)?)?),
        JobKind::MergeExact => Ok(serde_json::to_value(calibre_db::merge::execute_content_merges(cache, &params.merge_plan, params.keep_duplicate_formats, control))?),
        JobKind::Dedupe => crate::duplicates::run_job(cache, params.dedupe_min.unwrap_or(calibre_db::dedupe::Strength::Fuzzy), control),
        JobKind::Resolve => {
            let job = params.resolve.as_ref().ok_or_else(|| anyhow::anyhow!("a resolve job needs a plan"))?;
            crate::duplicates_review::run_resolve_job(cache, job, control)
        }
        JobKind::Absorb => {
            let (folder, absorb_options) = params.absorb.as_ref().ok_or_else(|| anyhow::anyhow!("an absorb job needs a folder"))?;
            Ok(serde_json::to_value(calibre_db::absorb::absorb_library(cache, folder, absorb_options, options, control)?)?)
        }
        JobKind::Unabsorb => {
            let (folder, _) = params.absorb.as_ref().ok_or_else(|| anyhow::anyhow!("an unabsorb job needs a folder"))?;
            Ok(serde_json::to_value(calibre_db::absorb::unabsorb_library(cache, folder, control)?)?)
        }
        JobKind::Copy => {
            let job = params.copy.as_ref().ok_or_else(|| anyhow::anyhow!("a copy job needs its books"))?;
            Ok(serde_json::to_value(calibre_db::copy_to_library::copy_books(&job.source, cache, &job.book_ids, job.options, control)?)?)
        }
        JobKind::Import => {
            let (source, import_options) = params.import.as_ref().ok_or_else(|| anyhow::anyhow!("an import job needs a source folder"))?;
            Ok(serde_json::to_value(calibre_db::import::import_folder(cache, source, import_options, options, control)?)?)
        }
    }
}

/// The stages of [`JobKind::Prepare`], in order.
pub const PREPARE_STAGES: [&str; 4] = ["scan", "hash", "fingerprint", "dedupe"];

fn prepare_stage_path(cache: &Cache) -> std::path::PathBuf {
    cache.backend.library_path.join(calibre_db::constants::LIBRARY_HANDLE_DIR_NAME).join("prepare-stage.json")
}

/// The stage a prepare run would start at: where the last one stopped, or the
/// scan if it finished (or never ran).
pub fn prepare_stage(cache: &Cache) -> &'static str {
    let stage = std::fs::read_to_string(prepare_stage_path(cache)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v["stage"].as_str().map(str::to_string));
    PREPARE_STAGES.iter().copied().find(|s| Some(*s) == stage.as_deref()).unwrap_or(PREPARE_STAGES[0])
}

fn persist_prepare_stage(cache: &Cache, stage: &str) {
    let path = prepare_stage_path(cache);
    let text = json!({"stage": stage, "updated_at": now_secs()}).to_string();
    let temp = path.with_extension("json.partial");
    if std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, &path)).is_err() {
        eprintln!("could not record the prepare stage {stage}");
    }
}

/// Everything duplicate detection needs, as one job: **scan** (walk, index,
/// then metadata and covers), **hash** (content hashes for every file the scan
/// did not hash), **fingerprint** (PDF page/text fingerprints) and **dedupe**
/// (the duplicate scan). Each stage resumes by itself (its work list is what
/// the database says is left), and the stage reached is written to
/// `.calibre-oxide/prepare-stage.json` before it starts, so a rerun -- after a
/// cancel, a crash or a restart -- continues at that stage instead of
/// walking the library again. A stage that fails (PDFium missing for the
/// fingerprints, say) is reported and the chain goes on; a cancel stops it
/// where it is.
fn run_prepare(cache: &Cache, options: &PipelineOptions, control: &JobControl, params: &JobParams) -> anyhow::Result<Value> {
    let start = if params.restart { PREPARE_STAGES[0] } else { prepare_stage(cache) };
    let mut stages = serde_json::Map::new();
    let mut finished = true;
    for stage in PREPARE_STAGES.iter().copied().skip_while(|s| *s != start) {
        if control.is_cancelled() {
            finished = false;
            break;
        }
        persist_prepare_stage(cache, stage);
        let began = std::time::Instant::now();
        let outcome: anyhow::Result<Value> = match stage {
            "scan" => (|| {
                let rescan = calibre_db::scan::rescan_files(cache, SystemTime::now(), options, control)?;
                let mut report = scan_report(&rescan, control.is_cancelled());
                if params.defer_enrichment {
                    report["enrichment_deferred"] = json!(true);
                } else if !control.is_cancelled() {
                    report["enrichment"] = serde_json::to_value(calibre_db::scan_pipeline::enrich_pending(cache, options, control)?)?;
                }
                Ok(report)
            })(),
            "hash" => calibre_db::scan_pipeline::backfill_hashes(cache, options, control).map(|r| json!(r)),
            "fingerprint" => calibre_db::pdf_similarity::fingerprint_pdfs(cache, options, control).map(|r| json!(r)),
            _ => crate::duplicates::run_job(cache, calibre_db::dedupe::Strength::Fuzzy, control),
        };
        control.add_time(&format!("prepare_{stage}"), began.elapsed());
        match outcome {
            Ok(report) => {
                stages.insert(stage.to_string(), report);
            }
            Err(e) => {
                stages.insert(stage.to_string(), json!({"error": format!("{e:#}")}));
            }
        }
        if control.is_cancelled() {
            finished = false;
            break;
        }
    }
    if finished {
        // Done: the next run starts over at the scan.
        persist_prepare_stage(cache, "done");
    }
    Ok(json!({"started_at_stage": start, "stages": stages, "finished": finished}))
}

/// The scan's final report, in the shape `POST /scan-library` has always
/// returned (plus `cancelled`).
fn scan_report(rescan: &calibre_db::scan::Rescan, cancelled: bool) -> Value {
    json!({
        "added": rescan.indexed.added,
        "already_known": rescan.indexed.already_known,
        // Books whose recorded path was corrected. Reported so a
        // refresh can say what it did, but not as a problem: moving
        // a file inside your own folder is allowed.
        "relocated": rescan.relocated,
        // Skipped because the user removed the book and kept the
        // file (#896). Surfaced rather than omitted -- a scan that
        // silently refuses to add a file the user can see is
        // indistinguishable from a broken one.
        "ignored": rescan.indexed.ignored,
        // Still being written. Not an error: the next scan gets them.
        "settling": rescan.scanned.settling,
        // Cloud-sync placeholders. Reading one would download it.
        "offline": rescan.scanned.offline,
        "nested_libraries": rescan.scanned.nested_libraries,
        "failed": rescan.indexed.failed.iter().map(|(path, why)| json!({"path": path, "error": why})).collect::<Vec<_>>(),
        // False means a directory could not be read, so the folder
        // was only partly seen -- or the job was cancelled part-way. What
        // was added is still correct; what is absent is unknown. The UI
        // has to say so rather than implying the library is now in sync.
        "conclusive": rescan.scanned.is_complete() && !cancelled,
        "unreadable": rescan.scanned.unreadable,
        "cancelled": cancelled,
    })
}

/// Shared by the three `POST` routes: start, then either answer at once
/// with the job's id or (`?wait=true`) hold the response until it ends.
pub async fn respond(cache: Arc<Cache>, kind: JobKind, query: &StartQuery) -> Response {
    let params = JobParams { restart: query.restart == Some(true), defer_enrichment: query.enrich == Some(false), pdf_workers: query.pdf_workers, cover_width: query.cover_width.map(|w| w.clamp(100, 4000)), ..Default::default() };
    let (job, already_running) = match start_with(cache, kind, query.workers, params) {
        Started::New(job) => (job, false),
        Started::AlreadyRunning(job) => (job, true),
        Started::Busy(active) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": format!("a {} job is already running on this library", active.kind.name()), "job_id": active.id, "kind": active.kind.name()})),
            )
                .into_response();
        }
    };

    if query.wait.unwrap_or(false) {
        job.wait().await;
        // The report on its own, as before; the job's own state alongside.
        let mut body = job.outcome.lock().unwrap().report.clone().unwrap_or_else(|| json!({}));
        let (state, error) = {
            let outcome = job.outcome.lock().unwrap();
            (outcome.state.name(), outcome.error.clone())
        };
        if let Some(map) = body.as_object_mut() {
            map.insert("job_id".into(), json!(job.id));
            map.insert("state".into(), json!(state));
        }
        if let Some(error) = error {
            return ServerError::InternalServerError(error).into_response();
        }
        return Json(body).into_response();
    }

    (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": job.kind.name(), "state": job.state().name(), "already_running": already_running, "status_url": format!("/library-jobs/{}", job.id)}))).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct StartQuery {
    /// Hold the response until the job ends, and answer with its report.
    pub wait: Option<bool>,
    /// Concurrent readers; defaults to a NAS-friendly 3
    /// (`CALIBRE_OXIDE_SCAN_WORKERS` overrides the default).
    pub workers: Option<usize>,
    /// `false` on the scan route: index the files and stop, leaving metadata
    /// and covers queued (`POST /enrich-library` does them later).
    pub enrich: Option<bool>,
    /// PDF worker processes (default: half the cores, at most 4;
    /// `CALIBRE_OXIDE_PDF_WORKERS` sets the default). `0` reads PDFs in the
    /// server process, one at a time.
    pub pdf_workers: Option<usize>,
    /// Width in pixels of covers rendered from PDFs (default 600).
    pub cover_width: Option<u32>,
    /// On the prepare route: start over at the scan instead of resuming.
    pub restart: Option<bool>,
}

fn library_cache(state: &AppState, library_id: &str) -> Result<Arc<Cache>, ServerError> {
    state.cache_for(Some(library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))
}

/// `POST /enrich-library/{library_id}` -- the metadata and cover pass
/// over books the scan registered from their filenames. The scan runs it
/// itself; this resumes it after a cancel or a restart.
pub async fn enrich(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    Ok(respond(library_cache(&state, &library_id)?, JobKind::Enrich, &query).await)
}

/// `POST /hash-backfill/{library_id}` -- gives every format file with no
/// content hash one, so de-duplication sees the whole library.
pub async fn hash_backfill(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    Ok(respond(library_cache(&state, &library_id)?, JobKind::HashBackfill, &query).await)
}

/// `POST /pdf-fingerprint/{library_id}` -- reads each PDF with a content hash
/// and no fingerprint once (page hashes and text), so the duplicate scan can
/// tell a scan from its OCR and a PDF from its optimized copy. Cancellable and
/// resumable; PDFs it cannot read are recorded and reported, not retried. Fails
/// (and records nothing against any PDF) if PDFium is not installed.
pub async fn pdf_fingerprint(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    Ok(respond(library_cache(&state, &library_id)?, JobKind::PdfFingerprint, &query).await)
}

/// `POST /prepare-library/{library_id}` -- scan, hash, fingerprint and
/// duplicate-scan in one resumable job (see [`run_prepare`]). Takes the scan's
/// `enrich`, `workers`, `pdf_workers` and `cover_width` parameters, and
/// `restart=true` to ignore the stage a previous run reached.
pub async fn prepare(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<StartQuery>) -> Result<Response, ServerError> {
    Ok(respond(library_cache(&state, &library_id)?, JobKind::Prepare, &query).await)
}

/// `GET /prepare-library/{library_id}` -- the stage the next prepare run would
/// start at, and the stages in order.
pub async fn prepare_status(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = library_cache(&state, &library_id)?;
    Ok(Json(json!({"stage": prepare_stage(&cache), "stages": PREPARE_STAGES})))
}

/// `GET /library-jobs` -- newest first.
pub async fn list(State(state): State<AppState>) -> Json<Value> {
    // Bring back what an earlier process left in this library's jobs.json, so
    // a job it was running shows as interrupted.
    restore(&state.cache.backend.library_path.to_string_lossy());
    let mut jobs: Vec<Arc<LibraryJob>> = registry().jobs.lock().unwrap().clone();
    jobs.sort_by_key(|j| std::cmp::Reverse(j.id));
    Json(json!({"jobs": jobs.iter().map(|j| j.to_json()).collect::<Vec<_>>()}))
}

fn find(id: u64) -> Result<Arc<LibraryJob>, ServerError> {
    registry().jobs.lock().unwrap().iter().find(|j| j.id == id).cloned().ok_or_else(|| ServerError::NotFound(format!("no job {id}")))
}

/// `GET /library-jobs/{job_id}`.
pub async fn status(Path(job_id): Path<u64>) -> Result<Json<Value>, ServerError> {
    Ok(Json(find(job_id)?.to_json()))
}

/// `POST /library-jobs/{job_id}/cancel` -- cooperative: the job stops at
/// its next read chunk and reports what it did. Cancelling a job that has
/// ended is a no-op.
pub async fn cancel(Path(job_id): Path<u64>) -> Result<Json<Value>, ServerError> {
    let job = find(job_id)?;
    if job.state() == JobState::Running {
        job.control.cancel();
    }
    Ok(Json(job.to_json()))
}
