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
//! - [`opds`]: the OPDS catalog (`opds.py`), scoped to the root feed,
//!   built-in title/newest acquisition feeds, category/categorygroup
//!   browsing (tags/authors/series/publisher/languages), and full
//!   query-language search -- see that module's doc for what's
//!   deferred (multi-library).
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
//! - [`ajax`]: the read-only JSON REST API (`ajax.py`) -- book
//!   metadata, category browsing, and search as JSON instead of Atom
//!   -- see that module's doc for what's narrowed relative to
//!   upstream's full `field_metadata`-driven version.
//! - [`cdb`]: a subset of the write/mutation JSON API (`cdb.py`) --
//!   add-book, delete-books, set-cover, set-fields (metadata/cover/
//!   format edits), copy-to-library (issue #425, real two-library
//!   copy/move built on #423's multi-library support), the generic
//!   `calibredb`-CLI-over-HTTP dispatcher (issue #426, narrowed to a
//!   representative subset of commands) -- see that module's own doc
//!   for what's narrowed in each.
//! - [`books`]: cross-device reading-position sync (a narrow slice of
//!   `books.py`) -- `book-get/set-last-read-position` only; the rest
//!   of that file is `render_book.py`'s in-browser EPUB rendering
//!   pipeline, not ported.
//!
//! - [`web_socket`]: real-time push notifications over `GET
//!   /web-socket` (`web_socket.py`, transport subsumed by `axum`'s
//!   native WebSocket support) -- see that module's own doc for why
//!   this is wired up differently than upstream's own incomplete
//!   implementation.
//! - [`fts`]: full-text search over indexed book content (`fts.py`),
//!   backed by `calibre_db`'s already-ported FTS5 engine -- see that
//!   module's own doc.
//! - [`notes`]: the "Notes" feature -- free-form HTML notes on a
//!   category item, with embedded image resources (a subset of
//!   `content.py`), backed by `calibre_db`'s already-ported notes
//!   engine -- see that module's own doc.
//! - [`data_files`]: arbitrary files attached to a book outside its
//!   standard formats (another subset of `content.py`, issue #418),
//!   backed by `calibre_db::extra_files` -- see that module's own doc.
//! - [`reader_profiles`]: named per-user in-browser-reader settings
//!   (another subset of `content.py`, issue #419) -- see that
//!   module's own doc.
//! - [`bonjour`]: mDNS/Bonjour advertisement of the running OPDS
//!   catalog (`bonjour.py`, issue #420), started from `main.rs` when
//!   `ServerOptions::use_bonjour` is set -- see that module's own doc.
//!
//! `manage_users_cli.py` is partially covered by this crate's own
//! `main.rs` binary directly (`--add-user`/`--remove-user`/
//! `--list-users`/`--set-readonly`/`--change-password` convenience
//! flags around `users::UserManager`), not this library crate itself
//! -- see `main.rs`'s own doc for exactly what's covered and what
//! isn't (`change_set_password`, `libraries`).
//!
//! - [`jobs`]: background job management (`jobs.py`, issue #428) --
//!   real bounded-concurrency task tracking with status polling and
//!   best-effort abort, built on real `tokio` tasks rather than
//!   upstream's forked-subprocess model -- see that module's own doc
//!   for the disclosed consequence of that substitution. Not yet
//!   wired into [`AppState`]/any route: neither of its two real
//!   upstream consumers (`books.py`'s render-book job, `convert.py`'s
//!   server-side conversion) is ported yet, and upstream itself never
//!   exposed `jobs.py`'s own test helpers (`sleep_test`/`error_test`)
//!   over HTTP either -- inventing a synthetic endpoint here would be
//!   API surface upstream never had, not a real port.
//!
//! - [`library_broker`]: a pool of opened libraries keyed by
//!   `library_id` (`library_broker.py`'s base `LibraryBroker` class,
//!   issue #423, first slice), wired into [`AppState::libraries`] +
//!   [`AppState::cache_for`] and threaded through [`content::get`] and
//!   [`cdb::copy_to_library`] (issue #425) as real end-to-end
//!   demonstrations -- every other handler still reads
//!   [`AppState::cache`] directly and ignores its own `library_id`
//!   path segment (`AppState::libraries` defaults to `None`, so every
//!   pre-#423 single-library test and call site is unaffected).
//!   Migrating the rest of the handlers to real `library_id` routing,
//!   and exposing `library_map` for real (`ajax::library_info`'s
//!   hardcoded single entry, OPDS per-library nav entries) are
//!   separate follow-ups -- see that module's own doc.
//!
//! **Deferred to future increments** (not started):
//! `render_book.py` (the in-browser EPUB reader), `convert.py`
//! (server-side conversion), `auto_reload.py` (dev-mode auto-restart),
//! `legacy.py` (the old pre-content-server API),
//! `standalone.py`/`embedded.py` (process-management entry points --
//! this increment has its own minimal `main.rs` instead).

