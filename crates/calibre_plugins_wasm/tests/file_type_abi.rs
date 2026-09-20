//! End-to-end proof of the file-type plugin ABI (issue #799).
//!
//! Uses `tests/fixtures/banner_plugin.wasm` -- a realistic third-party
//! plugin that stamps a banner onto an imported `.txt` file, built from
//! `tests/fixtures/banner_plugin/`.
//!
//! The important assertion is not "the ABI function returns bytes" but
//! that a WASM plugin runs through the **same** dispatch as an
//! in-process one: `calibre_customize::ui::run_plugins_on_import_from_registry`,
//! with the registry's real extension filtering, priority ordering and
//! enable/disable.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use calibre_customize::registry::PluginRegistry;
use calibre_customize::ui::run_plugins_on_import_from_registry;
use calibre_customize::{FileTypePlugin, Plugin};
use calibre_plugins_wasm::file_type::{FileTypeAbiError, WasmFileTypePlugin};
use calibre_plugins_wasm::host::PluginPackage;
use calibre_plugins_wasm::manifest::ABI_VERSION;

const BANNER: &str = "=== PROCESSED BY BANNER PLUGIN ===\n";

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e} -- run tests/fixtures/build.sh", path.display()))
}

/// Packages the banner plugin, optionally overriding manifest fields.
fn package_banner(dir: &Path, plugin_type: &str, file_types: &str) -> PathBuf {
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "Banner Plugin", "version": "2.1.0",
             "author": "A Third Party", "description": "Stamps a banner onto imported text files",
             "plugin_type": "{plugin_type}", "wasm": "banner_plugin.wasm", "file_types": {file_types}}}"#
    );
    let path = dir.join(format!("banner-{plugin_type}.zip"));
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("banner_plugin.wasm", opts).unwrap();
    zip.write_all(&fixture("banner_plugin.wasm")).unwrap();
    zip.finish().unwrap();
    path
}

fn load_banner(dir: &Path) -> WasmFileTypePlugin {
    let pkg = PluginPackage::read_zip(&package_banner(dir, "file_type", r#"["txt"]"#)).unwrap();
    WasmFileTypePlugin::load(&pkg).unwrap()
}

#[test]
fn a_real_wasm_plugin_transforms_real_content_through_the_abi() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin = load_banner(tmp.path());

    let out = plugin.transform(b"chapter one").unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{BANNER}chapter one"));
}

#[test]
fn a_real_wasm_plugin_exposes_its_manifest_metadata_as_a_real_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin = load_banner(tmp.path());

    assert_eq!(plugin.name(), "Banner Plugin");
    assert_eq!(plugin.version(), (2, 1, 0));
    assert_eq!(plugin.author(), "A Third Party");
    assert_eq!(plugin.file_types(), ["txt"]);
    assert!(plugin.on_import());
    // An installed third-party plugin must not claim to be a builtin.
    assert_eq!(plugin.installation_type(), Some(calibre_customize::PluginInstallationType::External));
}

#[test]
fn a_wasm_plugin_runs_through_the_same_registry_dispatch_as_an_in_process_one() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "chapter one").unwrap();

    let mut registry = PluginRegistry::new();
    registry.register::<dyn FileTypePlugin>(Arc::new(load_banner(tmp.path()))).unwrap();

    let out = run_plugins_on_import_from_registry(&src, &registry);

    assert_ne!(out, src, "the plugin should have produced a new path");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), format!("{BANNER}chapter one"));
}

#[test]
fn a_wasm_plugin_is_skipped_for_an_extension_it_did_not_declare() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("book.epub");
    std::fs::write(&src, "not text").unwrap();

    let mut registry = PluginRegistry::new();
    registry.register::<dyn FileTypePlugin>(Arc::new(load_banner(tmp.path()))).unwrap();

    let out = run_plugins_on_import_from_registry(&src, &registry);

    assert_eq!(out, src, "a .txt plugin must not run on a .epub");
    assert_eq!(std::fs::read_to_string(&src).unwrap(), "not text");
}

