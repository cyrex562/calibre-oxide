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
pub mod manifest;
pub mod store;

pub use host::{LoadedPlugin, PluginPackage, WasmPluginError};
pub use manifest::{Capabilities, Limits, Manifest, PluginType, ABI_VERSION};
pub use file_type::WasmFileTypePlugin;
pub use store::{PluginStore, StoreError};
