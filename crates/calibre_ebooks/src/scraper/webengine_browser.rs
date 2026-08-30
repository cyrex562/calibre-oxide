//! Port of `qt.py`'s `WebEngineBrowser` (the `webengine_backend.py`
//! half of `calibre/scraper/`) -- a JS-rendering fetcher, backed by a
//! real OS-native webview instead of a plain HTTP client.
//!
//! # Architecture
//!
//! [`WebEngineBrowser`] doesn't render pages itself -- it spawns and
//! talks to `calibre_scraper_worker` (a separate binary crate in this
//! workspace) over a JSON-lines stdin/stdout protocol, mirroring
//! upstream's own subprocess/IPC design almost exactly (a `Browser`
//! spawning a worker process it exchanges JSON commands with). The
//! only real difference: upstream's worker embeds a `QWebEnginePage`
//! (Chromium via PyQt); the worker here uses `wry`/`tao` -- the same
//! webview stack `app/src-tauri`, this workspace's own Tauri desktop
//! app, already depends on (`webkit2gtk` on Linux, `WebView2` on
//! Windows, `WKWebView` on macOS).
//!
//! `calibre_ebooks` itself stays free of that GUI/webview dependency --
//! it only needs to know how to spawn a binary and speak JSON lines to
//! it, which is why the worker lives in its own crate
//! (`crates/calibre_scraper_worker`) rather than being pulled in here
//! directly. Callers supply the path to that compiled binary explicitly
//! ([`WebEngineBrowser::new`]'s `worker_binary` parameter), matching
//! this whole port's "caller supplies deployment-specific paths"
//! convention (e.g. `catalogs::build::CatalogBuildOptions::resources_dir`
//! in `calibre_db`).
//!
//! # Requires a display
//!
//! An OS-native webview needs a real (or virtual) display server --
//! there is no headless/offscreen webview mode. On a machine with no
//! display, run whatever process ends up spawning
//! `calibre_scraper_worker` under `xvfb-run` (or an equivalent virtual
//! framebuffer), the same constraint upstream's own Qt/QtWebEngine
//! backend has. This module's own tests detect a missing display and
//! skip rather than fail (see `tests::has_display`).
//!
//! # Disclosed gaps versus [`super::browser::Browser`]
//!
//! - **No request bodies.** `wry`'s URL-loading API
//!   (`WebView::load_url_with_headers`) has no way to attach a body to
//!   a navigation -- there's no `<form method=post>` submission or JS
//!   `fetch()` trick wired up here. [`WebEngineBrowser::open`] returns
//!   an error immediately if [`super::browser::OpenOptions::data`] is
//!   set, rather than silently dropping the body.
//! - **No response status/headers.** A real top-level navigation
//!   doesn't expose the main document request's raw HTTP status code
//!   or response headers the way upstream's own `scraper.js`-injected
//!   `fetch()` trick did (see `calibre_scraper_worker`'s own doc for
//!   why this port does a real navigation instead of that trick).
//!   [`BrowserResponse::status`]/[`BrowserResponse::headers`] are
//!   always `None`/empty on a WebEngineBrowser response; a successful
//!   `open` means "the page loaded", not "the server returned 200".
//! - **Cookies with no `domain`** (upstream's "send with every
//!   request" case, [`Browser::set_simple_cookie`]'s `domain: None`)
//!   are silently dropped rather than applied -- see
//!   `calibre_scraper_worker`'s own `apply_cookie` doc.
//!
//! Sites that need exact status/headers or POST bodies should use the
//! plain [`super::browser::Browser`] instead; this type exists
//! specifically for sites that need real JS execution to render usable
//! content at all.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::browser::{merge_headers, BrowserError, BrowserResponse, OpenOptions};

#[derive(Debug, Clone)]
struct StoredCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
}

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Port of `WebEngineBrowser` -- see this module's doc for the full
/// architecture and disclosed gaps.
pub struct WebEngineBrowser {
    worker_binary: PathBuf,
    worker: Mutex<Option<WorkerProcess>>,
    user_agent: Mutex<String>,
    extra_headers: Mutex<Vec<(String, String)>>,
    cookies: Mutex<Vec<StoredCookie>>,
    next_id: AtomicU64,
}

