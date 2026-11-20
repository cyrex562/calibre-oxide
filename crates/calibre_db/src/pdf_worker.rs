//! PDFium in a separate process.
//!
//! # Why
//!
//! PDFium is native C++ code reading files nobody vetted: scans, corrupt
//! downloads, files cut off by a full disk. It crashes (access violations,
//! stack overflows) on some of them, and when it did so in the server's own
//! process it took the whole server down. Tranche 11 made that survivable by
//! reading one PDF at a time behind one lock, which also made a library of a
//! hundred thousand PDFs take days. Reading PDFs in child processes fixes both:
//!
//! - a crash kills one worker. The pool records the file as `crashed`, starts
//!   another worker and goes on;
//! - a hang is cut off by a per-file timeout and recorded as `timed out`;
//! - the workers are separate processes with separate PDFium instances, so
//!   they really do run in parallel.
//!
//! # The protocol
//!
//! The worker (`calibre_pdf_worker`, built with `calibre_srv`) reads one JSON
//! [`Request`] per line on stdin and answers one JSON [`Response`] per line on
//! stdout, in order, one at a time. It ends when stdin closes. Anything on
//! stdout that is not a JSON object (a library printing a banner) is ignored by
//! the pool, and every response starts on a fresh line for that reason.
//!
//! One request can ask for the Info strings, a cover at a given width, and the
//! page/text fingerprint of [`crate::pdf_similarity`], and the file is opened
//! **once** for all of them.
//!
//! # Using it
//!
//! [`Pool`] runs requests on up to N workers, started lazily. The jobs that
//! read PDFs ([`crate::pdf_similarity::fingerprint_pdfs`] and the scan's
//! enrichment, [`crate::scan_pipeline::enrich_pending`]) use one when a worker
//! binary can be found (see [`locate_worker`]) and otherwise fall back to
//! reading in-process, as before.
//!
//! # Configuration
//!
//! - `CALIBRE_OXIDE_PDF_WORKER`: path of the worker binary. Otherwise it is
//!   looked for next to the running executable.
//! - `CALIBRE_OXIDE_PDF_WORKERS`: how many workers. `0` turns the pool off.
//!   Default: half the cores, at most 4, at least 1.
//! - `CALIBRE_OXIDE_PDF_TIMEOUT_SECS`: per-file limit. Default 120.

use std::ffi::OsString;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::pdf_similarity::PdfFingerprint;

/// File name of the worker binary, without the platform's extension.
pub const WORKER_NAME: &str = "calibre_pdf_worker";
pub const WORKER_PATH_VAR: &str = "CALIBRE_OXIDE_PDF_WORKER";
pub const WORKERS_VAR: &str = "CALIBRE_OXIDE_PDF_WORKERS";
pub const TIMEOUT_VAR: &str = "CALIBRE_OXIDE_PDF_TIMEOUT_SECS";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Most workers started by default.
pub const MAX_DEFAULT_WORKERS: usize = 4;

/// What the worker is asked to do with one file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub path: String,
    /// Render page 1 as a JPEG this many pixels wide.
    #[serde(default)]
    pub cover_width: Option<u32>,
    /// Compute the page/text fingerprint.
    #[serde(default)]
    pub fingerprint: bool,
}

/// The Info strings of a PDF.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub keywords: Option<String>,
    pub subject: Option<String>,
}

impl From<Info> for calibre_ebooks::pdf::rasterize::PdfInfo {
    fn from(i: Info) -> Self {
        calibre_ebooks::pdf::rasterize::PdfInfo { title: i.title, author: i.author, keywords: i.keywords, subject: i.subject }
    }
}

/// The worker's answer to one [`Request`].
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    /// The file could not be read at all (PDFium said no, or is missing).
    #[serde(default)]
    pub error: Option<String>,
    /// `error` is "PDFium is not installed", which is not the file's fault.
    #[serde(default)]
    pub unavailable: bool,
    #[serde(default)]
    pub info: Option<Info>,
    /// The cover JPEG, base64.
    #[serde(default)]
    pub cover_b64: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<PdfFingerprint>,
    /// A fingerprint was asked for and could not be made (the file opened).
    #[serde(default)]
    pub fingerprint_error: Option<String>,
}

impl Response {
    pub fn cover(&self) -> Option<Vec<u8>> {
        self.cover_b64.as_ref().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
    }
}