#[test]
fn a_disabled_wasm_plugin_does_not_run() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "chapter one").unwrap();

    let mut registry = PluginRegistry::new();
    registry.register::<dyn FileTypePlugin>(Arc::new(load_banner(tmp.path()))).unwrap();
    registry.set_enabled("Banner Plugin", false).unwrap();

    let out = run_plugins_on_import_from_registry(&src, &registry);
    assert_eq!(out, src);
    assert_eq!(std::fs::read_to_string(&src).unwrap(), "chapter one", "a disabled plugin must not have rewritten anything");
}

#[test]
fn a_wasm_plugin_and_a_builtin_plugin_chain_together() {
    // The real point of reusing the existing dispatch: a third-party
    // WASM plugin and an in-process Rust plugin compose, in priority
    // order, with each one's output threaded into the next.
    struct Suffix;
    impl Plugin for Suffix {
        fn name(&self) -> &str {
            "Suffix (in-process)"
        }
        fn priority(&self) -> u64 {
            1 // lower than the WASM plugin's default, so it runs second
        }
    }
    impl FileTypePlugin for Suffix {
        fn file_types(&self) -> Vec<String> {
            vec!["txt".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, path: &Path) -> PathBuf {
            let content = std::fs::read_to_string(path).unwrap();
            let out = path.with_extension("suffixed.txt");
            std::fs::write(&out, format!("{content}\n-- the end --")).unwrap();
            out
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "chapter one").unwrap();

    let mut registry = PluginRegistry::new();
    // Registered second but higher priority, so it must still run first.
    registry.register::<dyn FileTypePlugin>(Arc::new(Suffix)).unwrap();
    registry.register::<dyn FileTypePlugin>(Arc::new(load_banner(tmp.path()))).unwrap();

    let out = run_plugins_on_import_from_registry(&src, &registry);
    let content = std::fs::read_to_string(&out).unwrap();

    assert_eq!(content, format!("{BANNER}chapter one\n-- the end --"), "both plugins should have applied, WASM first");
}

#[test]
fn a_package_declaring_a_different_plugin_type_is_refused() {
    // Refused at load, not at call time -- a metadata-source package
    // must not install successfully as a file-type plugin and then
    // fail on a missing export later.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = PluginPackage::read_zip(&package_banner(tmp.path(), "metadata_source", "[]")).unwrap();

    let err = match WasmFileTypePlugin::load(&pkg) {
        Err(e) => e,
        Ok(_) => panic!("a metadata_source package must not load as a file-type plugin"),
    };
    assert!(matches!(err, FileTypeAbiError::WrongPluginType { .. }), "got: {err}");
}

#[test]
fn a_plugin_failure_leaves_the_original_file_untouched() {
    // The probe plugin has no `run_file_type` export at all, so calling
    // the ABI against it fails. The dispatch must degrade to the
    // original path rather than losing the file.
    let tmp = tempfile::tempdir().unwrap();
    let json = format!(
        r#"{{"abi_version": {ABI_VERSION}, "name": "No ABI Plugin", "version": "1.0.0",
             "plugin_type": "file_type", "wasm": "probe_plugin.wasm", "file_types": ["txt"]}}"#
    );
    let pkg_path = tmp.path().join("noabi.zip");
    let file = std::fs::File::create(&pkg_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::FileOptions::default();
    zip.start_file("calibre-plugin.json", opts).unwrap();
    zip.write_all(json.as_bytes()).unwrap();
    zip.start_file("probe_plugin.wasm", opts).unwrap();
    zip.write_all(&fixture("probe_plugin.wasm")).unwrap();
    zip.finish().unwrap();

    let plugin = WasmFileTypePlugin::load(&PluginPackage::read_zip(&pkg_path).unwrap()).unwrap();
    assert!(plugin.transform(b"anything").is_err(), "a plugin with no run_file_type export should fail the ABI call");

    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "original content").unwrap();
    let mut registry = PluginRegistry::new();
    registry.register::<dyn FileTypePlugin>(Arc::new(plugin)).unwrap();

    let out = run_plugins_on_import_from_registry(&src, &registry);
    assert_eq!(out, src, "a failing plugin must leave the path unchanged");
    assert_eq!(std::fs::read_to_string(&src).unwrap(), "original content");
}