impl WebEngineBrowser {
    /// `worker_binary` is the path to a compiled `calibre_scraper_worker`
    /// binary -- not bundled or looked up automatically, see this
    /// module's doc. `user_agent` empty means "pick a random common
    /// Chrome user agent", matching [`super::browser::Browser::new`]'s
    /// own default.
    pub fn new(worker_binary: impl Into<PathBuf>, user_agent: &str, headers: &[(String, String)]) -> WebEngineBrowser {
        let user_agent =
            if user_agent.is_empty() { calibre_utils::random_ua::random_common_chrome_user_agent() } else { user_agent.to_string() };
        WebEngineBrowser {
            worker_binary: worker_binary.into(),
            worker: Mutex::new(None),
            user_agent: Mutex::new(user_agent),
            extra_headers: Mutex::new(headers.to_vec()),
            cookies: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Port of `open`. See this module's doc: no request bodies, no
    /// real status/headers on the response.
    pub fn open(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        self.fetch(url, opts)
    }

    /// Port of `open_novisit` -- identical to [`WebEngineBrowser::open`],
    /// same reasoning as [`super::browser::Browser::open_novisit`]
    /// (upstream's `visit` flag is inert in both backends).
    pub fn open_novisit(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        self.fetch(url, opts)
    }

    fn fetch(&self, url: &str, opts: &OpenOptions) -> Result<BrowserResponse, BrowserError> {
        if opts.data.is_some() {
            return Err(BrowserError::new(
                "WebEngineBrowser does not support request bodies -- wry's URL-loading API has no way to attach one to a navigation"
                    .to_string(),
                false,
            ));
        }

        let mut header_parts = self.extra_headers.lock().unwrap().clone();
        header_parts.extend(opts.headers.iter().cloned());
        let merged = merge_headers(header_parts);

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let timeout_ms = opts.timeout.unwrap_or(Duration::from_secs(60)).as_millis() as u64;
        let user_agent = self.user_agent.lock().unwrap().clone();

        let cmd = serde_json::json!({
            "action": "fetch",
            "id": id,
            "url": url,
            "user_agent": user_agent,
            "headers": merged,
            "timeout_ms": timeout_ms,
        });

        let mut guard = self.worker.lock().unwrap();
        self.ensure_worker(&mut guard)?;
        let worker = guard.as_mut().expect("just ensured");
        Self::send_command(worker, &cmd)?;
        Self::read_response(worker, id)
    }

    fn ensure_worker(&self, guard: &mut Option<WorkerProcess>) -> Result<(), BrowserError> {
        if guard.is_some() {
            return Ok(());
        }
        let mut child = Command::new(&self.worker_binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| BrowserError::new(format!("failed to spawn scraper worker {:?}: {e}", self.worker_binary), false))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        *guard = Some(WorkerProcess { child, stdin, stdout });
        let worker = guard.as_mut().expect("just set");
        for c in self.cookies.lock().unwrap().iter() {
            let cmd = serde_json::json!({"action": "set_cookie", "name": c.name, "value": c.value, "domain": c.domain, "path": c.path});
            let _ = Self::send_command(worker, &cmd);
        }
        Ok(())
    }

    fn send_command(worker: &mut WorkerProcess, cmd: &serde_json::Value) -> Result<(), BrowserError> {
        let line = serde_json::to_string(cmd).expect("commands built here always serialize");
        worker
            .stdin
            .write_all(line.as_bytes())
            .and_then(|_| worker.stdin.write_all(b"\n"))
            .and_then(|_| worker.stdin.flush())
            .map_err(|e| BrowserError::new(format!("failed to write to scraper worker: {e}"), true))
    }

    fn read_response(worker: &mut WorkerProcess, expected_id: u64) -> Result<BrowserResponse, BrowserError> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = worker.stdout.read_line(&mut line).map_err(|e| BrowserError::new(format!("failed to read from scraper worker: {e}"), true))?;
            if n == 0 {
                return Err(BrowserError::new("scraper worker process exited unexpectedly".to_string(), true));
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                continue;
            };
            if v.get("id").and_then(serde_json::Value::as_u64) != Some(expected_id) {
                continue;
            }
            if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
                let worth_retry = v.get("worth_retry").and_then(serde_json::Value::as_bool).unwrap_or(false);
                return Err(BrowserError::new(err.to_string(), worth_retry));
            }
            let final_url = v.get("final_url").and_then(serde_json::Value::as_str).unwrap_or(default_final_url()).to_string();
            let html = v.get("html").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            return Ok(BrowserResponse::from_webview(final_url, html.into_bytes()));
        }
    }

    /// Port of `set_simple_cookie`/`set_cookie`. Applied immediately if
    /// the worker is already running; otherwise queued and replayed
    /// when it next starts.
    pub fn set_simple_cookie(&self, name: &str, value: &str, domain: Option<&str>, path: Option<&str>) {
        let c = StoredCookie { name: name.to_string(), value: value.to_string(), domain: domain.map(str::to_string), path: path.map(str::to_string) };
        let mut guard = self.worker.lock().unwrap();
        if let Some(worker) = guard.as_mut() {
            let cmd = serde_json::json!({"action": "set_cookie", "name": c.name, "value": c.value, "domain": c.domain, "path": c.path});
            let _ = Self::send_command(worker, &cmd);
        }
        self.cookies.lock().unwrap().push(c);
    }

    /// Port of `set_user_agent`. Takes effect on the *next* fetch --
    /// the worker rebuilds its webview automatically when it sees a
    /// user agent that differs from the one it's currently using.
    pub fn set_user_agent(&self, val: &str) {
        *self.user_agent.lock().unwrap() = val.to_string();
    }

    /// Port of `shutdown`: asks the worker to quit and waits for it to
    /// exit. Unlike [`super::browser::Browser::shutdown`] (a no-op --
    /// there's no subprocess there), this one matters:
    /// [`WebEngineBrowser`] also implements [`Drop`] to call it
    /// automatically, matching upstream's own `atexit`/`__del__`
    /// cleanup.
    pub fn shutdown(&self) {
        let mut guard = self.worker.lock().unwrap();
        if let Some(mut worker) = guard.take() {
            let _ = Self::send_command(&mut worker, &serde_json::json!({"action": "quit"}));
            let _ = worker.child.wait();
        }
    }
}

