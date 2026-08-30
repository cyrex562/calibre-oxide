//! Port of `qt.py`'s `Browser`/`FakeResponse` -- the plain,
//! `reqwest`-based fetcher. `qt_backend.py`'s `FetchBackend` (the
//! subprocess `Browser` talks to over its own JSON-lines IPC protocol,
//! built on `QNetworkAccessManager`) has no separate Rust port -- there
//! is nothing left to port once the transport moves in-process, see
//! this module's parent doc for the full explanation.
//!
//! [`super::webengine_browser::WebEngineBrowser`] is the JS-rendering
//! counterpart (`webengine_backend.py`'s port) -- a genuinely different
//! implementation with its own subprocess IPC, in its own file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::Method;
use url::Url;

/// A fetched page or resource -- port of `FakeResponse`, minus its
/// file-object-shaped partial-read API (`read(n)`/`seek`/`tell`):
/// upstream's version is backed by a temp file because the actual
/// bytes arrive asynchronously from the subprocess worker; here the
/// fetch has already completed synchronously by the time a
/// `BrowserResponse` exists; callers that want the whole body just call
/// [`BrowserResponse::read`].
#[derive(Debug, Clone)]
pub struct BrowserResponse {
    final_url: String,
    status: Option<u16>,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl BrowserResponse {
    /// Used by [`super::webengine_browser`]: a webview-rendered response
    /// has a final URL and a body (the rendered HTML), but no real
    /// status/headers -- see that module's doc for why.
    pub(crate) fn from_webview(final_url: String, body: Vec<u8>) -> BrowserResponse {
        BrowserResponse { final_url, status: None, reason: String::new(), headers: Vec::new(), body }
    }

    /// Port of `FakeResponse.read()` (whole-body form only -- see this
    /// struct's doc).
    pub fn read(&self) -> &[u8] {
        &self.body
    }

    /// Consumes the response, returning its body.
    pub fn into_bytes(self) -> Vec<u8> {
        self.body
    }

    /// Port of the `url`/`geturl` property pair: the final URL after
    /// following any redirects.
    pub fn url(&self) -> &str {
        &self.final_url
    }

    /// Port of the `status`/`code`/`getcode` property pair.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Port of `reason`: the HTTP status line's reason phrase. Always
    /// the *canonical* phrase for the status code (`http::StatusCode`'s
    /// own table) rather than whatever exact text the server sent --
    /// matching upstream's own fallback path
    /// (`from http.client import responses`), just applied
    /// unconditionally instead of only when the backend didn't supply
    /// one (`reqwest` doesn't expose the raw wire reason phrase
    /// separately from the parsed status code).
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Port of `headers`/`getinfo`: every response header, in the order
    /// they arrived. Case-insensitive lookup: see [`BrowserResponse::header`].
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The first value of the response header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Port of the request body upstream's `_open` accepts as `data`: either
/// a raw byte payload (`isinstance(data, (str, bytes))`, str already
/// UTF-8-encoded by the caller) or a form dict (`isinstance(data, dict)`,
/// URL-encoded here the same way `urlencode` does upstream).
#[derive(Debug, Clone)]
pub enum RequestBody {
    Bytes(Vec<u8>),
    Form(Vec<(String, String)>),
}

/// Per-request overrides -- port of the extra arguments `Browser.open`/
/// `open_novisit` and `urllib.request.Request` together carry: per-request
/// headers, an optional body, an optional timeout override, and an
/// optional HTTP method (inferred from the presence of a body when unset,
/// matching upstream's own `method = 'POST' if data else 'GET'`).
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    pub headers: Vec<(String, String)>,
    pub data: Option<RequestBody>,
    pub timeout: Option<Duration>,
    pub method: Option<Method>,
}

/// Port of `URLError`/its `worth_retry` attribute
/// (`ex.worth_retry = bool(res.get('worth_retry'))`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BrowserError {
    message: String,
    worth_retry: bool,
}

impl BrowserError {
    /// Used by [`super::webengine_browser`], which raises its own
    /// worker-process/protocol errors rather than ones `reqwest`
    /// classified (see [`classify_error`]).
    pub(crate) fn new(message: String, worth_retry: bool) -> BrowserError {
        BrowserError { message, worth_retry }
    }