/// Reads the file as asked, in this process. The worker's whole job, and what
/// the tests compare the pool against.
pub fn handle(req: &Request) -> Response {
    use calibre_ebooks::pdf::rasterize::{read_pdf_from_path, RasterizeError};
    let mut out = Response { id: req.id, ..Default::default() };
    let sample: Option<(u32, Box<dyn FnOnce(usize) -> (Vec<usize>, Vec<usize>)>)> =
        req.fingerprint.then(|| (crate::pdf_similarity::RENDER_LONG_SIDE, Box::new(|n| (crate::pdf_similarity::sample_indices(n), crate::pdf_similarity::text_page_indices(n))) as Box<dyn FnOnce(usize) -> (Vec<usize>, Vec<usize>)>));
    match read_pdf_from_path(Path::new(&req.path), req.cover_width, sample) {
        Err(e) => {
            out.unavailable = matches!(e, RasterizeError::Unavailable { .. });
            out.error = Some(e.to_string());
        }
        Ok(read) => {
            out.info = Some(Info { title: read.info.title, author: read.info.author, keywords: read.info.keywords, subject: read.info.subject });
            out.cover_b64 = read.cover.map(|c| base64::engine::general_purpose::STANDARD.encode(c));
            if let Some(sampled) = read.sampled {
                match crate::pdf_similarity::fingerprint_sampled(&sampled) {
                    Ok(fp) => out.fingerprint = Some(fp),
                    Err(e) => out.fingerprint_error = Some(format!("{e:#}")),
                }
            }
        }
    }
    out
}

/// The worker's main loop: a request per line in, a response per line out,
/// until `input` ends.
pub fn serve<R: BufRead, W: Write>(input: R, mut output: W, handler: impl Fn(&Request) -> Response) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Request>(&line) else { continue };
        let response = handler(&request);
        let text = serde_json::to_string(&response).map_err(std::io::Error::other)?;
        // A leading newline so a stray unterminated print cannot glue itself
        // to the front of the JSON.
        output.write_all(b"\n")?;
        output.write_all(text.as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Finding and configuring the worker
// ---------------------------------------------------------------------

/// The worker binary: `CALIBRE_OXIDE_PDF_WORKER` if it names a file, else
/// `calibre_pdf_worker[.exe]` in the directory of the running executable (where
/// `calibre_srv` and the worker are built and packaged together).
pub fn locate_worker() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(WORKER_PATH_VAR).map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(format!("{WORKER_NAME}{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// Half the cores, at most [`MAX_DEFAULT_WORKERS`], at least one.
pub fn default_worker_count() -> usize {
    (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).clamp(1, MAX_DEFAULT_WORKERS)
}

/// How a pool starts and limits its workers.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub envs: Vec<(OsString, OsString)>,
    pub workers: usize,
    pub timeout: Duration,
}

impl PoolConfig {
    /// The real worker, if there is one and the environment has not turned
    /// the pool off (`CALIBRE_OXIDE_PDF_WORKERS=0`). `workers` overrides the
    /// count; `Some(0)` also turns it off.
    pub fn discover(workers: Option<usize>) -> Option<PoolConfig> {
        let count = workers.or_else(|| std::env::var(WORKERS_VAR).ok().and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or_else(default_worker_count);
        if count == 0 {
            return None;
        }
        let timeout = std::env::var(TIMEOUT_VAR).ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|s| *s > 0).map(Duration::from_secs).unwrap_or(DEFAULT_TIMEOUT);
        Some(PoolConfig { program: locate_worker()?, args: Vec::new(), envs: Vec::new(), workers: count.min(32), timeout })
    }
}

// ---------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------

/// What came of one request.
#[derive(Debug)]
pub enum Reply {
    /// The worker answered (the answer may itself say the file is unreadable).
    Done(Response),
    /// The worker died while reading the file; `why` says how.
    Crashed(String),
    /// The worker did not answer in time and was killed.
    TimedOut,
    /// No worker could be started. Nothing is the file's fault.
    SpawnFailed(String),
}

impl Reply {
    /// The reason to record against the file for the two outcomes that are its
    /// fault, `None` otherwise.
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            Reply::Crashed(why) => Some(format!("crashed: the PDF reader process died while reading this file ({why})")),
            Reply::TimedOut => Some("timed out: the PDF reader gave up on this file".to_string()),
            _ => None,
        }
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct PoolState {
    idle: Vec<Worker>,
    /// Workers idle plus workers in use.
    live: usize,
}

/// A set of worker processes. Cheap to share by reference; `run` may be called
/// from many threads at once and each call uses one worker.
pub struct Pool {
    config: PoolConfig,
    state: Mutex<PoolState>,
    freed: Condvar,
    next_id: AtomicU64,
    /// Workers started so far (a restart after a crash counts).
    pub started: AtomicU64,
}

impl Pool {
    pub fn new(config: PoolConfig) -> Pool {
        Pool { config, state: Mutex::new(PoolState::default()), freed: Condvar::new(), next_id: AtomicU64::new(1), started: AtomicU64::new(0) }
    }

    pub fn workers(&self) -> usize {
        self.config.workers.max(1)
    }

    pub fn timeout(&self) -> Duration {
        self.config.timeout
    }

    fn spawn(&self) -> Result<Worker, String> {
        let mut command = Command::new(&self.config.program);
        command.args(&self.config.args).envs(self.config.envs.iter().map(|(k, v)| (k, v))).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| format!("could not start {}: {e}", self.config.program.display()))?;
        let stdin = child.stdin.take().ok_or("worker has no stdin")?;
        let mut stdout = child.stdout.take().ok_or("worker has no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Bytes, not `lines()`: a worker that prints something that is not
            // UTF-8 must not end the reader.
            let mut pending: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&chunk[..n]);
                        while let Some(at) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=at).collect();
                            if tx.send(String::from_utf8_lossy(&line).trim().to_string()).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        self.started.fetch_add(1, Ordering::Relaxed);
        Ok(Worker { child, stdin, lines: rx })
    }

    fn acquire(&self) -> Result<Worker, String> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(worker) = state.idle.pop() {
                return Ok(worker);
            }
            if state.live < self.workers() {
                state.live += 1;
                drop(state);
                return match self.spawn() {
                    Ok(worker) => Ok(worker),
                    Err(e) => {
                        self.release(None);
                        Err(e)
                    }
                };
            }
            state = self.freed.wait(state).unwrap();
        }
    }

    /// Hands a worker back (`Some`), or reports that one is gone (`None`).
    fn release(&self, worker: Option<Worker>) {
        let mut state = self.state.lock().unwrap();
        match worker {
            Some(w) => state.idle.push(w),
            None => state.live = state.live.saturating_sub(1),
        }
        self.freed.notify_one();
    }

    /// Sends `request` to a worker and waits for its answer, at most the
    /// configured timeout. A worker that dies or hangs is discarded; the next
    /// call starts a fresh one.
    pub fn run(&self, mut request: Request) -> Reply {
        request.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut worker = match self.acquire() {
            Ok(w) => w,
            Err(e) => return Reply::SpawnFailed(e),
        };
        let line = serde_json::to_string(&request).expect("a request serialises");
        if let Err(e) = worker.stdin.write_all(line.as_bytes()).and_then(|_| worker.stdin.write_all(b"\n")).and_then(|_| worker.stdin.flush()) {
            let why = exit_description(&mut worker.child, &format!("could not send the request: {e}"));
            drop(worker);
            self.release(None);
            return Reply::Crashed(why);
        }
        let deadline = Instant::now() + self.config.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match worker.lines.recv_timeout(remaining) {
                Ok(text) => {
                    if !text.starts_with('{') {
                        continue;
                    }
                    match serde_json::from_str::<Response>(&text) {
                        Ok(response) if response.id == request.id => {
                            self.release(Some(worker));
                            return Reply::Done(response);
                        }
                        _ => continue,
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drop(worker); // kills it
                    self.release(None);
                    return Reply::TimedOut;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let why = exit_description(&mut worker.child, "the worker closed its output");
                    drop(worker);
                    self.release(None);
                    return Reply::Crashed(why);
                }
            }
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Dropping the idle workers kills them.
        self.state.lock().unwrap().idle.clear();
    }
}

