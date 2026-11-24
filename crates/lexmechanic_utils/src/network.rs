use std::net::{IpAddr, ToSocketAddrs};

pub fn is_ipv6_addr(addr: &str) -> bool {
    if let Ok(ip) = addr.parse::<IpAddr>() {
        return ip.is_ipv6();
    }
    false
}

pub fn format_addr_for_url(addr: &str) -> String {
    if is_ipv6_addr(addr) {
        format!("[{}]", addr)
    } else {
        addr.to_string()
    }
}

pub fn internet_connected() -> bool {
    // Basic check: try to resolve a known host?
    // Or just return true as "Dummy" implementation for now if we want to avoid DBus deps right now.
    // The python code tries DBus for NetworkManager.
    // Let's assume connected for now to match Dummy behaviour if we don't want to add huge deps.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv6() {
        assert!(is_ipv6_addr("::1"));
        assert!(!is_ipv6_addr("127.0.0.1"));
        assert!(!is_ipv6_addr("invalid"));
    }

    #[test]
    fn test_format() {
        assert_eq!(format_addr_for_url("::1"), "[::1]");
        assert_eq!(format_addr_for_url("127.0.0.1"), "127.0.0.1");
    }
}
