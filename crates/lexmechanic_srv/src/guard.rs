//! Browser-facing request guard: a `Host` check against DNS rebinding and an
//! `Origin` check against cross-site request forgery.
//!
//! The desktop app runs this server on `127.0.0.1` with no login, so the only
//! thing between the user's library and every web page they have open is the
//! browser's own rules. Those rules are weaker than they look:
//!
//! - A cross-site `<form>` or `fetch(..., {mode: "no-cors"})` can *send* a
//!   `POST` with a simple content type and no preflight. Several routes here
//!   (`/cdb/delete-books/..`, `/trash/empty/..`) take no body at all, so such a
//!   request is already a complete command.
//! - A page on `evil.example` can rebind that name to `127.0.0.1` and then
//!   read responses, because to the browser it is same-origin.
//!
//! Hence two checks, applied to every route (static files and `/mathjax`
//! included):
//!
//! 1. **Host.** The `Host` header must name this server: `localhost`, a
//!    loopback address, or the address it was told to listen on, with its own
//!    port. When listening on every interface (`--auth` on the LAN) any IP
//!    literal, single-label name or `.local` name is accepted, since the user
//!    reaches it by whatever address the LAN gives it -- but still never a
//!    public DNS name, which is what a rebinding attack needs.
//! 2. **Origin.** For anything but `GET`/`HEAD`/`OPTIONS` (and for WebSocket
//!    upgrades), an `Origin` header, when present, must be this server's own
//!    origin or one of the Tauri webview origins. `Origin: null` is refused.
//!    A request with no `Origin` is not a browser cross-site request (`curl`,
//!    the app's own Rust client) and passes.
//!
//! A missing `Host` is let through: HTTP/1.1 clients always send one, and
//! requiring it would only break in-process tests.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::opts::ServerOptions;
use crate::AppState;

/// Origins the Tauri webview reports for its own pages, per platform.
const TAURI_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];

/// The interface the server binds when `--listen-on` is not given: loopback,
/// unless `--auth` is on, in which case the user has chosen to expose it.
pub fn effective_listen_on(opts: &ServerOptions) -> String {
    match &opts.listen_on {
        Some(addr) => addr.clone(),
        None if opts.auth => "0.0.0.0".to_string(),
        None => "127.0.0.1".to_string(),
    }
}

/// True when `addr` (an `effective_listen_on` value) only accepts connections
/// from this machine. Network-facing extras (mDNS) stay off for such servers.
pub fn is_loopback_listen(addr: &str) -> bool {
    let a = addr.trim().trim_start_matches('[').trim_end_matches(']');
    a.eq_ignore_ascii_case("localhost") || a.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Splits `host[:port]` (IPv6 literals in brackets) into a lowercase host and
/// an optional port.
fn split_authority(authority: &str) -> (String, Option<u16>) {
    let authority = authority.trim().to_ascii_lowercase();
    if let Some(rest) = authority.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
            return (format!("[{host}]"), port);
        }
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host.to_string(), port.parse().ok()),
        _ => (authority, None),
    }
}

fn is_loopback_name(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost") || host == "[::1]" || host == "127.0.0.1" || host.starts_with("127.")
}

fn is_ip_literal(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Whether `authority` (a `Host` value, or the authority part of an `Origin`)
/// names this server.
fn host_is_ours(opts: &ServerOptions, authority: &str) -> bool {
    let (host, port) = split_authority(authority);
    if port != Some(opts.port) {
        return false;
    }
    if is_loopback_name(&host) {
        return true;
    }
    let listen = effective_listen_on(opts).to_ascii_lowercase();
    let listen = if listen.contains(':') && !listen.starts_with('[') { format!("[{listen}]") } else { listen };
    if listen == "0.0.0.0" || listen == "[::]" {
        // Every interface: the LAN address is whatever the machine has.
        return is_ip_literal(&host) || !host.contains('.') || host.ends_with(".local");
    }
    host == listen
}

/// Why a request was refused, or `Ok`.
pub fn check(opts: &ServerOptions, method: &Method, headers: &HeaderMap) -> Result<(), &'static str> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if let Some(host) = host {
        if !host_is_ours(opts, host) {
            return Err("unrecognised Host header");
        }
    }

    let unsafe_method = !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    let is_upgrade = headers.get(header::UPGRADE).is_some();
    if !(unsafe_method || is_upgrade) {
        return Ok(());
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let Ok(origin) = origin.to_str() else {
        return Err("unreadable Origin header");
    };
    let origin = origin.trim().to_ascii_lowercase();
    if TAURI_ORIGINS.contains(&origin.as_str()) {
        return Ok(());
    }
    let Some(authority) = origin.strip_prefix("http://") else {
        // `null`, `https://...` (this server speaks plain http), anything else.
        return Err("cross-origin request refused");
    };
    // Same origin as the request's own (already validated) Host. On loopback,
    // `localhost` and `127.0.0.1` are interchangeable names for it.
    if host.is_some_and(|h| h.trim().eq_ignore_ascii_case(authority)) {
        return Ok(());
    }
    let loopback = |a: &str| {
        let (h, p) = split_authority(a);
        p == Some(opts.port) && is_loopback_name(&h)
    };
    if loopback(authority) && host.is_none_or(loopback) {
        return Ok(());
    }
    Err("cross-origin request refused")
}