fn exit_description(child: &mut Child, fallback: &str) -> String {
    // The pipe closing and the process being reapable are not the same instant.
    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(status)) => return format!("worker {status}"),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    fallback.to_string()
}

/// Which reader a job uses for PDFs.
#[derive(Clone, Default)]
pub enum PdfPoolChoice {
    /// A pool of the worker found next to the executable, if any; otherwise
    /// the in-process reader.
    #[default]
    Auto,
    /// Always the in-process reader.
    InProcess,
    /// This pool (tests, and callers that share one across jobs).
    Pool(std::sync::Arc<Pool>),
}

impl PdfPoolChoice {
    /// The pool to use for one job, or `None` for the in-process reader.
    /// `workers` is the caller's override of the worker count.
    pub fn resolve(&self, workers: Option<usize>) -> Option<std::sync::Arc<Pool>> {
        match self {
            PdfPoolChoice::InProcess => None,
            PdfPoolChoice::Pool(pool) => Some(pool.clone()),
            PdfPoolChoice::Auto => PoolConfig::discover(workers).map(|c| std::sync::Arc::new(Pool::new(c))),
        }
    }
}

impl std::fmt::Debug for PdfPoolChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfPoolChoice::Auto => write!(f, "Auto"),
            PdfPoolChoice::InProcess => write!(f, "InProcess"),
            PdfPoolChoice::Pool(p) => write!(f, "Pool({} workers)", p.workers()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
