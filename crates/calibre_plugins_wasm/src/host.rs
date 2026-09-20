//! Loading and running sandboxed WASM plugins (issue #798).
//!
//! # Why Extism rather than raw wasmtime
//!
//! Extism (which is itself built on wasmtime) supplies the plugin ABI,
//! the host-function plumbing, and a manifest with capability controls.
//! Raw wasmtime would mean hand-designing linear-memory and string
//! marshalling before any plugin could be written at all, for no gain
//! at this stage. Recorded as a decision in #754 rather than left
//! implicit.
//!
//! # The security posture, concretely
//!
//! Every plugin is loaded with:
//!
//! - **`with_wasi(false)`** -- no WASI at all unless the plugin
//!   declares capabilities. A plugin with the default (empty)
//!   [`Capabilities`] therefore has no filesystem, no environment, no
//!   clock, and no network. It can compute and return bytes; nothing
//!   else.
//! - **`allowed_hosts` / `allowed_paths` from the manifest only** --
//!   never inferred, never widened at runtime.
//! - **A real timeout and memory cap** from [`Limits`], so a plugin
//!   that loops forever or allocates without bound is killed rather
//!   than taking the host with it.
//!
//! Compare upstream calibre, which `exec`s third-party plugin Python in
//! the host process with the user's full privileges and no restrictions
//! whatsoever.
//!
//! # Failure isolation
//!
//! A plugin that traps, times out, or exceeds its limits produces a
//! [`WasmPluginError`] -- never a panic and never a process abort. This
//! mirrors (and improves on) the `catch_unwind` isolation
//! `calibre_customize::ui::run_filetype_plugins` already applies to
//! in-process plugins.

use std::io::Read;
use std::path::Path;

use extism::{Manifest as ExtismManifest, Plugin as ExtismPlugin, Wasm};

use crate::manifest::{Capabilities, Limits, Manifest, ManifestError, MANIFEST_FILENAME};

#[derive(Debug, thiserror::Error)]
pub enum WasmPluginError {
    #[error("could not read plugin package: {0}")]
    Package(String),
    #[error("plugin package has no {MANIFEST_FILENAME}")]
    MissingManifest,
    #[error("plugin package's {MANIFEST_FILENAME} is not valid JSON: {0}")]
    MalformedManifest(String),
    #[error(transparent)]
    InvalidManifest(#[from] ManifestError),
    #[error("plugin package does not contain the wasm module {0:?} its manifest names")]
    MissingWasmModule(String),
    #[error("could not instantiate plugin {name:?}: {reason}")]
    Instantiate { name: String, reason: String },
    /// A trap, a timeout, or an exceeded limit. Deliberately one
    /// variant: from the host's perspective these are the same event
    /// (the plugin did not return normally and must be skipped), and
    /// the message carries the detail.
    #[error("plugin {name:?} failed while running {function:?}: {reason}")]
    Call { name: String, function: String, reason: String },
}

/// A plugin package read off disk: its manifest plus the raw bytes of
/// its WASM module.
///
/// Kept separate from [`LoadedPlugin`] so a caller (and #801's UI) can
/// inspect what a package *declares* -- most importantly its requested
/// [`Capabilities`] -- before instantiating anything.
#[derive(Debug, Clone)]
pub struct PluginPackage {
    pub manifest: Manifest,
    pub wasm: Vec<u8>,
}

impl PluginPackage {
    /// Reads and validates a `.zip` plugin package.
    ///
    /// Validates the manifest before touching the WASM module, so a
    /// package declaring an unsupported ABI is rejected without any of
    /// its code being read.
    pub fn read_zip(path: &Path) -> Result<PluginPackage, WasmPluginError> {
        let file = std::fs::File::open(path).map_err(|e| WasmPluginError::Package(e.to_string()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| WasmPluginError::Package(e.to_string()))?;

        let manifest: Manifest = {
            let mut entry = zip.by_name(MANIFEST_FILENAME).map_err(|_| WasmPluginError::MissingManifest)?;
            let mut raw = String::new();
            entry.read_to_string(&mut raw).map_err(|e| WasmPluginError::Package(e.to_string()))?;
            serde_json::from_str(&raw).map_err(|e| WasmPluginError::MalformedManifest(e.to_string()))?
        };
        manifest.validate()?;

        let wasm = {
            let mut entry = zip.by_name(&manifest.wasm).map_err(|_| WasmPluginError::MissingWasmModule(manifest.wasm.clone()))?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| WasmPluginError::Package(e.to_string()))?;
            buf
        };

        Ok(PluginPackage { manifest, wasm })
    }

