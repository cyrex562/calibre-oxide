//! Port of `simple.py`.

use std::sync::Mutex;
use std::time::Duration;

use super::browser::{BrowserError, OpenOptions, WebEngineBrowser};
use crate::chardet::xml_to_unicode;

/// Port of the module-level `overseers`/`storage`-list pattern: each
/// call site that wants a persistent, lazily-created
/// [`WebEngineBrowser`] owns one `BrowserCache` (upstream: `storage =
/// []` at whatever scope the caller chose). `cleanup_overseers`'s
/// weak-ref bookkeeping has no equivalent here -- Rust's ownership
/// model means a `BrowserCache` is dropped (and, if it were holding
/// anything needing explicit cleanup, cleaned up) exactly when its last
/// owner goes out of scope, with no manual "did every browser actually
/// get shut down" audit trail required.
pub struct BrowserCache(Mutex<Option<WebEngineBrowser>>);

impl BrowserCache {
    pub const fn new() -> BrowserCache {
        BrowserCache(Mutex::new(None))
    }
}

impl Default for BrowserCache {
    fn default() -> BrowserCache {
        BrowserCache::new()
    }
}

/// Port of `read_url(storage, url, timeout=60, as_html=False)`: fetch
/// `url` through `cache`'s lazily-created [`WebEngineBrowser`] and
/// return the raw response bytes.
pub fn read_url_bytes(cache: &BrowserCache, url: &str, timeout: Duration) -> Result<Vec<u8>, BrowserError> {
    let mut slot = cache.0.lock().unwrap();
    if slot.is_none() {
        *slot = Some(WebEngineBrowser::new("", &[], true));
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
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn read_url_bytes_fetches_the_raw_response() {
        let url = start_server("hello", "text/plain");
        let cache = BrowserCache::new();
        let bytes = read_url_bytes(&cache, &url, Duration::from_secs(5)).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn read_url_decodes_as_text() {
        let url = start_server("<html><body>hi</body></html>", "text/html; charset=utf-8");
        let cache = BrowserCache::new();
        let text = read_url(&cache, &url, Duration::from_secs(5)).unwrap();
        assert!(text.contains("hi"));
    }

    #[test]
    fn the_cache_holds_onto_its_lazily_created_browser_after_a_fetch() {
        let url = start_server("ok", "text/plain");
        let cache = BrowserCache::new();
        assert!(cache.0.lock().unwrap().is_none());
        read_url_bytes(&cache, &url, Duration::from_secs(5)).unwrap();
        assert!(cache.0.lock().unwrap().is_some());
    }
}
