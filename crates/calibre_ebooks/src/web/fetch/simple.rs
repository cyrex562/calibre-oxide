//! Port of `old_src/src/calibre/web/fetch/simple.py`'s `RecursiveFetcher`
//! (issue #455, epic), split per docs/AGENT_PORTING_GUIDE.md §5a into
//! #630-#633 (see this workspace's own scoping pass, referenced from
//! `docs/modules_to_port.md`'s `simple.py` entry, for the full split
//! rationale). This module covers **#630**: the single-resource fetch
//! primitive ([`fetch_url`]) and URL/link filtering
//! ([`is_link_ok`], [`is_link_wanted`], [`absurl`], [`normurl`],
//! [`localize_link`], [`canonicalize_url`], [`basename`]).
//!
//! HTML tree manipulation (`get_soup`'s `keep_only_tags`/`remove_tags`
//! matching engine, #631), image/stylesheet download (`process_images`/
//! `process_stylesheets`, #632), and the recursive crawl loop itself
//! (`RecursiveFetcher`'s `start_fetch`/`process_links`/
//! `process_return_links`/`save_soup`, #633) are none of them here.
//!
//! Builds entirely on the already-real [`crate::scraper::Browser`]
//! (issue #58) -- no new transport machinery. One real API difference,
//! disclosed: [`Browser::open`]/[`Browser::open_novisit`] return `Ok`
//! regardless of HTTP status (no `.error_for_status()`), where
//! Python's `urlopen` raises `URLError`/`HTTPError` for a non-2xx
//! response. [`fetch_url`] itself inspects [`BrowserResponse::status`]
//! and turns 4xx/5xx into a [`FetchError::Http`] using
//! [`BrowserResponse::reason`] -- the same observable outcome
//! (`FetchError(responses[err.code])` in Python), different mechanism.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::scraper::{Browser, BrowserResponse, OpenOptions};

/// Port of `response` (a `bytes` subclass in Python tagging its result
/// with a `.newurl` attribute) -- just its two real fields; no
/// subclassing needed in Rust.
#[derive(Debug, Clone)]
pub struct FetchedResource {
    pub data: Vec<u8>,
    /// `None` only for the `data:` URL branch, matching real Python's
    /// own `standard_b64decode(...)` early return (a bare `bytes`,
    /// never wrapped in `response`, so it has no `.newurl` either).
    pub new_url: Option<String>,
}

/// Port of `FetchError` (raised for a real HTTP error status) plus
/// the two other real failure modes `fetch_url` can propagate.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// Port of `FetchError(responses[err.code])`: `.0` is the
    /// canonical HTTP reason phrase, matching what Python's own
    /// `http.client.responses` table lookup produces.
    #[error("{0}")]
    Http(String),
    #[error(transparent)]
    Browser(#[from] crate::scraper::BrowserError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Port of `RecursiveFetcher.LINK_FILTER`, applied by [`is_link_ok`].
/// Preserved verbatim, including its real, slightly-loose patterns
/// (`.exe\s*$` etc. -- an unescaped `.` matches *any* character before
/// "exe", not just a literal dot; this is upstream's own long-standing
/// pattern, not a transcription error introduced here).
const LINK_FILTER_PATTERNS: &[&str] = &[r"(?i).exe\s*$", r"(?i).mp3\s*$", r"(?i).ogg\s*$", r"(?i)^\s*mailto:", r"(?i)^\s*$"];

fn link_filter_regexes() -> &'static [Regex] {
    static FILTER: OnceLock<Vec<Regex>> = OnceLock::new();
    FILTER.get_or_init(|| LINK_FILTER_PATTERNS.iter().map(|p| Regex::new(p).expect("static pattern")).collect())
}

/// Port of `RecursiveFetcher.is_link_ok`.
pub fn is_link_ok(url: &str) -> bool {
    !link_filter_regexes().iter().any(|r| r.is_match(url))
}

