//! Port of `old_src/src/calibre/srv/jobs.py` (issue #428): a
//! background job manager for server-side operations that shouldn't
//! block the HTTP request that triggered them (server-side
//! conversion, in-browser-reader rendering).
//!
//! # Scope
//!
//! Real: bounded concurrency (upstream's `max_jobs`, here a
//! [`tokio::sync::Semaphore`]), a query-able job-status map (waiting/
//! running/finished-with-result/finished-with-error/aborted, matching
//! upstream's own `job_status`'s four-way return), a maximum-job-time
//! auto-abort (`max_job_time`, via [`tokio::time::timeout`] racing the
//! job's own work), best-effort cancellation (upstream's
//! `abort_job`/`Event`-based `abort_event`, here [`JoinHandle::abort`]),
//! and lazy pruning of old finished jobs (upstream's own
//! `prune_finished_jobs`, run opportunistically rather than on a
//! dedicated sweep timer -- one less background task to manage).
//!
//! # Deliberately not a port of `fork_job`/subprocess isolation
//!
//! Upstream runs every job in a forked *subprocess*
//! (`calibre.utils.ipc.simple_worker.fork_job`, issue #68's own
//! `utils/ipc` gap, not ported), so a hanging or crashing job can be
//! killed/isolated outright regardless of what it's doing. This port
//! runs jobs as real `tokio` tasks in-process instead -- the natural
//! fit for a crate that's async/`tokio`-based throughout (`axum`
//! itself, `web_socket`'s own broadcast channel) rather than
//! hand-rolling process management this crate has no other use for.
//! The real, disclosed consequence: [`JoinHandle::abort`] only takes
//! effect at the job's own next `.await` point -- a job built from a
//! long synchronous, non-yielding computation can't actually be
//! interrupted early the way a killed subprocess could be. Real
//! future consumers of this module (`convert.py`'s server-side
//! conversion, the in-browser reader) should structure their work as
//! genuinely async (or `spawn_blocking` + accept it runs to
//! completion once started) with this in mind.
//!
//! A job's work returns `Result<String, String>` (a JSON-serializable
//! success payload, or an error message) rather than an arbitrary
//! Rust value -- matching the same "plain data crosses the boundary,
//! not a live object" contract upstream's own pickled-across-a-
//! process-boundary result has, just for a different reason (a shared
//! `HashMap` needs one concrete value type, not process-boundary
//! serialization).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

pub type JobId = u64;

/// Port of `job_status`'s four-way return, as a single real enum
/// instead of a `(state, result, traceback, was_aborted)` tuple.
#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    /// The job exists but hasn't started yet -- waiting for a free
    /// concurrency slot (upstream's `waiting_job_ids`).
    Waiting,
    /// The job's work is currently running.
    Running,
    /// The job ran to completion and returned a result.
    Finished { result: String },
    /// The job ran to completion and returned an error (or was
    /// aborted -- see `was_aborted`).
    Failed { error: String, was_aborted: bool },
    /// No job with this id is known (never existed, or its finished
    /// entry has since been pruned).
    Unknown,
}

