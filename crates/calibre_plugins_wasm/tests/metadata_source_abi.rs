//! End-to-end proof of the metadata-source ABI (issue #800).
//!
//! The point of this suite is the **capability model**: a sandboxed
//! plugin has no ambient network, so the only way out is the host's
//! guarded HTTP function, and these tests show it granting access to a
//! declared host and refusing everything else.
//!
//! Uses `tests/fixtures/meta_plugin.wasm`, a realistic third-party
//! plugin that calls the host getter and maps the response into
//! candidates.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

use calibre_plugins_wasm::host::PluginPackage;
use calibre_plugins_wasm::manifest::ABI_VERSION;
use calibre_plugins_wasm::metadata_source::{MetadataAbiError, MetadataQuery, WasmMetadataSource};

/// A tiny HTTP server standing in for a real metadata API.
struct TestApi {
    addr: std::net::SocketAddr,
}

impl TestApi {
    fn start(body: &'static str) -> TestApi {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream: TcpStream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                        break;
                    }
                }
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        TestApi { addr }
    }

    fn host(&self) -> String {
        format!("{}", self.addr)
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e} -- run tests/fixtures/build.sh", path.display()))
}

fn package_meta(dir: &Path, tag: &str, allowed_hosts: &str) -> PathBuf {
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "Test Metadata Source", "version": "1.0.0",
             "author": "A Third Party", "description": "looks books up over the network",
             "plugin_type": "metadata_source", "wasm": "meta_plugin.wasm",
             "capabilities": {{"allowed_hosts": {allowed_hosts}}}}}"#
    );
    let path = dir.join(format!("meta-{tag}.zip"));
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("meta_plugin.wasm", opts).unwrap();
    zip.write_all(&fixture("meta_plugin.wasm")).unwrap();
    zip.finish().unwrap();
    path
}

fn dune() -> MetadataQuery {
    MetadataQuery { title: Some("Dune".into()), authors: None, isbn: None }
}

#[test]
fn a_plugin_with_no_declared_hosts_gets_no_network_and_returns_nothing() {
    // The default-deny case: the fixture calls the host getter, is
    // refused, sees the ERROR body, and returns an empty list rather
    // than inventing candidates.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = PluginPackage::read_zip(&package_meta(tmp.path(), "denied", "[]")).unwrap();
    assert!(pkg.manifest.capabilities.is_fully_sandboxed());

    let source = WasmMetadataSource::load(&pkg).unwrap();
    let candidates = source.search(&dune()).unwrap();

    assert!(candidates.is_empty(), "a plugin with no allowed_hosts must not have reached the network: {candidates:?}");
}

#[test]
fn the_guarded_getter_really_fetches_from_an_allowlisted_host() {
    // Direct coverage of the security-critical function against a real
    // HTTP server: allowlisted host -> real bytes come back.
    calibre_plugins_wasm::allow_loopback_for_tests();
    let api = TestApi::start("real bytes from a real server");

    let url = format!("http://{}/search", api.host());
    let body = calibre_plugins_wasm::metadata_source::guarded_http_get(&url, &[api.addr.ip().to_string()]).unwrap();

    assert_eq!(String::from_utf8(body).unwrap(), "real bytes from a real server");
}

#[test]
fn the_guarded_getter_refuses_the_very_same_host_when_it_is_not_allowlisted() {
    // Same server, same URL, only the allowlist differs -- so this
    // isolates the allowlist as the thing doing the gating.
    calibre_plugins_wasm::allow_loopback_for_tests();
    let api = TestApi::start("should never be read");

    let url = format!("http://{}/search", api.host());
    let err = calibre_plugins_wasm::metadata_source::guarded_http_get(&url, &["somewhere.else".to_string()]).unwrap_err();

    assert!(err.contains("allowed_hosts"), "{err}");
}

#[test]
fn the_host_overwrites_the_source_name_so_a_plugin_cannot_impersonate_another() {
    // The fixture deliberately reports a bogus `source`; the host must
    // replace it with the installed plugin's real name.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = PluginPackage::read_zip(&package_meta(tmp.path(), "offline", "[]")).unwrap();
    let source = WasmMetadataSource::load(&pkg).unwrap();

    for c in source.search(&dune()).unwrap() {
        assert_eq!(c.source, "Test Metadata Source", "the host must stamp its own name on every candidate");
    }
}

#[test]
fn a_package_declaring_a_different_plugin_type_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "Not A Source", "version": "1.0.0",
             "plugin_type": "file_type", "wasm": "meta_plugin.wasm", "file_types": ["txt"]}}"#
    );
    let path = tmp.path().join("wrong.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("meta_plugin.wasm", opts).unwrap();
    zip.write_all(&fixture("meta_plugin.wasm")).unwrap();
    zip.finish().unwrap();

    let pkg = PluginPackage::read_zip(&path).unwrap();
    let err = match WasmMetadataSource::load(&pkg) {
        Err(e) => e,
        Ok(_) => panic!("a file_type package must not load as a metadata source"),
    };
    assert!(matches!(err, MetadataAbiError::WrongPluginType { .. }), "got: {err}");
}

#[test]
fn a_plugin_returning_garbage_is_reported_not_silently_accepted() {
    // The probe fixture has no `search_metadata` export at all.
    let tmp = tempfile::tempdir().unwrap();
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "No Search Export", "version": "1.0.0",
             "plugin_type": "metadata_source", "wasm": "probe_plugin.wasm"}}"#
    );
    let path = tmp.path().join("noexport.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("probe_plugin.wasm", opts).unwrap();
    zip.write_all(&fixture("probe_plugin.wasm")).unwrap();
    zip.finish().unwrap();

    let source = WasmMetadataSource::load(&PluginPackage::read_zip(&path).unwrap()).unwrap();
    assert!(source.search(&dune()).is_err(), "a plugin with no search export must surface an error");
}