impl Drop for WebEngineBrowser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn default_final_url() -> &'static str {
    ""
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::browser::RequestBody;
    use std::io::{BufRead as _, Write as _};
    use std::net::TcpListener;
    use std::thread;

    /// This module's tests spawn the real `calibre_scraper_worker`
    /// binary, which needs an OS-native webview -- and therefore a real
    /// or virtual display. Skip rather than fail when neither is
    /// available (matching this whole codebase's "skip gracefully when
    /// a required environment resource is missing" convention), so
    /// `cargo test` still passes in a bare CI container; run under
    /// `xvfb-run -a cargo test -p calibre_ebooks --lib scraper::webengine_browser`
    /// to actually exercise this module.
    fn has_display() -> bool {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    fn worker_binary() -> Option<PathBuf> {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/calibre_scraper_worker")
            .canonicalize()
            .ok()?;
        candidate.exists().then_some(candidate)
    }

    fn start_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    headers.push_str(&line);
                }
                let body = format!(
                    "<!DOCTYPE html><html><head><title>t</title></head><body><h1 id=\"m\">static</h1>\
                     <script>document.getElementById('m').textContent = 'js-rendered ua=' + navigator.userAgent + ' headers={}';</script>\
                     </body></html>",
                    headers.replace('"', "'").replace('\n', " ").replace('\r', "")
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn renders_javascript_and_applies_custom_user_agent_and_headers() {
        if !has_display() {
            eprintln!("skipping: no DISPLAY/WAYLAND_DISPLAY (run under xvfb-run to exercise this test)");
            return;
        }
        let Some(worker_binary) = worker_binary() else {
            eprintln!("skipping: calibre_scraper_worker binary not built (run `cargo build -p calibre_scraper_worker` first)");
            return;
        };

        let url = start_server();
        let br = WebEngineBrowser::new(worker_binary, "webengine-test-ua", &[("X-Scraper-Test".to_string(), "1".to_string())]);
        let opts = OpenOptions { timeout: Some(Duration::from_secs(90)), ..Default::default() };
        let resp = br.open(&url, &opts).expect("fetch should succeed");

        let html = String::from_utf8(resp.read().to_vec()).unwrap();
        assert!(html.contains("js-rendered"), "JS did not execute -- got: {html}");
        assert!(html.contains("webengine-test-ua"), "user agent not reflected in navigator.userAgent -- got: {html}");
        assert!(html.to_lowercase().contains("x-scraper-test"), "custom header not sent -- got: {html}");
        assert!(resp.url().starts_with(&url));
    }

    #[test]
    fn a_request_body_is_rejected_rather_than_silently_dropped() {
        if !has_display() {
            return;
        }
        let Some(worker_binary) = worker_binary() else {
            return;
        };
        let br = WebEngineBrowser::new(worker_binary, "ua", &[]);
        let opts = OpenOptions { data: Some(RequestBody::Bytes(b"x".to_vec())), ..Default::default() };
        let err = br.open("http://127.0.0.1:1/unreachable", &opts).unwrap_err();
        assert!(!err.worth_retry());
    }
}
