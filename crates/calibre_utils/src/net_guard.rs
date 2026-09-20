//! Shared SSRF guard (generalized here for issue #800).
//!
//! # Why this moved
//!
//! This logic started in `calibre_srv::net_guard`, written for
//! `news::validate_feed_url` and then reused by `share::share_email`
//! and `metadata_search::cover_proxy`. Issue #800 added a fourth
//! consumer that `calibre_srv` cannot reach: the WASM plugin host's
//! HTTP host function, in `calibre_plugins_wasm`, which must apply the
//! identical check so a sandboxed plugin cannot be used as an SSRF
//! proxy into the host's network.
//!
//! Rather than duplicating a security check -- the worst thing to have
//! two copies of -- the predicate lives here and every consumer
//! delegates. `calibre_srv::net_guard` keeps its own async wrapper and
//! its existing API, unchanged for its callers. Same "generalize, then
//! delegate" shape used when `covers.rs`'s template engine became
//! `calibre_utils::formatter::string_format` for issue #751.
//!
//! # Why a blocking resolver lives here too
//!
//! `calibre_srv`'s version is async (`tokio::net::lookup_host`), which
//! suits an axum handler. Extism host functions are synchronous and
//! run on a thread with no reactor, so the plugin host needs a
//! blocking resolve. Both share [`is_disallowed_ip`], which is where
//! the actual policy lives.
//!
//! The policy here is deliberately exception-free; see
//! [`is_disallowed_ip`] for why each consumer layers its own
//! test-only loopback allowance instead.

use std::net::{IpAddr, ToSocketAddrs};

/// `true` for any IP a caller-supplied host must not be allowed to
/// resolve to -- loopback, private (RFC 1918), link-local (this
/// catches `169.254.169.254`, the cloud-metadata address), IPv6
/// unique-local, or unspecified.
///
/// **Strict, with no exceptions.** Consumers whose own test suites bind
/// real loopback servers as fixtures layer their own allowance on top
/// (`calibre_srv::net_guard` keeps a `#[cfg(test)]` shortcut;
/// `calibre_plugins_wasm` uses an explicit opt-in because `cfg(test)`
/// is not set for a library when its *integration* tests compile).
/// Keeping this function exception-free means the shared policy cannot
/// be accidentally weakened for a shipped binary.
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified())
        }
    }
}

/// Blocking counterpart of `calibre_srv::net_guard::resolve_and_check`:
/// resolves `host`/`port` and rejects it if **any** resolved address is
/// disallowed.
///
/// Rejecting when *any* address is disallowed (rather than when all
/// are) is deliberate: a host that resolves to both a public and an
/// internal address must not be reachable, since which one gets dialed
/// is not under this code's control.
pub fn resolve_and_check_blocking_with(host: &str, port: u16, disallowed: impl Fn(IpAddr) -> bool) -> Result<(), String> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| format!("{host}: could not resolve host ({e})"))?;
    let mut resolved_any = false;
    for addr in addrs {
        resolved_any = true;
        if disallowed(addr.ip()) {
            return Err(format!("{host}: resolves to a disallowed address ({})", addr.ip()));
        }
    }
    if !resolved_any {
        return Err(format!("{host}: host did not resolve to any address"));
    }
    Ok(())
}

/// [`resolve_and_check_blocking_with`] using the strict
/// [`is_disallowed_ip`] policy.
pub fn resolve_and_check_blocking(host: &str, port: u16) -> Result<(), String> {
    resolve_and_check_blocking_with(host, port, is_disallowed_ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cloud_metadata_address_is_disallowed() {
        assert!(is_disallowed_ip("169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn private_and_unspecified_ranges_are_disallowed() {
        for ip in ["10.0.0.1", "192.168.1.1", "172.16.0.1", "0.0.0.0"] {
            assert!(is_disallowed_ip(ip.parse().unwrap()), "{ip} should be disallowed");
        }
    }

    #[test]
    fn ipv6_unique_local_and_mapped_private_are_disallowed() {
        assert!(is_disallowed_ip("fd00::1".parse().unwrap()));
        assert!(is_disallowed_ip("::ffff:10.0.0.1".parse().unwrap()), "an IPv4-mapped private address must not sneak through");
        assert!(is_disallowed_ip("::ffff:169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn a_public_address_is_allowed() {
        assert!(!is_disallowed_ip("93.184.216.34".parse().unwrap()));
        assert!(!is_disallowed_ip("2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()));
    }

    #[test]
    fn loopback_is_disallowed_by_the_shared_policy_with_no_exceptions() {
        // The shared predicate must never make an exception; consumers
        // layer their own test allowance on top, visibly.
        assert!(is_disallowed_ip("127.0.0.1".parse().unwrap()));
        assert!(is_disallowed_ip("::1".parse().unwrap()));
    }

    #[test]
    fn a_caller_supplied_policy_can_permit_loopback_without_weakening_the_rest() {
        let permissive = |ip: std::net::IpAddr| !ip.is_loopback() && is_disallowed_ip(ip);
        assert!(!permissive("127.0.0.1".parse().unwrap()));
        assert!(permissive("169.254.169.254".parse().unwrap()), "opting into loopback must not permit cloud metadata");
        assert!(permissive("10.0.0.1".parse().unwrap()));
    }
}
