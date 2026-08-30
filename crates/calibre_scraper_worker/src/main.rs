//! A small worker process that renders a URL through a real,
//! JS-capable OS webview (via `wry`/`tao` -- the same stack the
//! `app/src-tauri` desktop app uses) and reports the final rendered
//! HTML back to its parent over a JSON-lines stdin/stdout protocol.
//!
//! This is the Rust counterpart of upstream calibre's
//! `calibre/scraper/webengine_backend.py`: a subprocess a `Browser`
//! (here, `calibre_ebooks::scraper::browser::WebEngineBrowser`) spawns
//! on demand so JS-rendering fetches don't require the caller itself to
//! be running inside a GUI event loop. It lives in its own crate,
//! separate from `calibre_ebooks`, so that crate -- used from plenty of
//! headless/CLI contexts -- never has to link a GUI/webview toolkit
//! (`webkit2gtk` on Linux, `WebView2` on Windows, `WKWebView` on macOS)
//! just to provide plain HTTP fetching.
//!
//! # Requires a display
//!
//! Like upstream's own Qt/QtWebEngine backend, an OS-native webview
//! needs a real (or virtual) display server to exist at all -- there is
//! no true headless/offscreen webview mode here. On a machine with no
//! display, run this under `xvfb-run` (or an equivalent virtual
//! framebuffer) the same way CI systems run headless browser tests.
//!
//! # Protocol
//!
//! One JSON object per line on stdin:
//! - `{"action":"fetch","id":<u64>,"url":<string>,"user_agent":<string>,
//!   "headers":[[<name>,<value>],...],"timeout_ms":<u64>}` -- render
//!   `url` and report back the outer HTML of the loaded document.
//! - `{"action":"set_cookie","name":<string>,"value":<string>,
//!   "domain":<string|null>,"path":<string|null>}`
//! - `{"action":"quit"}`
//!
//! One JSON object per line on stdout per `fetch`:
//! - `{"action":"finished","id":<u64>,"final_url":<string|null>,
//!   "html":<string|null>,"error":<string|null>,"worth_retry":<bool>}`
//!
//! Only one `fetch` may be outstanding at a time -- the client is
//! expected to wait for a `finished` response before sending the next
//! `fetch` (this worker has exactly one webview, not a pool; a second
//! concurrent `fetch` gets an immediate `worth_retry: false` error
//! rather than being queued).
//!
//! # Not ported: `scraper.js`'s in-page fetch/download protocol
//!
//! Upstream's `webengine_backend.py` doesn't just navigate the page --
//! it loads a data: URL whose base URL is set to the target URL, then
//! uses an injected `scraper.js` to issue the *actual* request via the
//! page's own `fetch()`, chunking the response back out via
//! `console.log`-based messaging. That trick makes even the *primary*
//! request happen through the page's own JS fetch machinery (real
//! status/header/redirect visibility) while staying same-origin for
//! CORS purposes.
//!
//! This worker instead does a real top-level navigation to the target
//! URL directly -- arguably a *more* convincing anti-bot fingerprint
//! (an actual navigation not a fetch-from-a-data-url trick), and it
//! sidesteps CORS entirely since there's no cross-origin fetch
//! involved. The cost: `wry` doesn't expose the main document
//! navigation's raw HTTP status code or response headers the way a
//! page-level `fetch()` Response object would, so `finished` responses
//! carry no `status`/`headers` fields -- a successful `fetch` implies
//! "the page loaded", not "the server returned 200". Callers that need
//! exact status/headers for a JS-hostile site should use the plain
//! `Browser` instead.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebView, WebViewBuilder};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Command {
    Fetch {
        id: u64,
        url: String,
        #[serde(default)]
        user_agent: String,
        #[serde(default)]
        headers: Vec<(String, String)>,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    SetCookie {
        name: String,
        value: String,
        #[serde(default)]
        domain: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    Quit,
}

fn default_timeout_ms() -> u64 {
    60_000
}

#[derive(Debug, Serialize)]
struct FinishedResult {
    action: &'static str,
    id: u64,
    final_url: Option<String>,
    html: Option<String>,
    error: Option<String>,
    worth_retry: bool,
}

fn write_result(r: &FinishedResult) {
    if let Ok(line) = serde_json::to_string(r) {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(line.as_bytes());
        let _ = lock.write_all(b"\n");
        let _ = lock.flush();
    }
}

#[derive(Debug)]
enum AppEvent {
    Command(Command),
    /// The webview reported `PageLoadEvent::Finished` for `id`'s in-flight fetch.
    PageFinished { id: u64, url: String },
    /// `evaluate_script_with_callback` for `id` has returned; safe to
    /// start the next queued fetch now.
    RequestDone { id: u64 },
}

struct PendingFetch {
    id: u64,
    deadline: Instant,
}

struct StoredCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
}

