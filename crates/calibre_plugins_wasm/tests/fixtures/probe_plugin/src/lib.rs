//! The real WASM plugin used to prove the host in `calibre_plugins_wasm`
//! actually works (issue #798). One module exporting several functions,
//! each exercising one property the host must guarantee.
//!
//! Built by `../build.sh`; the resulting `probe_plugin.wasm` is checked
//! in so the host's tests don't require a WASM toolchain.

use extism_pdk::*;

/// Round-trip: proves bytes really cross into the sandbox and back.
#[plugin_fn]
pub fn echo(input: Vec<u8>) -> FnResult<Vec<u8>> {
    Ok(input)
}

/// Real work: proves the plugin computes rather than just echoing.
#[plugin_fn]
pub fn uppercase(input: String) -> FnResult<String> {
    Ok(input.to_uppercase())
}

/// Proves a trapping plugin is contained, not fatal to the host.
#[plugin_fn]
pub fn boom(_input: ()) -> FnResult<Vec<u8>> {
    panic!("this plugin deliberately traps");
}

/// Proves the timeout is real: loops until the host kills it.
#[plugin_fn]
pub fn spin(_input: ()) -> FnResult<Vec<u8>> {
    let mut n: u64 = 0;
    loop {
        n = n.wrapping_add(1);
        std::hint::black_box(n);
    }
}

/// Proves the network is denied by default: attempts an HTTP request to
/// a host the manifest did not allow.
#[plugin_fn]
pub fn fetch(_input: ()) -> FnResult<String> {
    let req = HttpRequest::new("https://example.com/");
    let res = http::request::<()>(&req, None)?;
    Ok(String::from_utf8_lossy(&res.body()).to_string())
}