/// Axum middleware wrapping [`check`].
pub async fn require_same_site(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match check(&state.opts, req.method(), req.headers()) {
        Ok(()) => next.run(req).await,
        Err(why) => (StatusCode::FORBIDDEN, why).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(port: u16, listen: Option<&str>, auth: bool) -> ServerOptions {
        ServerOptions { port, listen_on: listen.map(String::from), auth, ..ServerOptions::default() }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(header::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_foreign_origin_post_is_refused() {
        let o = opts(8080, Some("127.0.0.1"), false);
        let h = headers(&[("host", "127.0.0.1:8080"), ("origin", "http://evil.example")]);
        assert!(check(&o, &Method::POST, &h).is_err());
        let h = headers(&[("host", "127.0.0.1:8080"), ("origin", "null")]);
        assert!(check(&o, &Method::POST, &h).is_err());
        // Another port on loopback is another site.
        let h = headers(&[("host", "127.0.0.1:8080"), ("origin", "http://127.0.0.1:9999")]);
        assert!(check(&o, &Method::POST, &h).is_err());
    }

    #[test]
    fn a_foreign_host_is_refused_even_for_get() {
        let o = opts(8080, Some("127.0.0.1"), false);
        assert!(check(&o, &Method::GET, &headers(&[("host", "rebind.evil.example:8080")])).is_err());
        assert!(check(&o, &Method::GET, &headers(&[("host", "127.0.0.1:9")])).is_err());
    }

    #[test]
    fn same_origin_and_tauri_posts_are_allowed() {
        let o = opts(8080, Some("127.0.0.1"), false);
        for origin in ["http://127.0.0.1:8080", "http://localhost:8080", "tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"] {
            let h = headers(&[("host", "127.0.0.1:8080"), ("origin", origin)]);
            assert!(check(&o, &Method::POST, &h).is_ok(), "{origin}");
        }
    }

    #[test]
    fn get_without_origin_and_originless_post_are_allowed() {
        let o = opts(8080, Some("127.0.0.1"), false);
        assert!(check(&o, &Method::GET, &headers(&[("host", "localhost:8080")])).is_ok());
        assert!(check(&o, &Method::POST, &headers(&[("host", "localhost:8080")])).is_ok());
        assert!(check(&o, &Method::POST, &HeaderMap::new()).is_ok());
    }

    #[test]
    fn a_cross_site_websocket_upgrade_is_refused() {
        let o = opts(8080, Some("127.0.0.1"), false);
        let h = headers(&[("host", "127.0.0.1:8080"), ("upgrade", "websocket"), ("origin", "http://evil.example")]);
        assert!(check(&o, &Method::GET, &h).is_err());
    }

    #[test]
    fn lan_access_with_auth_keeps_working() {
        let o = opts(8080, None, true);
        let h = headers(&[("host", "192.168.1.20:8080"), ("origin", "http://192.168.1.20:8080")]);
        assert!(check(&o, &Method::POST, &h).is_ok());
        assert!(check(&o, &Method::GET, &headers(&[("host", "mybox.local:8080")])).is_ok());
        // ...but a public name is a rebinding candidate, and a LAN page cannot post cross-site.
        assert!(check(&o, &Method::GET, &headers(&[("host", "evil.example:8080")])).is_err());
        let h = headers(&[("host", "192.168.1.20:8080"), ("origin", "http://192.168.1.99:8080")]);
        assert!(check(&o, &Method::POST, &h).is_err());
    }

    #[test]
    fn a_specific_listen_address_is_the_only_non_loopback_host() {
        let o = opts(8080, Some("192.168.1.20"), true);
        assert!(check(&o, &Method::GET, &headers(&[("host", "192.168.1.20:8080")])).is_ok());
        assert!(check(&o, &Method::GET, &headers(&[("host", "192.168.1.21:8080")])).is_err());
    }

    #[test]
    fn listening_defaults_to_loopback_without_auth() {
        assert_eq!(effective_listen_on(&opts(1, None, false)), "127.0.0.1");
        assert_eq!(effective_listen_on(&opts(1, None, true)), "0.0.0.0");
        assert_eq!(effective_listen_on(&opts(1, Some("10.0.0.1"), false)), "10.0.0.1");
        assert!(is_loopback_listen("127.0.0.1"));
        assert!(is_loopback_listen("[::1]"));
        assert!(is_loopback_listen("localhost"));
        assert!(!is_loopback_listen("0.0.0.0"));
        assert!(!is_loopback_listen("10.0.0.1"));
    }
}