fn apply_cookie(webview: &WebView, c: &StoredCookie) {
    let domain = c.domain.clone().unwrap_or_default();
    let path = c.path.clone().unwrap_or_else(|| "/".to_string());
    // wry's `cookie::Cookie` needs a concrete domain to be attached at
    // all; a cookie with no domain (upstream's "send with every
    // request" case) has no clean webview equivalent -- skipped here
    // rather than guessed at, matching this whole port's "don't fake a
    // capability that doesn't exist" convention.
    if domain.is_empty() {
        return;
    }
    let builder = wry::cookie::Cookie::build((c.name.clone(), c.value.clone())).domain(domain).path(path);
    let cookie = builder.inner();
    let _ = webview.set_cookie(&cookie);
}

/// Builds a fresh webview and immediately points it at `url`/`headers`
/// as part of construction (`with_url_and_headers`), rather than
/// building against a placeholder page and issuing a separate
/// `load_url_with_headers` call afterwards -- a placeholder's own
/// `Started`/`Finished` load events would otherwise race the real
/// fetch's `current_id` bookkeeping, since they fire asynchronously
/// after the builder returns.
fn build_webview<'a>(
    window: &'a tao::window::Window,
    user_agent: &str,
    url: &str,
    header_map: http::HeaderMap,
    proxy: EventLoopProxy<AppEvent>,
    current_id: Arc<Mutex<Option<u64>>>,
) -> WebView {
    let mut builder = WebViewBuilder::new()
        .with_visible(false)
        .with_url_and_headers(url, header_map)
        .with_on_page_load_handler(move |event, url| {
            if matches!(event, PageLoadEvent::Finished) {
                if let Some(id) = *current_id.lock().unwrap() {
                    let _ = proxy.send_event(AppEvent::PageFinished { id, url });
                }
            }
        });
    if !user_agent.is_empty() {
        builder = builder.with_user_agent(user_agent);
    }
    builder.build(window).expect("failed to create webview -- is a display (or Xvfb) available?")
}

