//! Port of `old_src/src/calibre/scraper/` -- calibre's HTTP-fetching
//! "Browser" abstraction used by metadata-download plugins and news
//! recipes.
//!
//! # Two implementations, matching upstream's own split
//!
//! - [`browser::Browser`] (`qt.py`/`qt_backend.py`): plain HTTP,
//!   `reqwest::blocking`-based, in-process. Upstream's `Browser`
//!   doesn't do HTTP itself -- it spawns a Qt subprocess worker
//!   (`qt_backend.py`'s `FetchBackend`, built on
//!   `QNetworkAccessManager`) and talks to it over JSON-lines IPC. This
//!   workspace has no Qt dependency (`docs/AGENT_PORTING_GUIDE.md`
//!   names `reqwest` for network scraping instead), so `Browser` fetches
//!   directly, synchronously, matching the `reqwest::blocking`
//!   convention already established in `oeb::polish::download`. What's
//!   ported is the *observable contract* `qt.py`/`qt_backend.py`
//!   together implement (persistent cookies, header merging, redirects,
//!   timeouts with `worth_retry`), not the IPC machinery itself --
//!   there's nothing left to port once the transport moves in-process.
//!   See [`browser`]'s own doc and `mod tests` (verified against the
//!   same scenarios upstream's `test_fetch_backend.py` exercises).
//! - [`webengine_browser::WebEngineBrowser`] (`webengine_backend.py`):
//!   real JS execution, via a genuine OS-native webview. Unlike
//!   `Browser`, this **does** spawn a subprocess and talk to it over
//!   JSON-lines IPC -- `crates/calibre_scraper_worker`, a separate
//!   binary crate built on `wry`/`tao` (the same webview stack this
//!   workspace's own `app/src-tauri` Tauri desktop app depends on).
//!   Keeping that dependency in its own crate means `calibre_ebooks`
//!   itself never has to link a GUI/webview toolkit just to provide
//!   HTTP fetching -- it only needs to spawn a binary and speak JSON
//!   lines, mirroring upstream's own subprocess/IPC design almost
//!   exactly (a `Browser` spawning a worker process it exchanges
//!   commands with), just with a real OS webview standing in for
//!   `QWebEnginePage`. See [`webengine_browser`]'s own doc for the
//!   disclosed gaps versus `Browser` (no request bodies, no real
//!   response status/headers) and why they exist.
//!
//! `simple.py`'s `read_url` is ported as [`simple::read_url`]/
//! [`simple::read_url_bytes`].

pub mod browser;
pub mod simple;
pub mod webengine_browser;

pub use browser::{Browser, BrowserError, BrowserResponse, OpenOptions, RequestBody};
pub use simple::{read_url, read_url_bytes, BrowserCache};
pub use webengine_browser::WebEngineBrowser;
