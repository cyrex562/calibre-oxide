//! Sandboxed WASM plugin host for calibre-oxide (issue #798, part of
//! the #754 plugin epic).
//!
//! # Why this is its own crate
//!
//! Extism pulls in wasmtime -- roughly 300 transitive crates. Putting
//! it in `calibre_customize` would push a WASM runtime into
//! `calibre_ebooks` and, through it, into eight further crates
//! (including `calibre_scraper_worker` and `calibre_devices`) that have
//! no business carrying one. Keeping the host in its own crate means
//! only a consumer that actually wants third-party plugins pays for it.
//!
//! # What this crate does and does not do
//!
//! It owns the package format ([`manifest`]), the sandbox, and the
//! bytes-in/bytes-out execution primitive ([`host`]). It does **not**
//! define any *typed* plugin ABI: the file-type transform ABI is issue
//! #799 and the metadata-source ABI is #800, each layered on top of
//! [`host::LoadedPlugin::call`]. That split keeps the security-critical
//! surface small and lets each ABI evolve without touching it.
//!
//! # Relationship to existing calibre plugins
//!
//! None. Upstream's ecosystem is Python that imports calibre internals
//! directly; those plugins cannot run here and never will. This is a
//! new ecosystem with the same hook shapes and a real sandbox.

pub mod file_type;
pub mod host;
pub mod metadata_source;
pub mod manifest;
pub mod store;

pub use host::{LoadedPlugin, PluginPackage, WasmPluginError};
pub use manifest::{Capabilities, Limits, Manifest, PluginType, ABI_VERSION};
pub use file_type::WasmFileTypePlugin;
pub use metadata_source::WasmMetadataSource;
pub use store::{PluginStore, StoreError};

use std::sync::atomic::{AtomicBool, Ordering};

static ALLOW_LOOPBACK: AtomicBool = AtomicBool::new(false);

/// Permits loopback for this crate's own integration tests, which bind
/// a real local HTTP server as a fixture.
///
/// This is a runtime opt-in rather than a `#[cfg(test)]` for a real
/// reason: `cfg(test)` is **not** set for a library when its
/// *integration* tests (`tests/*.rs`) compile, so a `cfg(test)` here
/// would silently fail to apply exactly where it is needed. Making it
/// an explicit call also keeps the weakening visible at the call site.
///
/// Never call this from shipped code.
pub fn allow_loopback_for_tests() {
    ALLOW_LOOPBACK.store(true, Ordering::Relaxed);
}

/// The SSRF policy the plugin HTTP host function applies: the shared
/// strict policy from `calibre_utils`, plus this crate's test-only
/// loopback allowance.
pub(crate) fn loopback_aware_disallowed_ip(ip: std::net::IpAddr) -> bool {
    if ALLOW_LOOPBACK.load(Ordering::Relaxed) && ip.is_loopback() {
        return false;
    }
    calibre_utils::net_guard::is_disallowed_ip(ip)
}