    /// Instantiates the plugin under the sandbox its manifest declares,
    /// with no host functions.
    pub fn load(&self) -> Result<LoadedPlugin, WasmPluginError> {
        self.load_with_host_functions(Vec::new())
    }

    /// Instantiates the plugin, additionally exposing `host_functions`
    /// to it.
    ///
    /// Host functions are how a sandboxed plugin reaches anything at
    /// all, so each one is a deliberate, audited hole in the sandbox
    /// and belongs to the typed ABI that needs it -- not to this
    /// module. `metadata_source` supplies an SSRF-guarded HTTP getter
    /// this way (#800); the file-type ABI (#799) supplies none,
    /// because it does not need any.
    pub fn load_with_host_functions(&self, host_functions: Vec<extism::Function>) -> Result<LoadedPlugin, WasmPluginError> {
        let extism_manifest = build_extism_manifest(&self.wasm, &self.manifest.capabilities, &self.manifest.limits);

        // `with_wasi(false)`: a plugin gets no WASI surface at all.
        // Filesystem access, when granted, comes from `allowed_paths`
        // on the manifest above -- never from an ambient WASI context.
        let plugin = ExtismPlugin::new(&extism_manifest, host_functions, false)
            .map_err(|e| WasmPluginError::Instantiate { name: self.manifest.name.clone(), reason: e.to_string() })?;

        Ok(LoadedPlugin { manifest: self.manifest.clone(), plugin })
    }
}

fn build_extism_manifest(wasm: &[u8], capabilities: &Capabilities, limits: &Limits) -> ExtismManifest {
    let mut m = ExtismManifest::new([Wasm::data(wasm.to_vec())])
        .with_timeout(std::time::Duration::from_millis(limits.timeout_ms))
        .with_memory_max(limits.max_pages);

    // Default deny: these loops add nothing when the manifest declared
    // nothing, which is exactly the intent.
    for host in &capabilities.allowed_hosts {
        m = m.with_allowed_host(host);
    }
    for (host_path, guest_path) in &capabilities.allowed_paths {
        m = m.with_allowed_path(host_path.clone(), guest_path);
    }
    m
}

/// An instantiated, sandboxed plugin.
pub struct LoadedPlugin {
    manifest: Manifest,
    plugin: ExtismPlugin,
}

impl LoadedPlugin {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Calls an exported function with raw bytes in and raw bytes out.
    ///
    /// Bytes-in/bytes-out is the whole ABI surface at this layer
    /// deliberately: every typed plugin ABI (#799's file-type
    /// transform, #800's metadata search) is defined *on top of* this
    /// in its own issue, so the host has exactly one thing to get right
    /// and each ABI can evolve without touching the sandbox.
    ///
    /// A trap, a timeout, or an exceeded memory cap comes back as
    /// [`WasmPluginError::Call`] -- never a panic.
    pub fn call(&mut self, function: &str, input: &[u8]) -> Result<Vec<u8>, WasmPluginError> {
        self.plugin
            .call::<&[u8], &[u8]>(function, input)
            .map(<[u8]>::to_vec)
            .map_err(|e| WasmPluginError::Call { name: self.manifest.name.clone(), function: function.to_string(), reason: e.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PluginType, ABI_VERSION};
    use std::io::Write;

    /// Builds a real plugin `.zip` on disk from a manifest and module
    /// bytes, so the tests exercise the real package-reading path
    /// rather than constructing `PluginPackage` directly.
    pub(crate) fn write_package(dir: &Path, manifest_json: &str, wasm: &[u8], wasm_name: &str) -> std::path::PathBuf {
        let path = dir.join("plugin.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file(MANIFEST_FILENAME, opts).unwrap();
        zip.write_all(manifest_json.as_bytes()).unwrap();
        if !wasm_name.is_empty() {
            zip.start_file(wasm_name, opts).unwrap();
            zip.write_all(wasm).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn manifest_json(extra: &str) -> String {
        format!(
            r#"{{"abi_version": {ABI_VERSION}, "name": "Probe", "version": "1.0.0",
                 "plugin_type": "file_type", "wasm": "plugin.wasm", "file_types": ["txt"]{extra}}}"#
        )
    }

    #[test]
    fn a_package_with_no_manifest_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.zip");
        let file = std::fs::File::create(&path).unwrap();
        zip::ZipWriter::new(file).finish().unwrap();

        assert!(matches!(PluginPackage::read_zip(&path), Err(WasmPluginError::MissingManifest)));
    }

    #[test]
    fn a_package_whose_manifest_is_not_json_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_package(tmp.path(), "not json at all", b"\0asm", "plugin.wasm");
        assert!(matches!(PluginPackage::read_zip(&path), Err(WasmPluginError::MalformedManifest(_))));
    }

    #[test]
    fn a_package_declaring_an_unsupported_abi_is_refused_without_reading_its_code() {
        let tmp = tempfile::tempdir().unwrap();
        let json = format!(
            r#"{{"abi_version": {}, "name": "N", "version": "1", "plugin_type": "file_type", "wasm": "plugin.wasm", "file_types": ["txt"]}}"#,
            ABI_VERSION + 99
        );
        // Deliberately no wasm entry: if the ABI check did not come
        // first, this would fail with MissingWasmModule instead.
        let path = write_package(tmp.path(), &json, b"", "");
        assert!(matches!(PluginPackage::read_zip(&path), Err(WasmPluginError::InvalidManifest(ManifestError::UnsupportedAbiVersion { .. }))));
    }

    #[test]
    fn a_package_missing_the_wasm_module_its_manifest_names_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_package(tmp.path(), &manifest_json(""), b"", "");
        assert!(matches!(PluginPackage::read_zip(&path), Err(WasmPluginError::MissingWasmModule(_))));
    }

