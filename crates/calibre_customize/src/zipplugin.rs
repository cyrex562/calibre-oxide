//! Superseded by the real WASM plugin host (issue #798).
//!
//! This module used to be a 17-line stub whose `load_from_zip` did
//! nothing and returned `Ok(())`, with the comment *"In Rust, dynamic
//! loading is harder/different."* Real runtime plugin loading now
//! exists, in the `calibre_plugins_wasm` crate:
//!
//! - `calibre_plugins_wasm::PluginPackage::read_zip` reads and
//!   validates a plugin package.
//! - `calibre_plugins_wasm::PluginStore` installs, lists and removes
//!   them on disk.
//!
//! It lives in its own crate rather than here because Extism pulls in
//! wasmtime (~300 transitive crates); putting that in this crate would
//! push a WASM runtime into `calibre_ebooks` and, through it, into
//! eight further crates that have no use for one.
//!
//! Nothing is re-exported from here: the stub had no callers, and a
//! pass-through would only obscure where the real implementation
//! lives.