    /// Whether upstream's backend would have flagged this failure as
    /// `worth_retry` -- timeouts and connection failures were (matching
    /// `qt_backend.py`'s `TimeoutError`/`TemporaryNetworkFailureError`/
    /// `ConnectionRefusedError`/`RemoteHostClosedError`/
    /// `SslHandshakeFailedError` set); `reqwest` doesn't expose as fine
    /// a taxonomy, so this collapses to "was it a connect or timeout
    /// failure" -- a disclosed approximation, not an exact enum match.
    pub fn worth_retry(&self) -> bool {
        self.worth_retry
    }
}

fn classify_error(e: reqwest::Error) -> BrowserError {
    let worth_retry = e.is_timeout() || e.is_connect();
    BrowserError { message: e.to_string(), worth_retry }
}

/// Case-insensitively merges repeated header names into one
/// comma-joined value, in first-seen order -- port of
/// `qt_backend.py::FetchBackend.download`'s `QNetworkRequest::
/// setRawHeader` merge-on-repeat loop (`ex = rq.rawHeader(name); if
/// len(ex): val = ex.decode() + ', ' + val`).
pub(crate) fn merge_headers(parts: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, String> = HashMap::new();
    let mut display_name: HashMap<String, String> = HashMap::new();
    for (k, v) in parts {
        let lk = k.to_lowercase();
        match values.get_mut(&lk) {
            Some(existing) => {
                existing.push_str(", ");
                existing.push_str(&v);
            }
            None => {
                values.insert(lk.clone(), v);
                display_name.insert(lk.clone(), k);
                order.push(lk);
            }
        }
    }
    order.into_iter().map(|lk| (display_name.remove(&lk).unwrap(), values.remove(&lk).unwrap())).collect()
}

/// Port of `qt.py`'s `Browser`. Fetches happen synchronously, in-process
/// (see this module's parent doc for why) -- there is no `shutdown()`
/// subprocess to reap and no `clone_browser` aliasing helper (Rust's
/// ownership model makes an explicit "share this browser" `Arc<Browser>`
/// the natural equivalent instead), so both are omitted rather than
/// stubbed.
pub struct Browser {
    client: Client,
    cookie_jar: Arc<reqwest::cookie::Jar>,
    user_agent: Mutex<String>,
    /// Upstream's `self.addheaders` -- static headers sent with every request.
    extra_headers: Mutex<Vec<(String, String)>>,
    /// Cookies set via [`Browser::set_simple_cookie`] with no `domain`
    /// -- upstream's `CookieJar.all_request_cookies`, re-normalized onto
    /// every outgoing request's own URL (`cookiesForUrl`) rather than
    /// being tied to one host, since `reqwest`'s cookie store (like
    /// `QNetworkCookieJar`'s normal cookies) only tracks host-scoped
    /// cookies.
    domainless_cookies: Mutex<Vec<(String, String)>>,
}

