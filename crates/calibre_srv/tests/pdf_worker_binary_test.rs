//! The real `calibre_pdf_worker` binary, driven through the real pool. (The
//! pool's crash/timeout handling is tested in `calibre_db` against a fake
//! worker; this proves the shipped binary speaks the same protocol.)

use std::ffi::OsString;
use std::time::Duration;

use calibre_db::pdf_worker::{PdfPoolChoice, Pool, PoolConfig, Reply, Request};

fn pool() -> Pool {
    Pool::new(PoolConfig { program: env!("CARGO_BIN_EXE_calibre_pdf_worker").into(), args: Vec::<OsString>::new(), envs: Vec::new(), workers: 2, timeout: Duration::from_secs(120) })
}

#[test]
fn the_worker_binary_reads_a_real_pdf_like_the_in_process_reader() {
    if !calibre_ebooks::pdf::rasterize::is_available() {
        eprintln!("skipped: PDFium is not installed");
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../calibre_ebooks/tests/data/two-page.pdf");
    let pool = pool();
    let Reply::Done(resp) = pool.run(Request { id: 0, path: path.to_string_lossy().to_string(), cover_width: Some(600), fingerprint: true }) else { panic!("the worker did not answer") };
    assert!(resp.error.is_none(), "{:?}", resp.error);
    assert_eq!(resp.fingerprint, Some(calibre_db::pdf_similarity::fingerprint_file(&path).unwrap()));
    assert!(resp.cover().is_some_and(|c| c.starts_with(&[0xFF, 0xD8])), "a JPEG cover");
}

#[test]
fn the_worker_binary_reports_an_unreadable_file_without_dying() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("junk.pdf");
    std::fs::write(&path, b"not a pdf at all").unwrap();
    let pool = pool();
    for _ in 0..2 {
        let Reply::Done(resp) = pool.run(Request { id: 0, path: path.to_string_lossy().to_string(), cover_width: Some(600), fingerprint: true }) else { panic!("the worker did not answer") };
        assert!(resp.error.is_some() || resp.unavailable, "{resp:?}");
    }
    assert_eq!(pool.started.load(std::sync::atomic::Ordering::Relaxed), 1, "the same worker served both");
    let _ = PdfPoolChoice::InProcess; // the choice type is public API too
}

/// `Auto` finds the worker next to the running executable or at
/// `CALIBRE_OXIDE_PDF_WORKER`.
#[test]
fn the_worker_is_located_through_the_environment_variable() {
    std::env::set_var(calibre_db::pdf_worker::WORKER_PATH_VAR, env!("CARGO_BIN_EXE_calibre_pdf_worker"));
    let located = calibre_db::pdf_worker::locate_worker().expect("found");
    assert!(located.is_file());
    std::env::remove_var(calibre_db::pdf_worker::WORKER_PATH_VAR);
}

/// A worker exits promptly when its parent goes away (stdin closes), instead of
/// sitting in a read for ever.
#[test]
fn the_worker_exits_when_its_stdin_closes() {
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_calibre_pdf_worker")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    drop(child.stdin.take());
    let started = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the worker did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}