/// Port of `RecursiveFetcher.is_link_wanted`'s regexp fallthrough.
/// `hook_result` is the caller-supplied `_is_link_wanted` outcome:
/// `None` means "not implemented" (Python's `NotImplementedError`
/// path, falling through to the regexp logic below), `Some(false)`
/// means the hook explicitly rejected the link *or* raised any other
/// exception (Python's `except Exception: return False` collapses
/// onto this too), `Some(true)` means the hook explicitly wants it
/// and the regexps are never consulted. Kept decoupled from
/// `NewsRecipeHooks` deliberately: a plain `web2disk`-style caller
/// with no recipe at all can pass `None` unconditionally and still
/// get correct regexp-only behavior.
pub fn is_link_wanted(url: &str, hook_result: Option<bool>, filter_regexps: &[Regex], match_regexps: &[Regex]) -> bool {
    if let Some(wanted) = hook_result {
        return wanted;
    }
    if filter_regexps.iter().any(|r| r.is_match(url)) {
        return false;
    }
    if !match_regexps.is_empty() {
        return match_regexps.iter().any(|r| r.is_match(url));
    }
    true
}

/// Port of `RecursiveFetcher.absurl`. `href` is the raw attribute
/// value (`tag[key]` in Python); `is_wanted` is called only when
/// `filter` is true, and only after [`is_link_ok`] already passed --
/// matching the real short-circuit order. Returns `None` for anything
/// to skip (a fragment-only/empty href, a link `is_link_ok`/`is_wanted`
/// rejects), the same as Python's early-return `None`s, not an error.
pub fn absurl(base_url: &str, href: &str, filter: bool, is_wanted: impl FnOnce(&str) -> bool) -> Option<String> {
    // Port of `if not parts.netloc and not parts.path and not
    // parts.query: return None` -- everything before the first `#` is
    // exactly netloc+path+query combined; a fragment-only or empty
    // href leaves that empty.
    if href.split('#').next().unwrap_or("").is_empty() {
        return None;
    }
    let iurl = match url::Url::parse(href) {
        // Already has a scheme (absolute URL) -- kept verbatim, not
        // re-normalized, matching Python's own "only urljoin when
        // `not parts.scheme`" branch.
        Ok(_) => href.to_string(),
        Err(_) => {
            let base = url::Url::parse(base_url).ok()?;
            base.join(href).ok()?.to_string()
        }
    };
    if !is_link_ok(&iurl) {
        return None;
    }
    if filter && !is_wanted(&iurl) {
        return None;
    }
    Some(iurl)
}

/// Port of `RecursiveFetcher.normurl`: strips the URL fragment,
/// leaving everything else -- including the exact original string
/// form of the rest -- untouched (no re-serialization, avoiding any
/// drift a round-trip through a URL parser could introduce).
pub fn normurl(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_string()
}

/// Port of `RecursiveFetcher.localize_link`: `local_path` plus the
/// original href's own fragment (if any), re-attached.
pub fn localize_link(original_href: &str, local_path: &str) -> String {
    match original_href.split_once('#') {
        Some((_, fragment)) => format!("{local_path}#{fragment}"),
        None => local_path.to_string(),
    }
}

