//! Port of a subset of `calibre.srv.auth` -- HTTP authentication for
//! the content server.
//!
//! # HTTP Basic only, not Digest
//!
//! Upstream defaults to HTTP Digest authentication (`opts.auth_mode ==
//! 'auto'` picks Digest unless SSL is configured) specifically so
//! passwords never cross the wire even in plaintext form. This port
//! only implements Basic auth -- Digest's nonce-synthesis/validation
//! machinery (`synthesize_nonce`/`validate_nonce`/`DigestAuth`, built on
//! HMAC-like keyed hashing and stale-nonce/replay tracking) is real
//! complexity deferred to a future increment. **Operationally this
//! means credentials are sent in a trivially-decodable (base64, not
//! encrypted) form** -- exactly the scenario upstream's own docs already
//! call out ("If you care about this vulnerability, run the server
//! behind a reverse proxy that uses HTTPS", `AuthController`'s
//! docstring) as the correct mitigation regardless of Basic vs Digest,
//! since calibre's own threat model here is a private/LAN server, not a
//! primary line of defense on its own.
//!
//! # Also not ported
//!
//! The Android-cookie workaround (downloads on Android hand off to a
//! separate process that can't carry HTTP Auth headers, so upstream
//! issues a signed cookie for `/get` requests instead) and per-user
//! library restrictions (`UserManager`'s `restriction` column isn't
//! exposed by this port's `users.rs` yet either) are both deferred.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use indexmap::IndexMap;

use crate::errors::ServerError;
use crate::users::UserManager;
use crate::AppState;

/// Port of `BanList`: temporarily bans a key (here, the remote IP) after
/// repeated authentication failures. Disabled entirely (matching
/// upstream's own `if not self.interval or not self.max_failures_before_ban`
/// short-circuit) when either `ban_time_in_minutes` or
/// `max_failures_before_ban` is zero -- [`crate::opts::ServerOptions`]'s
/// own `ban_for`/`ban_after` default to `0`/`5`, so banning is off by
/// default, matching upstream.
pub struct BanList {
    interval: Duration,
    max_failures: u32,
    items: Option<Mutex<IndexMap<String, (Instant, u32)>>>,
}

impl BanList {
    pub fn new(ban_time_in_minutes: i64, max_failures_before_ban: i64) -> BanList {
        let interval = Duration::from_secs((ban_time_in_minutes.max(0) as u64) * 60);
        let max_failures = max_failures_before_ban.max(0) as u32;
        let items = if interval.is_zero() || max_failures == 0 { None } else { Some(Mutex::new(IndexMap::new())) };
        BanList { interval, max_failures, items }
    }

    /// Port of `is_banned`.
    pub fn is_banned(&self, key: &str) -> bool {
        let Some(items) = &self.items else { return false };
        let items = items.lock().unwrap();
        let Some(&(previous_fail, fail_count)) = items.get(key) else { return false };
        if fail_count < self.max_failures {
            return false;
        }
        previous_fail.elapsed() < self.interval
    }

    /// Port of `failed`: records a failed attempt for `key`, and prunes
    /// entries older than `interval` from the front (matching
    /// upstream's own `for old in reversed(self.items): ... else: break`
    /// early-stop, relying on insertion order approximating recency).
    pub fn failed(&self, key: &str) {
        let Some(items) = &self.items else { return };
        let mut items = items.lock().unwrap();
        let fail_count = items.shift_remove(key).map(|(_, c)| c).unwrap_or(0);
        let now = Instant::now();
        items.insert(key.to_string(), (now, fail_count + 1));
        let mut remove = Vec::new();
        for (old_key, (previous_fail, _)) in items.iter().rev() {
            if now.duration_since(*previous_fail) > self.interval {
                remove.push(old_key.clone());
            } else {
                break;
            }
        }
        for r in remove {
            items.shift_remove(&r);
        }
    }
}

/// Port of the Basic-auth-relevant slice of `AuthController`.
pub struct AuthGate {
    pub users: UserManager,
    pub realm: String,
    pub ban_list: BanList,
}

impl AuthGate {
    pub fn new(users: UserManager, realm: String, ban_for_minutes: i64, ban_after: i64) -> AuthGate {
        AuthGate { users, realm, ban_list: BanList::new(ban_for_minutes, ban_after) }
    }

    /// Port of `check`.
    fn check(&self, username: &str, password: &str) -> bool {
        !password.is_empty() && self.users.get(username).as_deref() == Some(password)
    }
}

