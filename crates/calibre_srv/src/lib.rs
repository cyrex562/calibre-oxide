//! Port of `old_src/src/calibre/srv/` (issue #60) -- calibre's Content
//! Server: a full HTTP/WebSocket server for browsing, reading, and
//! downloading a calibre library over the network (OPDS feeds for
//! e-readers, a browser-based book viewer, a REST API, user
//! management, and more).
//!
//! # This is a large, multi-increment port
//!
//! 36 files, ~12,300 lines. This crate is **not** a complete port of
//! `calibre.srv` -- it's the first of several planned increments. See
//! each module's own doc for exactly what it covers and what's
//! deliberately deferred; the GitHub issue tracks per-file status.
//!
//! # Architecture: `axum`/`tokio`, not a hand-rolled event loop
//!
//! Upstream implements its own async HTTP/1.1 server from scratch --
//! `loop.py` (an epoll/select event loop), `http_request.py`/
//! `http_response.py` (hand-written HTTP parsing/serialization),
//! `pool.py` (a worker-thread pool for handlers), and `web_socket.py`
//! (a hand-rolled WebSocket implementation) -- because calibre predates
//! mature async I/O in Python. `docs/AGENT_PORTING_GUIDE.md` already
//! names `tokio` as this workspace's async-I/O crate; `axum` (built on
//! `tokio`+`hyper`) is the natural routing/handler layer on top of it,
//! and both are dramatically more mature, better-tested, and better-
//! performing than a line-for-line port of calibre's own hand-rolled
//! networking code would be. **None of `loop.py`/`http_request.py`/
//! `http_response.py`/`pool.py`/`routes.py`/`web_socket.py` are ported
//! file-by-file** -- `axum::Router` replaces `routes.py`'s own URL-pattern
//! dispatch (which upstream's own `@endpoint('/opds/category/{category}/{which}')`
//! decorator syntax maps onto almost directly -- axum path params use
//! `:category`/`:which` instead of `{category}`/`{which}`), and
//! `axum`/`hyper`/`tokio` together replace the rest. Every other file's
//! actual *behavior* (auth logic, feed generation, book serving, etc.)
//! is what gets ported.
//!
//! # What's in this increment
//!
//! - [`opts`]: server configuration (`opts.py`, full port).
//! - [`errors`]: HTTP error types (`errors.py`, full port, as an
//!   `axum::IntoResponse` enum instead of an exception hierarchy).
//! - [`utils`]: pagination and HTTP date formatting (the still-relevant
//!   pure-logic slice of `utils.py` -- see that module's own doc for
//!   what's socket/logging plumbing this doesn't need).
//! - [`opds`]: the OPDS catalog (`opds.py`), scoped to the root feed and
//!   built-in title/newest acquisition feeds -- see that module's doc
//!   for what's deferred (categories, search, multi-library).
//! - [`content`]: book/cover/format downloads (the `/get` endpoint from
//!   `content.py`), scoped to what OPDS acquisition links need -- see
//!   that module's doc for what's deferred (etag/conditional-GET
//!   caching, thumbnail resizing, plugboard metadata transforms).
//!
//! - [`users`]: the user credential store (`users.py`'s `UserManager`,
//!   CRUD + password check only -- see that module's doc for what's
//!   deferred).
//! - [`auth`]: HTTP Basic authentication (`auth.py`, restricted to
//!   Basic -- Digest is deferred, see that module's doc), applied as a
//!   blanket `axum` middleware layer over every route in [`router`]
//!   when [`opts::ServerOptions::auth`] is set.
//! - [`users_api`]: `POST /users/change-pw` (`users_api.py`), letting an
//!   authenticated user change their own password.
//!
//! **Deferred to future increments** (not started): `ajax.py`/`cdb.py`
//! (the JSON REST API), `manage_users_cli.py` (a CLI wrapper around
//! `users.py` this crate's own binary doesn't expose yet),
//! `render_book.py` (the in-browser EPUB reader), `convert.py`
//! (server-side conversion), `fts.py` (full-text search), `jobs.py`
//! (background job management), `web_socket.py` (real-time UI updates),
//! `auto_reload.py` (dev-mode auto-restart), `bonjour.py` (mDNS
//! advertisement), `legacy.py` (the old pre-content-server API),
//! `library_broker.py` (multi-library support), `standalone.py`/
//! `embedded.py` (process-management entry points -- this increment has
//! its own minimal `main.rs` instead).

pub mod auth;
pub mod content;
pub mod errors;
pub mod opds;
pub mod opts;
pub mod users;
pub mod users_api;
pub mod utils;

use std::sync::Arc;

use calibre_db::cache::Cache;

/// Shared application state every handler receives via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    pub cache: Arc<Cache>,
    pub opts: Arc<opts::ServerOptions>,
    /// `None` means authentication is disabled -- every request is
    /// allowed through, matching upstream's own unauthenticated default
    /// (`opts.auth == False`).
    pub auth: Option<Arc<auth::AuthGate>>,
}

/// Builds the `axum::Router` for this increment's endpoints. When
/// `state.auth` is set, every route requires HTTP Basic credentials
/// (see `auth::require_auth`) -- there are no public/unauthenticated
/// routes in this increment (upstream exempts some, e.g. `/static`,
/// via each `@endpoint`'s own `auth_required=False`; nothing in this
/// increment needs that yet).
pub fn router(state: AppState) -> axum::Router {
    use axum::middleware;
    use axum::routing::{get, post};

    let api = axum::Router::new()
        .route("/opds", get(opds::root))
        .route("/opds/navcatalog/{which}", get(opds::navcatalog))
        .route("/get/{what}/{book_id}/{library_id}", get(content::get))
        .route("/get/{what}/{book_id}", get(content::get_no_library))
        .route("/users/change-pw", post(users_api::change_pw));

    api.route_layer(middleware::from_fn_with_state(state.clone(), auth::require_auth)).with_state(state)
}

/// [`router`], plus a mocked [`axum::extract::ConnectInfo`] so
/// `axum::Router::oneshot`-based tests (which never go through a real
/// TCP listener, so have no real connect info to extract) work with
/// `auth::require_auth`'s `ConnectInfo<SocketAddr>` extractor. Every
/// test in this crate that exercises the router through `require_auth`
/// should use this instead of calling [`router`] directly.
#[cfg(test)]
pub fn test_router(state: AppState) -> axum::Router {
    use axum::extract::connect_info::MockConnectInfo;
    use std::net::SocketAddr;
    router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
}