impl Browser {
    /// Port of `Browser.__init__`. `user_agent` empty means "pick a
    /// random common Chrome user agent", matching upstream's own
    /// backend-side `user_agent or random_common_chrome_user_agent()`
    /// default (there's no separate frontend/backend split here for it
    /// to live on, so it happens here instead).
    pub fn new(user_agent: &str, headers: &[(String, String)], verify_ssl_certificates: bool) -> Browser {
        let cookie_jar = Arc::new(reqwest::cookie::Jar::default());
        let mut builder = Client::builder().cookie_provider(Arc::clone(&cookie_jar)).redirect(reqwest::redirect::Policy::limited(10));
        if !verify_ssl_certificates {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let client = builder.build().expect("a reqwest client with only cookie/redirect/TLS options should always build");
        let user_agent =
            if user_agent.is_empty() { calibre_utils::random_ua::random_common_chrome_user_agent() } else { user_agent.to_string() };
        Browser {
            client,
            cookie_jar,
            user_agent: Mutex::new(user_agent),
            extra_headers: Mutex::new(headers.to_vec()),
            domainless_cookies: Mutex::new(Vec::new()),
        }
    }

    /// Port of `open`.
    pub fn open(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        self.fetch(url, opts)
    }

    /// Port of `open_novisit`. Upstream threads a `visit: bool` flag
    /// through to the backend's `download` command, but neither backend
    /// (`qt_backend.py` nor `webengine_backend.py`) ever reads it back
    /// out of the command dict -- it's already inert in the Python
    /// source, not a simplification made here. `open_novisit` is
    /// therefore identical to `open`.
    pub fn open_novisit(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        self.fetch(url, opts)
    }

    fn fetch(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        let parsed = Url::parse(url).map_err(|e| BrowserError { message: e.to_string(), worth_retry: false })?;

        {
            let domainless = self.domainless_cookies.lock().unwrap();
            for (name, value) in domainless.iter() {
                let _ = self.cookie_jar.add_cookie_str(&format!("{name}={value}; Path=/"), &parsed);
            }
        }

        let method = opts.method.clone().unwrap_or(if opts.data.is_some() { Method::POST } else { Method::GET });

        let mut header_parts: Vec<(String, String)> = Vec::new();
        header_parts.push(("User-Agent".to_string(), self.user_agent.lock().unwrap().clone()));
        header_parts.extend(self.extra_headers.lock().unwrap().iter().cloned());
        header_parts.extend(opts.headers.iter().cloned());

        let body_bytes: Option<Vec<u8>> = match &opts.data {
            None => None,
            Some(RequestBody::Bytes(b)) => Some(b.clone()),
            Some(RequestBody::Form(pairs)) => {
                header_parts.push(("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()));
                Some(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish().into_bytes())
            }
        };

        if body_bytes.is_some() && !header_parts.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            header_parts.push(("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()));
        }

        let merged_headers = merge_headers(header_parts);

        let mut rb = self.client.request(method, parsed);
        for (k, v) in &merged_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(body) = body_bytes {
            rb = rb.body(body);
        }
        if let Some(timeout) = opts.timeout {
            rb = rb.timeout(timeout);
        }

        let resp = rb.send().map_err(classify_error)?;
        let final_url = resp.url().to_string();
        let status_code = resp.status();
        let reason = status_code.canonical_reason().unwrap_or("").to_string();
        let headers: Vec<(String, String)> =
            resp.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
        let body = resp.bytes().map_err(classify_error)?.to_vec();

        Ok(BrowserResponse { final_url, status: Some(status_code.as_u16()), reason, headers, body })
    }

    /// Port of `set_simple_cookie`/`set_cookie`.
    pub fn set_simple_cookie(&self, name: &str, value: &str, domain: Option<&str>, path: Option<&str>) {
        match domain {
            Some(domain) => {
                let path = path.unwrap_or("/");
                let base = Url::parse(&format!("https://{}{}", domain.trim_start_matches('.'), path)).ok();
                if let Some(base) = base {
                    let _ = self.cookie_jar.add_cookie_str(&format!("{name}={value}; Domain={domain}; Path={path}"), &base);
                }
            }
            None => {
                self.domainless_cookies.lock().unwrap().push((name.to_string(), value.to_string()));
            }
        }
    }

    /// Port of `set_user_agent`.
    pub fn set_user_agent(&self, val: &str) {
        *self.user_agent.lock().unwrap() = val.to_string();
    }

    /// Port of `Browser.shutdown`. There's no subprocess here to
    /// terminate -- see this struct's doc -- so this is a no-op, kept
    /// only so callers ported from the Python call site don't need an
    /// unconditional edit.
    pub fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    /// A minimal single-threaded HTTP/1.1 test server -- port of
    /// `test_fetch_backend.py`'s hand-rolled `Handler`
    /// (`http.server.BaseHTTPRequestHandler` subclass). No dev-dependency
    /// in this crate provides one, so this is std-only, mirroring
    /// upstream's own choice to hand-roll rather than depend on a
    /// framework for a test fixture.
    struct TestServer {
        addr: std::net::SocketAddr,
    }

    fn read_request(stream: &mut TcpStream) -> (String, String, Vec<(String, String)>, Vec<u8>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("GET").to_string();
        let path = parts.next().unwrap_or("/").to_string();

        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                let (k, v) = (k.trim().to_string(), v.trim().to_string());
                if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).unwrap();
        }
        (method, path, headers, body)
    }

    impl TestServer {
        /// `dont_send_response`/`dont_send_body` mirror the Python
        /// test's own flags of the same name, used to simulate a
        /// timeout.
        fn start(dont_send_response: Arc<AtomicUsize>, dont_send_body: Arc<AtomicUsize>) -> TestServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let request_count = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&request_count);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = match stream {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if dont_send_response.load(Ordering::SeqCst) > 0 {
                        // Hold the connection open without responding, to
                        // simulate an unresponsive server (the client-side
                        // timeout is what's under test).
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    let (method, path, headers, body) = read_request(&mut stream);
                    if path == "/redirect" {
                        let resp = "HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                        continue;
                    }
                    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut hdr_json = String::from("{");
                    for (i, (k, v)) in headers.iter().enumerate() {
                        if i > 0 {
                            hdr_json.push(',');
                        }
                        hdr_json.push_str(&format!("{:?}:{:?}", k, v));
                    }
                    hdr_json.push('}');
                    let data_field = if body.is_empty() {
                        String::new()
                    } else {
                        format!(",\"data\":{:?}", String::from_utf8_lossy(&body))
                    };
                    let payload = format!(
                        "{{\"path\":{:?},\"method\":{:?},\"request_count\":{n}{data_field},\"headers\":{hdr_json}}}",
                        path, method
                    );
                    if dont_send_body.load(Ordering::SeqCst) > 0 {
                        let resp = "HTTP/1.1 200 OK\r\nSet-Cookie: sc=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                        continue;
                    }
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: sc=1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
            TestServer { addr }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }
    }

    fn parse_json_headers(body: &[u8]) -> serde_json::Value {
        serde_json::from_slice(body).expect("test server always returns valid JSON")
    }

    fn header_values<'a>(json: &'a serde_json::Value, name: &str) -> Vec<&'a str> {
        json["headers"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str().unwrap())
            .collect()
    }

