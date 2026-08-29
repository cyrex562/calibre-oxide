//! Port of `old_src/src/calibre/scraper/` -- calibre's HTTP-fetching
//! "Browser" abstraction used by metadata-download plugins and news
//! recipes.
//!
//! # Architecture change from upstream
//!
//! Upstream's `Browser` (`qt.py`) doesn't do HTTP itself -- it spawns a
//! Qt subprocess worker (`qt_backend.py`'s `FetchBackend`, built on
//! `QNetworkAccessManager`) and talks to it over a JSON-lines
//! stdin/stdout protocol, so the actual network I/O happens in a
//! separate process running its own Qt event loop. This workspace has
//! no Qt dependency anywhere (`docs/AGENT_PORTING_GUIDE.md` names
//! `reqwest` as the crate for network scraping instead), so
//! [`browser::Browser`] does the HTTP fetching directly, synchronously,
//! in-process with `reqwest::blocking` -- matching the existing
//! `reqwest::blocking` convention in `oeb::polish::download` (this
//! crate's other real-network-I/O module).
//!
//! There is therefore nothing to separately port from `qt_backend.py`'s
//! `Request`/`DownloadRequest`/`CookieJar`/`FetchBackend` classes or
//! `qt.py`'s `FakeResponse` -- those exist purely to shuttle a request
//! across the subprocess boundary and back. What's ported is the
//! *observable contract* they together implement: a persistent cookie
//! jar, browser-level-header-plus-per-request-header merging (with
//! same-named headers combined into one comma-joined line, matching
//! `QNetworkRequest::setRawHeader`'s own merge-on-repeat behavior),
//! redirect-following with the real final URL, a per-request timeout,
//! and a `worth_retry` flag on failure. [`browser`]'s own `mod tests`
//! verifies this contract against a hand-rolled HTTP test server, the
//! same scenarios upstream's own `test_fetch_backend.py` exercises.
//!
//! # Not ported: the WebEngine (JS-rendering) backend
//!
//! `webengine_backend.py` drives a real `QWebEnginePage` (Chromium) to
//! fetch resources from *within* a rendered page's JS execution
//! context -- needed for sites whose anti-scraping measures require a
//! real browser fingerprint (cookies/headers alone aren't enough).
//! There is no `reqwest` (or any other) equivalent for that: doing it
//! for real needs an embedded JS-capable rendering engine, a genuinely
//! new class of dependency this workspace doesn't have anywhere yet
//! (the same class of gap as `calibre_db::catalogs::thumbnails`'
//! disclosed `generate_masthead_image` skip). [`browser::WebEngineBrowser`]
//! is a thin wrapper around [`browser::Browser`] rather than a distinct
//! JS-rendering implementation, so it does real HTTP fetching -- the
//! *plain-HTTP* part of upstream's own `test_fetch_backend.py` contract
//! (`TestFetchBackend.test_recipe_browser_webengine` exercises the exact
//! same non-JS HTTP scenarios as the plain `Browser` test) -- but never
//! executes JavaScript. Callers relying on JS-rendered content will get
//! the pre-render HTML instead of silently getting nothing.
//!
//! `simple.py`'s `read_url` is ported as [`simple::read_url`]/
//! [`simple::read_url_bytes`].

pub mod browser;
pub mod simple;

pub use browser::{Browser, BrowserError, BrowserResponse, OpenOptions, RequestBody, WebEngineBrowser};
pub use simple::{read_url, read_url_bytes, BrowserCache};
