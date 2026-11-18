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
//! - `POST /scan-library/{library_id}` -- scan, then enrich ([`crate::scan_library`])
//! - `POST /enrich-library/{library_id}` -- the metadata pass alone (resume)
//! - `POST /hash-backfill/{library_id}` -- hash every file with no hash
//! - `POST /pdf-fingerprint/{library_id}` -- page/text fingerprints of PDFs ([`calibre_db::pdf_similarity`])
//! - `POST /import-folder/{library_id}` -- copy a folder tree in ([`crate::import_folder`])
//! - `POST /duplicates/scan/{library_id}?job=true` -- the duplicate scan ([`crate::duplicates`])
//! - `GET /library-jobs`, `GET /library-jobs/{id}`, `POST /library-jobs/{id}/cancel`

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
}

impl JobState {
    fn name(self) -> &'static str {
        match self {
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
    pub control: Arc<JobControl>,
    outcome: Mutex<Outcome>,
    done: tokio::sync::watch::Sender<bool>,
}

impl LibraryJob {
    pub fn state(&self) -> JobState {
        self.outcome.lock().unwrap().state
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
        })
    }
}

struct Registry {
    next_id: AtomicU64,
    jobs: Mutex<Vec<Arc<LibraryJob>>>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry { next_id: AtomicU64::new(1), jobs: Mutex::new(Vec::new()) })
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
}

pub fn start_with(cache: Arc<Cache>, kind: JobKind, workers: Option<usize>, params: JobParams) -> Started {
    let library = cache.backend.library_path.to_string_lossy().to_string();
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

    let options = match workers {
        Some(n) => PipelineOptions::with_workers(n),
        None => PipelineOptions::default(),
    };
    let worker_job = job.clone();
    tokio::task::spawn_blocking(move || {
        // A panic in the pipeline must end the job, not leave it
        // "running" for ever.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&cache, kind, &options, &worker_job.control, &params)));
        let (state, report, error) = match result {
            Ok(Ok(report)) => {
                let state = if worker_job.control.is_cancelled() { JobState::Cancelled } else { JobState::Finished };
                (state, Some(report), None)
            }
            Ok(Err(e)) => (JobState::Failed, None, Some(format!("{e:#}"))),
            Err(_) => (JobState::Failed, None, Some("the job panicked".to_string())),
        };
        *worker_job.outcome.lock().unwrap() = Outcome { state, report, error, finished_at: Some(now_secs()) };
        let _ = worker_job.done.send(true);
    });
    Started::New(job)
}

fn run(cache: &Cache, kind: JobKind, options: &PipelineOptions, control: &JobControl, params: &JobParams) -> anyhow::Result<Value> {
    match kind {
        JobKind::Scan => {
            let rescan = calibre_db::scan::rescan_files(cache, SystemTime::now(), options, control)?;
            let mut report = scan_report(&rescan, control.is_cancelled());
            // The books exist now; fill in their metadata and covers as a
            // second stage, so the first is not held up by page renders.
            if !control.is_cancelled() {
                let enriched = calibre_db::scan_pipeline::enrich_pending(cache, options, control)?;
                report["enrichment"] = serde_json::to_value(enriched)?;
            }
            Ok(report)
        }
        JobKind::Enrich => Ok(serde_json::to_value(calibre_db::scan_pipeline::enrich_pending(cache, options, control)?)?),
        JobKind::HashBackfill => Ok(serde_json::to_value(calibre_db::scan_pipeline::backfill_hashes(cache, options, control)?)?),
        JobKind::PdfFingerprint => Ok(serde_json::to_value(calibre_db::pdf_similarity::fingerprint_pdfs(cache, options, control)?)?),
        JobKind::MergeExact => Ok(serde_json::to_value(calibre_db::merge::execute_content_merges(cache, &params.merge_plan, params.keep_duplicate_formats, control))?),
        JobKind::Dedupe => crate::duplicates::run_job(cache, params.dedupe_min.unwrap_or(calibre_db::dedupe::Strength::Fuzzy), control),
        JobKind::Import => {
            let (source, import_options) = params.import.as_ref().ok_or_else(|| anyhow::anyhow!("an import job needs a source folder"))?;
            Ok(serde_json::to_value(calibre_db::import::import_folder(cache, source, import_options, options, control)?)?)
        }
    }
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
    let (job, already_running) = match start(cache, kind, query.workers) {
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

/// `GET /library-jobs` -- newest first.
pub async fn list() -> Json<Value> {
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