fn decode_basic_credentials(rest: &str) -> Option<(String, String)> {
    let decoded = base64::engine::general_purpose::STANDARD.decode(rest.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (un, pw) = text.split_once(':')?;
    if un.is_empty() || pw.is_empty() {
        return None;
    }
    Some((un.to_string(), pw.to_string()))
}

/// Port of `AuthController.do_http_auth`, restricted to the Basic-auth
/// branch (see this module's doc). Applied as an `axum` middleware
/// layer over the whole router when [`crate::opts::ServerOptions::auth`]
/// is set -- see `lib.rs::router`.
///
/// [`BanList`] is keyed on the real remote IP via `axum`'s
/// [`ConnectInfo`] extractor, matching upstream's own
/// `data.remote_addr` key -- this requires the server to actually be
/// served with `Router::into_make_service_with_connect_info` (`main.rs`
/// does this); tests use `axum::extract::connect_info::MockConnectInfo`
/// instead (see this module's own tests).
pub async fn require_auth(State(state): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Result<Response, ServerError> {
    let Some(gate) = &state.auth else {
        return Ok(next.run(req).await);
    };
    let ban_key = addr.ip().to_string();
    if gate.ban_list.is_banned(&ban_key) {
        return Err(ServerError::Forbidden("Too many login attempts".to_string()));
    }

    let auth_header = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let Some(auth_header) = auth_header else {
        return Err(ServerError::Unauthorized);
    };
    let (scheme, rest) = auth_header.split_once(' ').unwrap_or((auth_header, ""));
    if !scheme.eq_ignore_ascii_case("basic") {
        return Err(ServerError::BadRequest("Unsupported authentication method".to_string()));
    }
    let Some((username, password)) = decode_basic_credentials(rest) else {
        return Err(ServerError::BadRequest("The username or password was empty or malformed".to_string()));
    };
    if gate.check(&username, &password) {
        // Port of `data.username = un` -- makes the authenticated user
        // available to downstream handlers (e.g. `users_api::change_pw`)
        // via the `AuthenticatedUser` request extension.
        req.extensions_mut().insert(AuthenticatedUser(username));
        return Ok(next.run(req).await);
    }
    gate.ban_list.failed(&ban_key);
    Err(ServerError::Unauthorized)
}

/// Port of `rd.username`: the username `require_auth` authenticated
/// this request as, available to handlers via `axum`'s `Extension`
/// extractor. Only present when [`require_auth`] actually ran and
/// succeeded (i.e. auth is enabled and credentials were valid).
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub String);

/// Not part of upstream's own public API surface (upstream's
/// per-request `Authorization` header check has no standalone helper
/// like this) -- exposed for tests that want to verify credential
/// encoding without going through a full HTTP round trip.
#[allow(dead_code)]
fn basic_auth_header(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opts::ServerOptions;
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state(auth_enabled: bool) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let cache = calibre_db::cache::Cache::new(dir.path()).unwrap();
        let auth = if auth_enabled {
            let users = UserManager::new(&dir.path().join("users.sqlite")).unwrap();
            users.add_user("alice", "hunter2", false).unwrap();
            Some(Arc::new(AuthGate::new(users, "calibre".to_string(), 0, 5)))
        } else {
            None
        };
        (dir, AppState { libraries: None, cache: Arc::new(cache), opts: Arc::new(ServerOptions::default()), auth, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: Arc::new(crate::books_cache::BookCache::open_temp()), jobs: Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: Arc::new(crate::convert::ConversionJobRegistry::new()) })
    }

    #[tokio::test]
    async fn requests_without_credentials_are_rejected_when_auth_is_enabled() {
        let (_dir, state) = test_state(true);
        let router = crate::test_router(state);
        let req = HttpRequest::builder().uri("/opds").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn requests_with_correct_credentials_succeed() {
        let (_dir, state) = test_state(true);
        let router = crate::test_router(state);
        let req = HttpRequest::builder().uri("/opds").header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2")).body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn requests_with_wrong_password_are_rejected() {
        let (_dir, state) = test_state(true);
        let router = crate::test_router(state);
        let req = HttpRequest::builder().uri("/opds").header(header::AUTHORIZATION, basic_auth_header("alice", "wrong")).body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn requests_succeed_with_no_credentials_when_auth_is_disabled() {
        let (_dir, state) = test_state(false);
        let router = crate::test_router(state);
        let req = HttpRequest::builder().uri("/opds").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn ban_list_bans_after_the_configured_number_of_failures() {
        let bl = BanList::new(1, 2);
        assert!(!bl.is_banned("1.2.3.4"));
        bl.failed("1.2.3.4");
        assert!(!bl.is_banned("1.2.3.4"));
        bl.failed("1.2.3.4");
        assert!(bl.is_banned("1.2.3.4"));
    }

    #[test]
    fn ban_list_is_a_no_op_when_disabled() {
        let bl = BanList::new(0, 5);
        bl.failed("1.2.3.4");
        bl.failed("1.2.3.4");
        bl.failed("1.2.3.4");
        bl.failed("1.2.3.4");
        bl.failed("1.2.3.4");
        assert!(!bl.is_banned("1.2.3.4"));
    }
}