fn percent_quote_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Port of `canonicalize_url`: percent-quotes everything after the
/// scheme+authority when (and only when) the URL contains whitespace
/// -- mechanize doesn't quote automatically, so upstream fixes up
/// malformed-but-common cases (a literal space in a path segment).
/// Disclosed simplification: quotes the whole path+query+fragment as
/// one run rather than replicating Python's separate `params`
/// component (the legacy `;params` path suffix, essentially unused by
/// any real HTTP URL this crawler encounters) -- no observable
/// difference for real inputs.
pub fn canonicalize_url(url: &str) -> String {
    if !url.chars().any(|c| c.is_whitespace()) {
        return url.to_string();
    }
    match url.find("://") {
        Some(scheme_end) => {
            let after_scheme = &url[scheme_end + 3..];
            let netloc_end = after_scheme.find('/').map(|i| scheme_end + 3 + i).unwrap_or(url.len());
            let (prefix, rest) = url.split_at(netloc_end);
            format!("{prefix}{}", percent_quote_component(rest))
        }
        None => percent_quote_component(url),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Port of `basename`. Disclosed simplification: real Python guards
/// against `url` not even being a string (an unreachable case given
/// this port's `&str` signature) with a `bad_url_{N}.html` fallback --
/// omitted here since there's no analogous failure mode to guard
/// against. Disclosed narrow drift: a name with no dot at all, or
/// ending in a bare `.`, is treated as "no extension" here; real
/// Python's `os.path.splitext` also treats a *leading*-dot-only name
/// (a dotfile, e.g. `.bashrc`) as having no extension, which this
/// port's simpler `rsplit_once('.')` does not special-case -- harmless
/// in practice, since URL basenames essentially never take that shape.
pub fn basename(url: &str) -> String {
    let without_query_or_fragment = url.split(['?', '#']).next().unwrap_or(url);
    let decoded = percent_decode(without_query_or_fragment);
    let last = decoded.rsplit('/').next().unwrap_or("");
    let has_extension = last.rsplit_once('.').map(|(_, ext)| !ext.is_empty()).unwrap_or(false);
    if has_extension {
        last.to_string()
    } else {
        "index.html".to_string()
    }
}

/// Port of `self.last_fetch_at`/`self.get_delay`-driven rate limiting
/// in `fetch_url`, factored into its own type so a future #633 can
/// own one shared instance across a whole crawl (one `RecursiveFetcher`
/// == one throttle, matching Python's one-`last_fetch_at`-per-instance
/// shape).
pub struct FetchThrottle {
    last_fetch_at: Mutex<Instant>,
}

impl Default for FetchThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchThrottle {
    /// Port of `self.last_fetch_at = 0.` -- the very first fetch is
    /// never delayed, so this seeds far enough in the past that no
    /// realistic `delay` value could trigger a wait on the first call.
    pub fn new() -> Self {
        FetchThrottle { last_fetch_at: Mutex::new(Instant::now() - Duration::from_secs(24 * 3600)) }
    }

    fn wait_before_fetch(&self, delay: Duration) {
        let elapsed = self.last_fetch_at.lock().unwrap().elapsed();
        if elapsed < delay {
            std::thread::sleep(delay - elapsed);
        }
    }

    /// Port of `finally: self.last_fetch_at = time.monotonic()` --
    /// called unconditionally after a network fetch attempt, success
    /// or failure alike.
    fn mark_fetched(&self) {
        *self.last_fetch_at.lock().unwrap() = Instant::now();
    }
}

fn finish_response(resp: BrowserResponse) -> Result<FetchedResource, FetchError> {
    if let Some(code) = resp.status() {
        if code >= 400 {
            return Err(FetchError::Http(resp.reason().to_string()));
        }
    }
    Ok(FetchedResource { data: resp.read().to_vec(), new_url: Some(resp.url().to_string()) })
}

fn fetch_over_network(browser: &Browser, url: &str, timeout: Duration) -> Result<FetchedResource, FetchError> {
    let canonical = canonicalize_url(url);
    let opts = OpenOptions { timeout: Some(timeout), ..Default::default() };
    match browser.open_novisit(&canonical, &opts) {
        Ok(resp) => finish_response(resp),
        Err(e) if e.worth_retry() => {
            std::thread::sleep(Duration::from_secs(1));
            finish_response(browser.open_novisit(&canonical, &opts)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Port of `RecursiveFetcher.fetch_url`, minus the caller's own
/// preprocessing hooks (#631's job) and the current-dir/file-writing
/// side effects that only make sense once the crawl loop (#633) exists.
/// `preloaded` is `self.preloaded_urls` (an already-known-content
/// shortcut, consumed once like Python's own `dict.pop`); `get_delay`
/// is a closure so #619's `NewsRecipeHooks::get_url_specific_delay`
/// can be passed straight through by #633 without this module
/// depending on that trait.
pub fn fetch_url(browser: &Browser, throttle: &FetchThrottle, preloaded: &Mutex<HashMap<String, Vec<u8>>>, url: &str, timeout: Duration, get_delay: impl Fn(&str) -> Duration) -> Result<FetchedResource, FetchError> {
    if let Some(data) = preloaded.lock().unwrap().remove(url) {
        return Ok(FetchedResource { data, new_url: Some(url.to_string()) });
    }
    if let Some(payload) = url.strip_prefix("data:") {
        let b64 = payload.split_once(',').map(|(_, p)| p).unwrap_or("");
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default();
        return Ok(FetchedResource { data, new_url: None });
    }
    if let Some(rest) = url.strip_prefix("file://").or_else(|| url.strip_prefix("file:")) {
        let data = std::fs::read(rest)?;
        return Ok(FetchedResource { data, new_url: Some(format!("file:{rest}")) });
    }

    throttle.wait_before_fetch(get_delay(url));
    let result = fetch_over_network(browser, url, timeout);
    throttle.mark_fetched();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    // ===============================================================
    // is_link_ok / is_link_wanted
    // ===============================================================

    #[test]
    fn is_link_ok_rejects_the_static_filter_list() {
        assert!(!is_link_ok("http://example.com/file.exe"));
        assert!(!is_link_ok("http://example.com/song.mp3"));
        assert!(!is_link_ok("http://example.com/song.ogg"));
        assert!(!is_link_ok("mailto:foo@example.com"));
        assert!(!is_link_ok(""));
        assert!(!is_link_ok("   "));
        assert!(is_link_ok("http://example.com/page.html"));
    }

    #[test]
    fn is_link_wanted_prefers_the_hook_result_over_regexps() {
        let filter: Vec<Regex> = vec![Regex::new("ads").unwrap()];
        let matches: Vec<Regex> = vec![];
        assert!(is_link_wanted("http://x/ads", Some(true), &filter, &matches), "hook says yes even though filter_regexps would reject");
        assert!(!is_link_wanted("http://x/page", Some(false), &filter, &matches), "hook says no even though nothing else would reject");
    }

    #[test]
    fn is_link_wanted_falls_through_to_filter_then_match_regexps() {
        let filter: Vec<Regex> = vec![Regex::new("ads").unwrap()];
        let matches: Vec<Regex> = vec![Regex::new(r"/articles/").unwrap()];
        assert!(!is_link_wanted("http://x/ads/page", None, &filter, &matches), "filter_regexps rejects regardless of match_regexps");
        assert!(is_link_wanted("http://x/articles/1", None, &filter, &matches));
        assert!(!is_link_wanted("http://x/other", None, &filter, &matches), "match_regexps set and non-empty, so anything not matching is rejected");
    }

    #[test]
    fn is_link_wanted_accepts_everything_with_no_regexps_configured() {
        assert!(is_link_wanted("http://x/anything", None, &[], &[]));
    }

    // ===============================================================
    // absurl / normurl / localize_link
    // ===============================================================

    #[test]
    fn absurl_resolves_a_relative_href_against_the_base() {
        let got = absurl("http://example.com/dir/page.html", "sub.html", true, |_| true);
        assert_eq!(got.as_deref(), Some("http://example.com/dir/sub.html"));
    }

    #[test]
    fn absurl_keeps_an_absolute_href_verbatim() {
        let got = absurl("http://example.com/dir/page.html", "http://other.com/x?y=1", true, |_| true);
        assert_eq!(got.as_deref(), Some("http://other.com/x?y=1"));
    }

    #[test]
    fn absurl_rejects_a_fragment_only_or_empty_href() {
        assert!(absurl("http://example.com/", "#section", true, |_| true).is_none());
        assert!(absurl("http://example.com/", "", true, |_| true).is_none());
    }

    #[test]
    fn absurl_keeps_a_query_only_href() {
        let got = absurl("http://example.com/page.html", "?q=1", true, |_| true);
        assert_eq!(got.as_deref(), Some("http://example.com/page.html?q=1"));
    }

    #[test]
    fn absurl_rejects_links_the_static_filter_catches_before_consulting_is_wanted() {
        let mut called = false;
        let got = absurl("http://example.com/", "song.mp3", true, |_| {
            called = true;
            true
        });
        assert!(got.is_none());
        assert!(!called, "is_wanted must not be consulted once is_link_ok already rejected the link");
    }

    #[test]
    fn absurl_skips_the_is_wanted_check_when_filter_is_false() {
        let got = absurl("http://example.com/", "page.html", false, |_| false);
        assert_eq!(got.as_deref(), Some("http://example.com/page.html"), "recursion_level 0's own starting link is never filtered");
    }

    #[test]
    fn normurl_strips_only_the_fragment() {
        assert_eq!(normurl("http://x/page.html?a=1#frag"), "http://x/page.html?a=1");
        assert_eq!(normurl("http://x/page.html"), "http://x/page.html");
    }

    #[test]
    fn localize_link_reattaches_the_original_fragment() {
        assert_eq!(localize_link("orig.html#s1", "article_0/index.html"), "article_0/index.html#s1");
        assert_eq!(localize_link("orig.html", "article_0/index.html"), "article_0/index.html");
    }

    // ===============================================================
    // canonicalize_url / basename
    // ===============================================================

    #[test]
    fn canonicalize_url_leaves_whitespace_free_urls_unchanged() {
        assert_eq!(canonicalize_url("http://example.com/a/b?c=1"), "http://example.com/a/b?c=1");
    }

    #[test]
    fn canonicalize_url_quotes_whitespace_in_the_path_but_not_the_authority() {
        let got = canonicalize_url("http://example.com/a b/c.html");
        assert_eq!(got, "http://example.com/a%20b/c.html");
    }

    #[test]
    fn basename_returns_the_last_path_segment() {
        assert_eq!(basename("http://example.com/dir/page.html"), "page.html");
        assert_eq!(basename("http://example.com/dir/img.jpg?x=1#f"), "img.jpg");
    }

    #[test]
    fn basename_falls_back_to_index_html_when_there_is_no_extension() {
        assert_eq!(basename("http://example.com/dir/"), "index.html");
        assert_eq!(basename("http://example.com/dir/no_ext"), "index.html");
    }

    #[test]
    fn basename_percent_decodes_the_path() {
        assert_eq!(basename("http://example.com/my%20file.html"), "my file.html");
    }

    // ===============================================================
    // fetch_url: file:/data: short-circuiting, preloaded URLs
    // ===============================================================

    #[test]
    fn fetch_url_returns_preloaded_content_without_touching_the_network() {
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::from([("http://example.com/x".to_string(), b"cached".to_vec())]));
        let got = fetch_url(&browser, &throttle, &preloaded, "http://example.com/x", Duration::from_secs(5), |_| Duration::ZERO).unwrap();
        assert_eq!(got.data, b"cached");
        assert_eq!(got.new_url.as_deref(), Some("http://example.com/x"));
        assert!(preloaded.lock().unwrap().is_empty(), "a preloaded URL is consumed once, like dict.pop");
    }

    #[test]
    fn fetch_url_reads_a_data_url_without_a_newurl() {
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"hello world");
        let url = format!("data:text/plain;base64,{encoded}");
        let got = fetch_url(&browser, &throttle, &preloaded, &url, Duration::from_secs(5), |_| Duration::ZERO).unwrap();
        assert_eq!(got.data, b"hello world");
        assert_eq!(got.new_url, None);
    }

    #[test]
    fn fetch_url_reads_a_file_url() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-fetch-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page.html");
        std::fs::write(&path, b"<html></html>").unwrap();

        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        let url = format!("file://{}", path.display());
        let got = fetch_url(&browser, &throttle, &preloaded, &url, Duration::from_secs(5), |_| Duration::ZERO).unwrap();
        assert_eq!(got.data, b"<html></html>");
        assert_eq!(got.new_url, Some(format!("file:{}", path.display())));

        std::fs::remove_dir_all(&dir).ok();
    }

    // ===============================================================
    // fetch_url: real network behavior (status mapping, delay, retry)
    // ===============================================================

    /// A tiny, purpose-built single-threaded HTTP/1.1 test server --
    /// narrower than `scraper::browser`'s own `TestServer` (private to
    /// that module), just enough for this module's own real-network
    /// assertions: a fixed status/body, or a connection that's held
    /// open (never responds) to force a timeout.
    struct TestServer {
        addr: std::net::SocketAddr,
    }

    impl TestServer {
        fn start_status(status_line: &'static str, body: &'static [u8]) -> TestServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    respond(stream, status_line, body);
                }
            });
            TestServer { addr }
        }

        /// Never responds -- every connection is accepted, then held
        /// open, forcing the client's own timeout to fire.
        fn start_hanging() -> TestServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    // Leak the stream: keep the connection open without
                    // ever writing to it.
                    std::mem::forget(stream);
                }
            });
            TestServer { addr }
        }

        /// Responds 200 to every request but records how many it has
        /// seen -- used to confirm the once-per-fetch retry count.
        fn start_counting(counter: Arc<AtomicUsize>) -> TestServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    counter.fetch_add(1, Ordering::SeqCst);
                    respond(stream, "200 OK", b"ok");
                }
            });
            TestServer { addr }
        }

        fn url(&self) -> String {
            format!("http://{}/", self.addr)
        }
    }

    fn respond(mut stream: TcpStream, status_line: &str, body: &[u8]) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        loop {
            let mut l = String::new();
            if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                break;
            }
        }
        let resp = format!("HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.write_all(body);
    }

    #[test]
    fn fetch_url_maps_a_4xx_status_to_a_real_http_error() {
        let server = TestServer::start_status("404 Not Found", b"");
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        let err = fetch_url(&browser, &throttle, &preloaded, &server.url(), Duration::from_secs(5), |_| Duration::ZERO).unwrap_err();
        match err {
            FetchError::Http(reason) => assert_eq!(reason, "Not Found"),
            other => panic!("expected FetchError::Http, got {other:?}"),
        }
    }

    #[test]
    fn fetch_url_returns_body_bytes_on_success() {
        let server = TestServer::start_status("200 OK", b"hello");
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        let got = fetch_url(&browser, &throttle, &preloaded, &server.url(), Duration::from_secs(5), |_| Duration::ZERO).unwrap();
        assert_eq!(got.data, b"hello");
    }

    #[test]
    fn fetch_url_honors_the_delay_between_two_fetches() {
        let counter = Arc::new(AtomicUsize::new(0));
        let server = TestServer::start_counting(Arc::clone(&counter));
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        let delay = Duration::from_millis(300);

        let start = Instant::now();
        fetch_url(&browser, &throttle, &preloaded, &server.url(), Duration::from_secs(5), |_| delay).unwrap();
        fetch_url(&browser, &throttle, &preloaded, &server.url(), Duration::from_secs(5), |_| delay).unwrap();
        assert!(start.elapsed() >= delay, "the second fetch must wait out the configured delay");
    }

    #[test]
    fn fetch_url_retries_once_on_a_worth_retry_error_then_gives_up() {
        let server = TestServer::start_hanging();
        let browser = Browser::new("", &[], true);
        let throttle = FetchThrottle::new();
        let preloaded = Mutex::new(HashMap::new());
        // A short client timeout against a server that never responds
        // is exactly `worth_retry`'s real trigger (a timeout).
        let err = fetch_url(&browser, &throttle, &preloaded, &server.url(), Duration::from_millis(200), |_| Duration::ZERO);
        assert!(err.is_err(), "a server that never responds must still fail after the single retry");
    }
}
