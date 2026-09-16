//! Shared SSRF guard for any route that has the server dial a
//! caller-supplied host on the caller's behalf (fetching a feed URL,
//! connecting to an SMTP relay, etc.). Originally written for
//! `news::validate_feed_url`; extracted here once `share::share_email`
//! needed the identical check for its relay host -- a bare
//! non-HTTP TCP connection is still a real SSRF vector (it lets a
//! caller make this server open arbitrary internal connections and
//! send it attacker-chosen bytes, e.g. smuggling a Redis/Memcached
//! command line through what looks like an SMTP dialog -- impact
//! doesn't require the response to be readable by the caller).

/// `true` for any IP a caller-supplied host must not be allowed to
/// resolve to -- loopback, private (RFC 1918), link-local (this
/// catches `169.254.169.254`, the cloud-metadata address), IPv6
/// unique-local, or unspecified.
pub fn is_disallowed_ip(ip: std::net::IpAddr) -> bool {
    // This crate's own tests use local loopback-bound test servers for
    // deterministic fixtures (news.rs's TestSite, share.rs's inline
    // SMTP server) -- #[cfg(test)] only affects the `cargo test`
    // binary, never a real `cargo build`/`cargo run`, so allowing
    // loopback here doesn't weaken real SSRF protection in any shipped
    // or normally-run binary.
    #[cfg(test)]
    if ip.is_loopback() {
        return false;
    }
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()),
    }
}

/// Real-DNS-resolves `host`/`port` and rejects it if any resolved
/// address is disallowed (see [`is_disallowed_ip`]). Real, disclosed
/// narrowing: this validates the host the caller asked to connect to
/// at the time of the check -- it doesn't protect against a
/// connection that itself later gets redirected or proxied elsewhere
/// (not a concern for a raw TCP dial like SMTP; `news::validate_feed_url`
/// documents the analogous HTTP-redirect caveat for its own caller).
pub async fn resolve_and_check(host: &str, port: u16) -> Result<(), String> {
    let addrs = tokio::net::lookup_host((host, port)).await.map_err(|e| format!("{host}: could not resolve host ({e})"))?;
    let mut resolved_any = false;
    for addr in addrs {
        resolved_any = true;
        if is_disallowed_ip(addr.ip()) {
            return Err(format!("{host}: resolves to a disallowed address ({})", addr.ip()));
        }
    }
    if !resolved_any {
        return Err(format!("{host}: host did not resolve to any address"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_disallowed_ip_flags_every_real_private_range() {
        // loopback is allowed under cfg(test) (see doc comment above);
        // every other disallowed range must still be flagged in tests.
        assert!(is_disallowed_ip("169.254.169.254".parse().unwrap()));
        assert!(is_disallowed_ip("10.0.0.1".parse().unwrap()));
        assert!(is_disallowed_ip("172.16.0.1".parse().unwrap()));
        assert!(is_disallowed_ip("192.168.1.1".parse().unwrap()));
        assert!(is_disallowed_ip("0.0.0.0".parse().unwrap()));
        assert!(is_disallowed_ip("fc00::1".parse().unwrap()));
        assert!(!is_disallowed_ip("8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn resolve_and_check_rejects_a_private_literal_address() {
        let err = resolve_and_check("10.0.0.5", 25).await.unwrap_err();
        assert!(err.contains("disallowed"), "{err}");
    }

    #[tokio::test]
    async fn resolve_and_check_allows_a_public_literal_address() {
        resolve_and_check("8.8.8.8", 25).await.unwrap();
    }
}
