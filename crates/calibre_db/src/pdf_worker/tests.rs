//! The pool and the protocol, against a worker that is this very test binary
//! re-run with a filter: it needs no PDFium and no separately built binary, and
//! it can be told to crash or hang. The real worker binary (`calibre_pdf_worker`
//! in `calibre_srv`) runs the same [`serve`] with the real [`handle`].

use super::*;
use crate::pdf_similarity::PageHash;

const CHILD_VAR: &str = "CALIBRE_PDF_WORKER_TEST_CHILD";

/// A made-up fingerprint, for workers that pretend to have read a PDF.
pub(crate) fn canned_fingerprint() -> PdfFingerprint {
    PdfFingerprint { page_count: 3, pages: vec![PageHash { index: 0, bits: [1, 2, 3, 4], mask: [u64::MAX; 4], blank: false }], text_chars: 10, has_text: true, scanned: false, minhash: Vec::new() }
}

/// Run as a child process by [`fake_pool`], this is the worker. Under a normal
/// `cargo test` it does nothing.
///
/// What it does with a file depends on the file's name: `crash` aborts the
/// process, `hang` never answers, `good` answers with a canned fingerprint,
/// title and cover; anything else is read for real by [`handle`].
#[test]
fn child_entry() {
    if std::env::var(CHILD_VAR).is_err() {
        return;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(stdin.lock(), stdout.lock(), |req| {
        let name = Path::new(&req.path).file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if name.contains("crash") {
            std::process::abort();
        }
        if name.contains("hang") {
            std::thread::sleep(Duration::from_secs(3600));
        }
        if name.contains("good") {
            return Response {
                id: req.id,
                info: Some(Info { title: Some(format!("Title of {}", name.trim_end_matches(".pdf"))), author: Some("An Author".to_string()), keywords: None, subject: None }),
                cover_b64: req.cover_width.map(|_| base64::engine::general_purpose::STANDARD.encode(b"fake cover")),
                fingerprint: req.fingerprint.then(canned_fingerprint),
                ..Default::default()
            };
        }
        handle(req)
    })
    .unwrap();
}

/// A pool whose workers are [`child_entry`].
pub(crate) fn fake_pool(workers: usize, timeout: Duration) -> Pool {
    Pool::new(PoolConfig {
        program: std::env::current_exe().unwrap(),
        args: ["--exact", "pdf_worker::tests::child_entry", "--nocapture", "--test-threads=1"].iter().map(OsString::from).collect(),
        envs: vec![(OsString::from(CHILD_VAR), OsString::from("1"))],
        workers,
        timeout,
    })
}

fn request(path: &str) -> Request {
    Request { id: 0, path: path.to_string(), cover_width: Some(600), fingerprint: true }
}

#[test]
fn a_worker_answers_requests_in_order_and_is_reused() {
    let pool = fake_pool(1, Duration::from_secs(60));
    for i in 0..3 {
        let Reply::Done(resp) = pool.run(request(&format!("/x/good{i}.pdf"))) else { panic!("no answer") };
        assert_eq!(resp.info.as_ref().unwrap().title.as_deref(), Some(format!("Title of good{i}").as_str()));
        assert_eq!(resp.cover().as_deref(), Some(&b"fake cover"[..]));
        assert_eq!(resp.fingerprint, Some(canned_fingerprint()));
    }
    assert_eq!(pool.started.load(Ordering::Relaxed), 1, "one process served all three");
}

#[test]
fn a_crashing_file_is_reported_and_the_next_one_gets_a_fresh_worker() {
    let pool = fake_pool(1, Duration::from_secs(60));
    assert!(matches!(pool.run(request("/x/good-a.pdf")), Reply::Done(_)));
    let crashed = pool.run(request("/x/crash.pdf"));
    let Reply::Crashed(why) = &crashed else { panic!("expected a crash, got {crashed:?}") };
    assert!(!why.is_empty());
    assert!(crashed.failure_reason().unwrap().starts_with("crashed:"));
    assert!(matches!(pool.run(request("/x/good-b.pdf")), Reply::Done(_)), "the run continues");
    assert_eq!(pool.started.load(Ordering::Relaxed), 2, "the dead worker was replaced");
}

#[test]
fn a_hanging_file_times_out_and_the_worker_is_killed() {
    let pool = fake_pool(1, Duration::from_millis(4000));
    let started = Instant::now();
    let hung = pool.run(request("/x/hang.pdf"));
    assert!(matches!(hung, Reply::TimedOut), "{hung:?}");
    assert!(hung.failure_reason().unwrap().starts_with("timed out:"));
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(matches!(pool.run(request("/x/good.pdf")), Reply::Done(_)), "a new worker takes over");
}

#[test]
fn several_workers_run_at_once_and_a_crash_costs_only_its_own_file() {
    let pool = fake_pool(3, Duration::from_secs(60));
    let names: Vec<String> = (0..9).map(|i| if i == 4 { "/x/crash.pdf".to_string() } else { format!("/x/good{i}.pdf") }).collect();
    let replies: Vec<Reply> = std::thread::scope(|scope| {
        let handles: Vec<_> = names.iter().map(|n| scope.spawn(|| pool.run(request(n)))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, reply) in replies.iter().enumerate() {
        if i == 4 {
            assert!(matches!(reply, Reply::Crashed(_)), "{reply:?}");
        } else {
            assert!(matches!(reply, Reply::Done(_)), "file {i}: {reply:?}");
        }
    }
}

#[test]
fn a_worker_that_cannot_start_is_reported_as_such() {
    let pool = Pool::new(PoolConfig { program: PathBuf::from("/definitely/not/a/worker"), args: Vec::new(), envs: Vec::new(), workers: 2, timeout: Duration::from_secs(5) });
    assert!(matches!(pool.run(request("/x/good.pdf")), Reply::SpawnFailed(_)));
    // And a failed start does not leak a slot: asking again still fails fast.
    assert!(matches!(pool.run(request("/x/good.pdf")), Reply::SpawnFailed(_)));
}

#[test]
fn serve_skips_junk_lines_and_answers_each_request() {
    let input = b"\nnot json\n{\"id\":7,\"path\":\"/x/a.pdf\"}\n";
    let mut out = Vec::new();
    serve(&input[..], &mut out, |req| Response { id: req.id, error: Some(req.path.clone()), ..Default::default() }).unwrap();
    let text = String::from_utf8(out).unwrap();
    let answers: Vec<Response> = text.lines().filter(|l| l.starts_with('{')).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(answers.len(), 1);
    assert_eq!((answers[0].id, answers[0].error.as_deref()), (7, Some("/x/a.pdf")));
}

/// The real thing, when PDFium is installed: the pool, the protocol and the
/// real [`handle`] agree with reading the file in this process.
#[test]
fn the_worker_agrees_with_the_in_process_reader_on_a_real_pdf() {
    if !calibre_ebooks::pdf::rasterize::is_available() {
        eprintln!("skipped: PDFium is not installed");
        return;
    }
    const TWO_PAGE: &[u8] = include_bytes!("../../../calibre_ebooks/tests/data/two-page.pdf");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-page.pdf");
    std::fs::write(&path, TWO_PAGE).unwrap();

    // `child_entry` falls through to the real `handle` for this name.
    let pool = fake_pool(1, Duration::from_secs(120));
    let Reply::Done(resp) = pool.run(Request { id: 0, path: path.to_string_lossy().to_string(), cover_width: Some(calibre_ebooks::pdf::rasterize::scan_cover_width()), fingerprint: true }) else { panic!("no answer") };
    assert!(resp.error.is_none(), "{:?}", resp.error);

    assert_eq!(resp.fingerprint.as_ref(), Some(&crate::pdf_similarity::fingerprint_file(&path).unwrap()), "same fingerprint as in-process");
    let (info, cover) = calibre_ebooks::pdf::rasterize::info_and_cover_from_path_at(&path, Some(calibre_ebooks::pdf::rasterize::scan_cover_width())).unwrap();
    assert_eq!(resp.info, Some(Info { title: info.title, author: info.author, keywords: info.keywords, subject: info.subject }));
    assert_eq!(resp.cover(), cover, "same cover bytes");
    assert!(resp.cover().is_some_and(|c| c.starts_with(&[0xFF, 0xD8])), "a JPEG");
}