    /// Port of `TestFetchBackend.do_recipe_browser_test`'s `Browser`
    /// half (`webengine_browser::tests` has its own, smaller contract
    /// test for `WebEngineBrowser` -- it can't satisfy this one
    /// byte-for-byte, see that module's doc for why).
    fn recipe_browser_contract(open: impl Fn(&str, &OpenOptions) -> Result<BrowserResponse, BrowserError>, set_cookie: impl Fn(&str, &str), set_ua: impl Fn(&str)) {
        let dont_send_response = Arc::new(AtomicUsize::new(0));
        let dont_send_body = Arc::new(AtomicUsize::new(0));
        let server = TestServer::start(Arc::clone(&dont_send_response), Arc::clone(&dont_send_body));

        // First request: browser-level user agent + header land on the wire.
        let r = open(&server.url(""), &OpenOptions::default()).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["method"], "GET");
        assert_eq!(json["request_count"], 1);
        assert_eq!(header_values(&json, "th"), vec!["1"]);
        assert_eq!(header_values(&json, "user-agent"), vec!["test-ua"]);
        assert!(!header_values(&json, "accept-encoding").is_empty());

        // Second request: the `Set-Cookie: sc=1` from the first response
        // comes back as `Cookie: sc=1`.
        let r = open(&server.url(""), &OpenOptions::default()).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["request_count"], 2);
        assert_eq!(header_values(&json, "cookie"), vec!["sc=1"]);

        // Timeout -> worth_retry, whether or not the server accepts the
        // connection at all.
        dont_send_body.store(1, Ordering::SeqCst);
        dont_send_response.store(1, Ordering::SeqCst);
        let opts = OpenOptions { timeout: Some(Duration::from_millis(20)), ..Default::default() };
        let err = open(&server.url(""), &opts).unwrap_err();
        assert!(err.worth_retry());
        dont_send_response.store(0, Ordering::SeqCst);
        let err = open(&server.url(""), &opts).unwrap_err();
        assert!(err.worth_retry());
        dont_send_body.store(0, Ordering::SeqCst);

        // Redirect-following, with a real final URL and headers preserved.
        let r = open(&server.url("/redirect"), &OpenOptions::default()).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["path"], "/redirected");
        assert_eq!(header_values(&json, "th"), vec!["1"]);
        assert!(r.url().ends_with("/redirected"));
        assert_eq!(header_values(&json, "user-agent"), vec!["test-ua"]);

        // Per-request headers merge with browser-level headers, same name
        // combined into one comma-joined value.
        let opts = OpenOptions { headers: vec![("th".to_string(), "2".to_string()), ("tc".to_string(), "1".to_string())], ..Default::default() };
        let r = open(&server.url(""), &opts).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(header_values(&json, "th"), vec!["1, 2"]);
        assert_eq!(header_values(&json, "tc"), vec!["1"]);

        // set_simple_cookie/set_user_agent affect subsequent requests.
        set_cookie("cook", "ie");
        set_ua("man in black");
        let r = open(&server.url(""), &OpenOptions::default()).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(header_values(&json, "user-agent"), vec!["man in black"]);
        let cookie = header_values(&json, "cookie");
        assert_eq!(cookie.len(), 1);
        assert!(cookie[0].contains("sc=1"));
        assert!(cookie[0].contains("cook=ie"));

        // POST with a raw byte body defaults to form-urlencoded content type.
        let opts = OpenOptions { data: Some(RequestBody::Bytes(b"1234".to_vec())), ..Default::default() };
        let r = open(&server.url(""), &opts).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["method"], "POST");
        assert_eq!(json["data"], "1234");
        assert_eq!(header_values(&json, "content-type"), vec!["application/x-www-form-urlencoded"]);
    }

    #[test]
    fn browser_satisfies_the_recipe_contract() {
        let br = Browser::new("test-ua", &[("th".to_string(), "1".to_string())], true);
        recipe_browser_contract(|url, opts| br.open(url, opts), |n, v| br.set_simple_cookie(n, v, None, None), |ua| br.set_user_agent(ua));
    }

    #[test]
    fn open_novisit_behaves_identically_to_open() {
        let dont_send_response = Arc::new(AtomicUsize::new(0));
        let dont_send_body = Arc::new(AtomicUsize::new(0));
        let server = TestServer::start(dont_send_response, dont_send_body);
        let br = Browser::new("ua", &[], true);
        let r = br.open_novisit(&server.url(""), &OpenOptions::default()).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["method"], "GET");
    }

    #[test]
    fn form_body_is_urlencoded_and_sets_content_type() {
        let dont_send_response = Arc::new(AtomicUsize::new(0));
        let dont_send_body = Arc::new(AtomicUsize::new(0));
        let server = TestServer::start(dont_send_response, dont_send_body);
        let br = Browser::new("ua", &[], true);
        let opts = OpenOptions {
            data: Some(RequestBody::Form(vec![("a".to_string(), "1".to_string()), ("b".to_string(), "hello world".to_string())])),
            ..Default::default()
        };
        let r = br.open(&server.url(""), &opts).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(json["data"], "a=1&b=hello+world");
    }

    #[test]
    fn an_explicit_content_type_header_is_not_overridden() {
        let dont_send_response = Arc::new(AtomicUsize::new(0));
        let dont_send_body = Arc::new(AtomicUsize::new(0));
        let server = TestServer::start(dont_send_response, dont_send_body);
        let br = Browser::new("ua", &[], true);
        let opts = OpenOptions {
            data: Some(RequestBody::Bytes(b"{}".to_vec())),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            ..Default::default()
        };
        let r = br.open(&server.url(""), &opts).unwrap();
        let json = parse_json_headers(r.read());
        assert_eq!(header_values(&json, "content-type"), vec!["application/json"]);
    }

    #[test]
    fn merge_headers_combines_repeated_names_in_order() {
        let merged = merge_headers(vec![
            ("th".to_string(), "1".to_string()),
            ("X".to_string(), "y".to_string()),
            ("TH".to_string(), "2".to_string()),
        ]);
        assert_eq!(merged, vec![("th".to_string(), "1, 2".to_string()), ("X".to_string(), "y".to_string())]);
    }
}