struct JobEntry {
    /// Set once the job's work actually starts (after acquiring a
    /// concurrency permit) -- lets [`JobsManager::status`] tell
    /// "waiting" and "running" apart without the job itself reporting in.
    started: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

struct FinishedEntry {
    result: Option<String>,
    error: Option<String>,
    was_aborted: bool,
    finished_at: std::time::Instant,
}

struct Inner {
    running: HashMap<JobId, JobEntry>,
    finished: HashMap<JobId, FinishedEntry>,
}

/// Port of `JobsManager`.
pub struct JobsManager {
    semaphore: Arc<Semaphore>,
    max_job_time: Duration,
    /// Port of `prune_finished_jobs`'s hardcoded one-hour retention.
    finished_retention: Duration,
    next_id: AtomicU64,
    inner: Mutex<Inner>,
}

impl JobsManager {
    /// `max_jobs`: how many jobs may run concurrently (upstream falls
    /// back to `detect_ncpus()` for `max_jobs < 1`; callers here
    /// should do the equivalent, e.g. `std::thread::available_parallelism()`,
    /// themselves before calling this -- narrowed out of this
    /// constructor to keep it a pure function of its arguments).
    /// `max_job_time`: `Duration::ZERO` means "no timeout" (upstream's
    /// `opts.max_job_time <= 0` disables `abort_hanging_jobs`
    /// entirely).
    pub fn new(max_jobs: usize, max_job_time: Duration) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_jobs.max(1))),
            max_job_time,
            finished_retention: Duration::from_secs(3600),
            next_id: AtomicU64::new(0),
            inner: Mutex::new(Inner { running: HashMap::new(), finished: HashMap::new() }),
        }
    }

    /// Port of `start_job`: queues `work` to run as soon as a
    /// concurrency slot is free, returning its id immediately.
    pub async fn start_job<F, Fut>(self: &Arc<Self>, work: F) -> JobId
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<String, String>> + Send + 'static,
    {
        let job_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let started = Arc::new(AtomicBool::new(false));
        let this = self.clone();
        let semaphore = self.semaphore.clone();
        let max_job_time = self.max_job_time;
        let started_flag = started.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await;
            started_flag.store(true, Ordering::SeqCst);

            let outcome = if max_job_time.is_zero() {
                work().await
            } else {
                match tokio::time::timeout(max_job_time, work()).await {
                    Ok(r) => r,
                    Err(_) => Err("job exceeded its maximum allotted time".to_string()),
                }
            };

            let (result, error) = match outcome {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e)),
            };
            this.finish(job_id, result, error, false).await;
        });

        let mut inner = self.inner.lock().await;
        inner.running.insert(job_id, JobEntry { started, handle });
        job_id
    }

    async fn finish(&self, job_id: JobId, result: Option<String>, error: Option<String>, was_aborted: bool) {
        let mut inner = self.inner.lock().await;
        inner.running.remove(&job_id);
        inner.finished.insert(job_id, FinishedEntry { result, error, was_aborted, finished_at: std::time::Instant::now() });
        Self::prune_finished_locked(&mut inner, self.finished_retention);
    }

    fn prune_finished_locked(inner: &mut Inner, retention: Duration) {
        let now = std::time::Instant::now();
        inner.finished.retain(|_, job| now.duration_since(job.finished_at) <= retention);
    }

    /// Port of `job_status`.
    pub async fn status(&self, job_id: JobId) -> JobStatus {
        let mut inner = self.inner.lock().await;
        Self::prune_finished_locked(&mut inner, self.finished_retention);
        if let Some(job) = inner.finished.get(&job_id) {
            return match &job.result {
                Some(r) => JobStatus::Finished { result: r.clone() },
                None => JobStatus::Failed { error: job.error.clone().unwrap_or_default(), was_aborted: job.was_aborted },
            };
        }
        if let Some(job) = inner.running.get(&job_id) {
            return if job.started.load(Ordering::SeqCst) { JobStatus::Running } else { JobStatus::Waiting };
        }
        JobStatus::Unknown
    }

    /// Port of `abort_job`. Best-effort -- see the module doc for why
    /// this can't interrupt a non-yielding job early the way killing
    /// a subprocess could.
    pub async fn abort_job(&self, job_id: JobId) {
        let inner = self.inner.lock().await;
        if let Some(job) = inner.running.get(&job_id) {
            job.handle.abort();
        }
        drop(inner);
        // The aborted task's own `this.finish(...)` call never runs
        // (the task was killed, not allowed to finish normally), so
        // record the abort here instead -- matching upstream's own
        // `was_aborted` becoming true once the job observes its
        // `abort_event`.
        self.finish(job_id, None, Some("job was aborted".to_string()), true).await;
    }

    /// How many jobs are currently waiting or running (upstream has
    /// no direct equivalent -- `len(self.jobs) + len(waiting_job_ids)`
    /// -- exposed here as a small real convenience for a status/
    /// health endpoint).
    pub async fn active_count(&self) -> usize {
        self.inner.lock().await.running.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_job_starts_as_waiting_or_running_and_ends_finished() {
        let mgr = Arc::new(JobsManager::new(4, Duration::ZERO));
        let id = mgr.start_job(|| async { Ok("42".to_string()) }).await;

        // Give the spawned task a chance to actually run.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id).await, JobStatus::Finished { result: "42".to_string() });
    }

    #[tokio::test]
    async fn an_unknown_job_id_reports_unknown() {
        let mgr = Arc::new(JobsManager::new(4, Duration::ZERO));
        assert_eq!(mgr.status(999).await, JobStatus::Unknown);
    }

    #[tokio::test]
    async fn a_failing_job_reports_the_error() {
        let mgr = Arc::new(JobsManager::new(4, Duration::ZERO));
        let id = mgr.start_job(|| async { Err("boom".to_string()) }).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id).await, JobStatus::Failed { error: "boom".to_string(), was_aborted: false });
    }

    #[tokio::test]
    async fn a_job_can_be_observed_while_still_running() {
        let mgr = Arc::new(JobsManager::new(4, Duration::ZERO));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let id = mgr
            .start_job(|| async move {
                let _ = rx.await;
                Ok("done".to_string())
            })
            .await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id).await, JobStatus::Running);

        tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id).await, JobStatus::Finished { result: "done".to_string() });
    }

    #[tokio::test]
    async fn a_job_exceeding_its_time_limit_is_reported_as_a_timeout_failure() {
        let mgr = Arc::new(JobsManager::new(4, Duration::from_millis(10)));
        let id = mgr
            .start_job(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok("never".to_string())
            })
            .await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        match mgr.status(id).await {
            JobStatus::Failed { error, was_aborted: false } => assert!(error.contains("time"), "{error}"),
            other => panic!("expected a timeout failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn aborting_a_running_job_marks_it_aborted() {
        let mgr = Arc::new(JobsManager::new(4, Duration::ZERO));
        let id = mgr
            .start_job(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok("never".to_string())
            })
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        mgr.abort_job(id).await;
        assert_eq!(mgr.status(id).await, JobStatus::Failed { error: "job was aborted".to_string(), was_aborted: true });
    }

    #[tokio::test]
    async fn concurrency_is_bounded_by_max_jobs() {
        let mgr = Arc::new(JobsManager::new(1, Duration::ZERO));
        let (tx1, rx1) = tokio::sync::oneshot::channel::<()>();
        let id1 = mgr
            .start_job(|| async move {
                let _ = rx1.await;
                Ok("first".to_string())
            })
            .await;
        let id2 = mgr.start_job(|| async { Ok("second".to_string()) }).await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id1).await, JobStatus::Running, "the first job should have the only slot");
        assert_eq!(mgr.status(id2).await, JobStatus::Waiting, "the second job should still be waiting on it");

        tx1.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(mgr.status(id1).await, JobStatus::Finished { result: "first".to_string() });
        assert_eq!(mgr.status(id2).await, JobStatus::Finished { result: "second".to_string() }, "freeing the slot should let the second job run");
    }
}