    #[test]
    fn a_manifest_pointing_outside_the_package_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let json = format!(
            r#"{{"abi_version": {ABI_VERSION}, "name": "N", "version": "1", "plugin_type": "file_type", "wasm": "../../etc/passwd", "file_types": ["txt"]}}"#
        );
        let path = write_package(tmp.path(), &json, b"\0asm", "plugin.wasm");
        assert!(matches!(PluginPackage::read_zip(&path), Err(WasmPluginError::InvalidManifest(ManifestError::UnsafeWasmPath(_)))));
    }

    #[test]
    fn a_real_package_reads_back_its_manifest_and_module_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_package(tmp.path(), &manifest_json(""), b"\0asm\x01\x00\x00\x00", "plugin.wasm");

        let pkg = PluginPackage::read_zip(&path).unwrap();
        assert_eq!(pkg.manifest.name, "Probe");
        assert_eq!(pkg.manifest.plugin_type, PluginType::FileType);
        assert_eq!(pkg.wasm, b"\0asm\x01\x00\x00\x00");
        assert!(pkg.manifest.capabilities.is_fully_sandboxed());
    }

    #[test]
    fn declared_capabilities_survive_into_the_package_for_a_ui_to_show() {
        let tmp = tempfile::tempdir().unwrap();
        let json = manifest_json(r#", "capabilities": {"allowed_hosts": ["example.com"]}"#);
        let path = write_package(tmp.path(), &json, b"\0asm", "plugin.wasm");

        let pkg = PluginPackage::read_zip(&path).unwrap();
        assert_eq!(pkg.manifest.capabilities.allowed_hosts, ["example.com"]);
        assert!(!pkg.manifest.capabilities.is_fully_sandboxed());
    }

    #[test]
    fn garbage_wasm_fails_to_instantiate_rather_than_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_package(tmp.path(), &manifest_json(""), b"definitely not a wasm module", "plugin.wasm");
        let pkg = PluginPackage::read_zip(&path).unwrap();

        assert!(matches!(pkg.load(), Err(WasmPluginError::Instantiate { .. })));
    }
}
