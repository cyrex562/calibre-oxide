//! Port of `simple.py`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use super::browser::OpenOptions;
use super::webengine_browser::WebEngineBrowser;
use crate::chardet::xml_to_unicode;

pub use super::browser::BrowserError;

/// Port of the module-level `overseers`/`storage`-list pattern: each
/// call site that wants a persistent, lazily-created
/// [`WebEngineBrowser`] owns one `BrowserCache` (upstream: `storage =
/// []` at whatever scope the caller chose). `cleanup_overseers`'s
/// weak-ref bookkeeping has no equivalent here -- Rust's ownership
/// model means a `BrowserCache` is dropped (and, if it were holding
/// anything needing explicit cleanup, cleaned up) exactly when its last
/// owner goes out of scope, with no manual "did every browser actually
/// get shut down" audit trail required.
pub struct BrowserCache {
    worker_binary: PathBuf,
    browser: Mutex<Option<WebEngineBrowser>>,
}

impl BrowserCache {
    /// `worker_binary` is the path to a compiled `calibre_scraper_worker`
    /// binary -- see [`WebEngineBrowser::new`]'s own doc.
    pub fn new(worker_binary: impl Into<PathBuf>) -> BrowserCache {
        BrowserCache { worker_binary: worker_binary.into(), browser: Mutex::new(None) }
    }
}

/// Port of `read_url(storage, url, timeout=60, as_html=False)`: fetch
/// `url` through `cache`'s lazily-created [`WebEngineBrowser`].
///
/// **Not the original response bytes.** Upstream's `WebEngineBrowser`
/// fetches via an injected `scraper.js`'s own `fetch()` call from
/// within the page's JS context, so `FakeResponse.read()` really is the
/// raw bytes the server sent. This port's `WebEngineBrowser` does a
/// real top-level navigation instead (see its own module doc for why),
/// so what comes back here is the loaded document's
/// `documentElement.outerHTML` -- a *rendered* serialization. For plain
/// HTML pages the two are close enough to be interchangeable for
/// scraping purposes (the whole point of `as_html=True`'s decode path);
/// for genuinely non-HTML content (binary files, bare plain text) the
/// browser will have wrapped it in a synthetic viewer document, so the
/// bytes returned are *not* a faithful copy of the original response.
pub fn read_url_bytes(cache: &BrowserCache, url: &str, timeout: Duration) -> Result<Vec<u8>, BrowserError> {
    let mut slot = cache.browser.lock().unwrap();
    if slot.is_none() {
        *slot = Some(WebEngineBrowser::new(cache.worker_binary.clone(), "", &[]));
    }
    let browser = slot.as_ref().unwrap();
    let opts = OpenOptions { timeout: Some(timeout), ..Default::default() };
    let resp = browser.open_novisit(url, &opts)?;
    Ok(resp.into_bytes())
}

/// Port of `read_url(storage, url, timeout=60, as_html=True)` (upstream's
/// default): fetch `url` and decode it with [`xml_to_unicode`]
/// (`strip_encoding_pats=True`, matching upstream's own call).
pub fn read_url(cache: &BrowserCache, url: &str, timeout: Duration) -> Result<String, BrowserError> {
    let raw = read_url_bytes(cache, url, timeout)?;
    Ok(xml_to_unicode(&raw, true, false).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    fn has_display() -> bool {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    fn worker_binary() -> Option<PathBuf> {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/calibre_scraper_worker").canonicalize().ok()?;
        candidate.exists().then_some(candidate)
    }

    fn start_server(body: &'static str, content_type: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok(mut stream) = listener.accept().map(|(s, _)| s) {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn read_url_bytes_fetches_the_rendered_document() {
        if !has_display() {
            eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY (run under xvfb-run)");
            return;
        }
        let Some(worker_binary) = worker_binary() else {
            eprintln!("skipping: calibre_scraper_worker binary not built");
            return;
        };
        // Plain text, not HTML: read_url_bytes's own doc discloses that
        // what comes back is the browser's *rendered* document, not a
        // faithful copy of the original bytes -- a browser wraps plain
        // text in a synthetic viewer document, so this only checks the
        // original content survives somewhere in there, not byte
        // equality.
        let url = start_server("hello", "text/plain");
        let cache = BrowserCache::new(worker_binary);
        let bytes = read_url_bytes(&cache, &url, Duration::from_secs(90)).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("hello"));
    }

    #[test]
    fn read_url_decodes_as_text() {
        if !has_display() {
            return;
        }
        let Some(worker_binary) = worker_binary() else {
            return;
        };
        let url = start_server("<html><body>hi</body></html>", "text/html; charset=utf-8");
        let cache = BrowserCache::new(worker_binary);
        let text = read_url(&cache, &url, Duration::from_secs(90)).unwrap();
        assert!(text.contains("hi"));
    }

    #[test]
    fn the_cache_holds_onto_its_lazily_created_browser_after_a_fetch() {
        if !has_display() {
            return;
        }
        let Some(worker_binary) = worker_binary() else {
            return;
        };
        let url = start_server("ok", "text/plain");
        let cache = BrowserCache::new(worker_binary);
        assert!(cache.browser.lock().unwrap().is_none());
        read_url_bytes(&cache, &url, Duration::from_secs(90)).unwrap();
        assert!(cache.browser.lock().unwrap().is_some());
    }
}
