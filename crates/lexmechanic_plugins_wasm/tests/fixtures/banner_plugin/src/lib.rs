//! A realistic third-party file-type plugin (issue #799).
//!
//! Stamps a banner onto an imported `.txt` file. Deliberately shaped
//! like a real plugin an outside author would write -- it implements
//! only the documented `run_file_type` export, uses no host
//! capabilities at all, and does real work on the content it is given.

use extism_pdk::*;

/// The file-type transform ABI: content in, transformed content out.
/// The host does the reading and writing; a sandboxed plugin has no
/// filesystem.
#[plugin_fn]
pub fn run_file_type(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let text = String::from_utf8_lossy(&input);
    let mut out = String::from("=== PROCESSED BY BANNER PLUGIN ===\n");
    out.push_str(&text);
    Ok(out.into_bytes())
}