pub mod ajax;
pub mod auth;
pub mod bonjour;
pub mod books;
pub mod cdb;
pub mod content;
pub mod data_files;
pub mod errors;
pub mod fts;
pub mod jobs;
pub mod library_broker;
pub mod notes;
pub mod opds;
pub mod opts;
pub mod reader_profiles;
pub mod users;
pub mod users_api;
pub mod utils;
pub mod web_socket;

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
    /// Broadcasts [`web_socket::ChangeEvent`]s to every connected
    /// `/web-socket` client -- see that module's doc.
    pub changes: web_socket::ChangeBroadcaster,
    /// Per-user in-browser-reader settings -- see
    /// [`reader_profiles::ProfileStore`]'s own doc.
    pub reader_profiles: Arc<reader_profiles::ProfileStore>,
    /// The full pool of opened libraries (issue #423, first slice) --
    /// `None` means single-library mode: every handler falls back to
    /// [`AppState::cache`] regardless of any `library_id` it was
    /// given, exactly matching this crate's pre-#423 behavior. `Some`
    /// libraries are addressed by id via [`AppState::cache_for`];
    /// `cache` itself still names the default library either way, so
    /// existing single-library call sites that never call
    /// `cache_for` keep working unchanged.
    pub libraries: Option<Arc<library_broker::LibraryBroker>>,
}

impl AppState {
    /// The library for `library_id`, or the default library
    /// ([`AppState::cache`]) when `library_id` is `None`/empty, or
    /// [`AppState::libraries`] is `None` (single-library mode).
    /// `None` only when `library_id` names a library that isn't in
    /// [`AppState::libraries`].
    pub fn cache_for(&self, library_id: Option<&str>) -> Option<Arc<Cache>> {
        match &self.libraries {
            None => Some(self.cache.clone()),
            Some(broker) => broker.get(library_id),
        }
    }
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
        .route("/opds/category/{category}/{which}", get(opds::category))
        .route("/opds/categorygroup/{category}/{which}", get(opds::categorygroup))
        .route("/opds/search/{query}", get(opds::search))
        .route("/get/{what}/{book_id}/{library_id}", get(content::get))
        .route("/get/{what}/{book_id}", get(content::get_no_library))
        .route("/users/change-pw", post(users_api::change_pw))
        .route("/ajax/book/{book_id}", get(ajax::book))
        .route("/ajax/books", get(ajax::books))
        .route("/ajax/categories", get(ajax::categories_list))
        .route("/ajax/category/{name}", get(ajax::category))
        .route("/ajax/books_in/{category}/{item_id}", get(ajax::books_in))
        .route("/ajax/search", get(ajax::search))
        .route("/ajax/library-info", get(ajax::library_info))
        .route("/cdb/add-book/{job_id}/{add_duplicates}/{filename}/{library_id}", post(cdb::add_book))
        .route("/cdb/delete-books/{book_ids}", post(cdb::delete_books))
        .route("/cdb/set-cover/{book_id}", post(cdb::set_cover))
        .route("/cdb/set-fields/{book_id}", post(cdb::set_fields))
        .route("/cdb/copy-to-library/{target_library_id}/{library_id}", post(cdb::copy_to_library))
        .route("/cdb/copy-to-library/{target_library_id}", post(cdb::copy_to_library_no_source))
        .route("/cdb/cmd/{which}/{version}", get(cdb::cmd).post(cdb::cmd))
        .route("/book-get-last-read-position/{library_id}/{which}", get(books::get_last_read_position))
        .route("/book-set-last-read-position/{library_id}/{book_id}/{fmt}", post(books::set_last_read_position))
        .route("/web-socket", get(web_socket::upgrade))
        .route("/fts/search", get(fts::search))
        .route("/fts/disable", post(fts::disable))
        .route("/fts/reindex", post(fts::reindex))
        .route("/fts/indexing", post(fts::indexing))
        .route("/fts/snippets/{book_ids}", get(fts::snippets))
        .route("/get-note/{field}/{item_id}/{library_id}", get(notes::get_note))
        .route("/get-note-from-item-val/{field}/{item}/{library_id}", get(notes::get_note_from_val))
        .route("/get-note-resource/{scheme}/{digest}/{library_id}", get(notes::get_note_resource))
        .route("/set-note/{field}/{item_id}/{library_id}", post(notes::set_note))
        .route("/data-files/get/{book_id}/{*relpath}", get(data_files::get))
        .route("/data-files/upload/{book_id}/{library_id}", post(data_files::upload))
        .route("/data-files/remove/{book_id}/{library_id}", post(data_files::remove))
        .route("/reader-profiles/get-all", get(reader_profiles::get_all))
        .route("/reader-profiles/save", post(reader_profiles::save));

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
