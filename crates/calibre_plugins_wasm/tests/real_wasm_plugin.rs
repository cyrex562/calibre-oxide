//! Executes a **real** WASM plugin through the host (issue #798).
//!
//! The unit tests in `src/host.rs` cover package/manifest rejection
//! paths without running any code. These run actual WebAssembly:
//! `tests/fixtures/probe_plugin.wasm`, built from
//! `tests/fixtures/probe_plugin/` by `tests/fixtures/build.sh`.
//!
//! The `.wasm` is checked in so this suite does not require a WASM
//! toolchain; its full source and build script are checked in beside
//! it so it stays reproducible and auditable rather than being an
//! opaque binary.

use std::io::Write;
use std::path::{Path, PathBuf};

use calibre_plugins_wasm::host::{PluginPackage, WasmPluginError};
use calibre_plugins_wasm::manifest::ABI_VERSION;

fn probe_wasm() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe_plugin.wasm");
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e} -- run tests/fixtures/build.sh", path.display()))
}

/// Packages the real probe plugin with a caller-chosen capabilities and
/// limits block, so each test can vary only the sandbox policy.
fn package_probe(dir: &Path, extra_manifest: &str) -> PathBuf {
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "Probe Plugin", "version": "1.0.0",
             "author": "calibre-oxide tests", "description": "exercises the sandbox",
             "plugin_type": "file_type", "wasm": "probe_plugin.wasm", "file_types": ["txt"]{extra_manifest}}}"#
    );

    let path = dir.join("probe.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("probe_plugin.wasm", opts).unwrap();
    zip.write_all(&probe_wasm()).unwrap();
    zip.finish().unwrap();
    path
}

#[test]
fn a_real_wasm_plugin_installed_from_a_file_really_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg_path = package_probe(tmp.path(), "");

    let pkg = PluginPackage::read_zip(&pkg_path).unwrap();
    assert_eq!(pkg.manifest.name, "Probe Plugin");

    let mut plugin = pkg.load().unwrap();
    let out = plugin.call("echo", b"round trip through real webassembly").unwrap();
    assert_eq!(out, b"round trip through real webassembly");
}

#[test]
fn a_real_wasm_plugin_really_computes_rather_than_echoing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut plugin = PluginPackage::read_zip(&package_probe(tmp.path(), "")).unwrap().load().unwrap();

    let out = plugin.call("uppercase", b"hello from the host").unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "HELLO FROM THE HOST");
}

#[test]
fn a_trapping_plugin_is_contained_and_the_host_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let mut plugin = PluginPackage::read_zip(&package_probe(tmp.path(), "")).unwrap().load().unwrap();

    let err = plugin.call("boom", b"").unwrap_err();
    assert!(matches!(err, WasmPluginError::Call { .. }), "a trap must surface as a Call error, got: {err}");

    // The host is still usable afterwards -- containment, not just a
    // caught error on the way down.
    let out = plugin.call("echo", b"still alive").unwrap();
    assert_eq!(out, b"still alive");
}

#[test]
fn an_infinite_loop_is_killed_by_the_declared_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    // A deliberately tiny budget so the test is fast; the point is that
    // *some* real wall-clock bound is enforced.
    let pkg_path = package_probe(tmp.path(), r#", "limits": {"timeout_ms": 300, "max_pages": 256}"#);
    let mut plugin = PluginPackage::read_zip(&pkg_path).unwrap().load().unwrap();

    let started = std::time::Instant::now();
    let err = plugin.call("spin", b"").unwrap_err();
    let elapsed = started.elapsed();

    assert!(matches!(err, WasmPluginError::Call { .. }), "a timeout must surface as a Call error, got: {err}");
    assert!(elapsed < std::time::Duration::from_secs(20), "the plugin should have been killed promptly, took {elapsed:?}");
}

#[test]
fn network_access_is_denied_when_the_manifest_declares_no_allowed_hosts() {
    let tmp = tempfile::tempdir().unwrap();
    // No `capabilities` block at all -- the default-deny case.
    let mut plugin = PluginPackage::read_zip(&package_probe(tmp.path(), "")).unwrap().load().unwrap();

    let err = plugin.call("fetch", b"").unwrap_err();
    assert!(
        matches!(err, WasmPluginError::Call { .. }),
        "a plugin that never declared allowed_hosts must not be able to reach the network, got: {err}"
    );
}

#[test]
fn a_fully_sandboxed_plugin_reports_itself_as_such_before_being_loaded() {
    // #801's UI needs to show what a package wants *before* the user
    // installs it, so this must be readable without instantiating.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = PluginPackage::read_zip(&package_probe(tmp.path(), "")).unwrap();
    assert!(pkg.manifest.capabilities.is_fully_sandboxed());

    let pkg = PluginPackage::read_zip(&package_probe(tmp.path(), r#", "capabilities": {"allowed_hosts": ["example.com"]}"#)).unwrap();
    assert!(!pkg.manifest.capabilities.is_fully_sandboxed());
    assert_eq!(pkg.manifest.capabilities.allowed_hosts, ["example.com"]);
}

#[test]
fn calling_a_function_the_plugin_does_not_export_is_an_error_not_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let mut plugin = PluginPackage::read_zip(&package_probe(tmp.path(), "")).unwrap().load().unwrap();

    let err = plugin.call("no_such_export", b"").unwrap_err();
    assert!(matches!(err, WasmPluginError::Call { .. }));
}