fn main() {
    let event_loop: EventLoop<AppEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Command>(&line) {
                    Ok(cmd) => {
                        if proxy.send_event(AppEvent::Command(cmd)).is_err() {
                            break;
                        }
                    }
                    Err(e) => eprintln!("calibre_scraper_worker: bad command: {e}"),
                }
            }
            let _ = proxy.send_event(AppEvent::Command(Command::Quit));
        });
    }

    let window = WindowBuilder::new()
        .with_visible(true)
        .with_inner_size(tao::dpi::LogicalSize::new(1024.0, 768.0))
        .build(&event_loop)
        .expect("failed to create window -- is a display (or Xvfb) available?");

    let current_id: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    let mut webview: Option<WebView> = None;
    let mut webview_user_agent = String::new();
    let mut cookies: Vec<StoredCookie> = Vec::new();
    let mut pending: Option<PendingFetch> = None;
    let mut queue: VecDeque<(u64, String, String, Vec<(String, String)>, u64)> = VecDeque::new();

    event_loop.run(move |event, window_target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }

            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(p) = &pending {
                    if Instant::now() >= p.deadline {
                        write_result(&FinishedResult {
                            action: "finished",
                            id: p.id,
                            final_url: None,
                            html: None,
                            error: Some("timed out".to_string()),
                            worth_retry: true,
                        });
                        *current_id.lock().unwrap() = None;
                        pending = None;
                    }
                }
            }

            Event::UserEvent(AppEvent::Command(Command::Quit)) => {
                *control_flow = ControlFlow::Exit;
            }

            Event::UserEvent(AppEvent::Command(Command::SetCookie { name, value, domain, path })) => {
                let c = StoredCookie { name, value, domain, path };
                if let Some(wv) = &webview {
                    apply_cookie(wv, &c);
                }
                cookies.push(c);
            }

            Event::UserEvent(AppEvent::Command(Command::Fetch { id, url, user_agent, headers, timeout_ms })) => {
                if pending.is_some() {
                    write_result(&FinishedResult {
                        action: "finished",
                        id,
                        final_url: None,
                        html: None,
                        error: Some("a fetch is already in progress".to_string()),
                        worth_retry: false,
                    });
                } else {
                    queue.push_back((id, url, user_agent, headers, timeout_ms));
                    start_next(
                        window_target,
                        &window,
                        &mut webview,
                        &mut webview_user_agent,
                        &cookies,
                        &proxy,
                        current_id.clone(),
                        &mut queue,
                        &mut pending,
                    );
                }
            }

            Event::UserEvent(AppEvent::PageFinished { id, url }) => {
                let matches = pending.as_ref().is_some_and(|p| p.id == id);
                if matches {
                    // Only act on the first Finished for this id (a page
                    // with iframes can fire more than one).
                    *current_id.lock().unwrap() = None;
                    if let Some(wv) = &webview {
                        let result_proxy = proxy.clone();
                        let callback_url = url.clone();
                        let eval = wv.evaluate_script_with_callback("document.documentElement.outerHTML", move |raw| {
                            let html: String = serde_json::from_str(&raw).unwrap_or(raw);
                            write_result(&FinishedResult {
                                action: "finished",
                                id,
                                final_url: Some(callback_url.clone()),
                                html: Some(html),
                                error: None,
                                worth_retry: false,
                            });
                            let _ = result_proxy.send_event(AppEvent::RequestDone { id });
                        });
                        if eval.is_err() {
                            write_result(&FinishedResult {
                                action: "finished",
                                id,
                                final_url: Some(url),
                                html: None,
                                error: Some("failed to evaluate script".to_string()),
                                worth_retry: false,
                            });
                            pending = None;
                        }
                    }
                }
            }

            Event::UserEvent(AppEvent::RequestDone { id }) => {
                if pending.as_ref().is_some_and(|p| p.id == id) {
                    pending = None;
                }
                start_next(
                    window_target,
                    &window,
                    &mut webview,
                    &mut webview_user_agent,
                    &cookies,
                    &proxy,
                    current_id.clone(),
                    &mut queue,
                    &mut pending,
                );
            }

            _ => {}
        }

        if let Some(p) = &pending {
            *control_flow = ControlFlow::WaitUntil(p.deadline);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn start_next(
    _window_target: &tao::event_loop::EventLoopWindowTarget<AppEvent>,
    window: &tao::window::Window,
    webview: &mut Option<WebView>,
    webview_user_agent: &mut String,
    cookies: &[StoredCookie],
    proxy: &EventLoopProxy<AppEvent>,
    current_id: Arc<Mutex<Option<u64>>>,
    queue: &mut VecDeque<(u64, String, String, Vec<(String, String)>, u64)>,
    pending: &mut Option<PendingFetch>,
) {
    if pending.is_some() {
        return;
    }
    let Some((id, url, user_agent, headers, timeout_ms)) = queue.pop_front() else {
        return;
    };

    let mut header_map = http::HeaderMap::new();
    for (k, v) in &headers {
        if let (Ok(name), Ok(value)) = (http::HeaderName::from_bytes(k.as_bytes()), http::HeaderValue::from_str(v)) {
            header_map.insert(name, value);
        }
    }

    // Set current_id *before* triggering any navigation, fresh-build or
    // reused -- the load-finished event for this fetch can otherwise
    // race in before it's set.
    *current_id.lock().unwrap() = Some(id);
    *pending = Some(PendingFetch { id, deadline: Instant::now() + Duration::from_millis(timeout_ms) });

    if webview.is_none() || *webview_user_agent != user_agent {
        *webview = Some(build_webview(window, &user_agent, &url, header_map, proxy.clone(), current_id.clone()));
        *webview_user_agent = user_agent.clone();
        for c in cookies {
            apply_cookie(webview.as_ref().unwrap(), c);
        }
    } else {
        let wv = webview.as_ref().unwrap();
        if let Err(e) = wv.load_url_with_headers(&url, header_map) {
            write_result(&FinishedResult {
                action: "finished",
                id,
                final_url: None,
                html: None,
                error: Some(e.to_string()),
                worth_retry: false,
            });
            *current_id.lock().unwrap() = None;
            *pending = None;
        }
    }
}
