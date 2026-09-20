//! The file-type transform ABI (issue #799), the first typed plugin
//! ABI layered on #798's bytes-in/bytes-out host.
//!
//! # Why this type went first
//!
//! Checked against real upstream rather than assumed:
//!
//! - Its shape is effectively bytes-in/bytes-out
//!   (`run(path_to_ebook) -> path_to_ebook`), which is exactly what a
//!   WASM boundary handles most naturally -- no host object graph has
//!   to cross it.
//! - It carries the most real-world third-party weight. Upstream's own
//!   `manual/creating_plugins.rst` teaches `FileTypePlugin` first and
//!   ships it as the "Hello World" sample
//!   (`manual/plugin_examples/helloworld/`), and it is the hook the
//!   most widely used third-party calibre plugin (DeDRM) is built on
//!   -- enough that `customize/ui.py` carries a hardcoded version
//!   blacklist for it.
//!
//! # The ABI
//!
//! A plugin exports **`run_file_type`**, taking the file's *content*
//! and returning the transformed content. Deliberately content, not a
//! path: a sandboxed plugin has no filesystem, and handing it one
//! would mean granting filesystem capability to every file-type plugin
//! just to let it read the file it was already given. The host reads
//! and writes; the plugin only transforms bytes.
//!
//! That is a real, disclosed divergence from upstream, whose `run`
//! takes a path and returns a (possibly different) path, because
//! upstream plugins have unrestricted filesystem access anyway.
//!
//! # Reuses the existing dispatch, does not duplicate it
//!
//! [`WasmFileTypePlugin`] implements `calibre_customize::FileTypePlugin`,
//! so a loaded WASM plugin goes through the **same**
//! `ui.rs::run_filetype_plugins` path as an in-process one: same
//! extension/occasion filtering, same priority ordering, same
//! `catch_unwind` isolation. Nothing about the dispatch is re-written
//! for WASM.
//!
//! # No compatibility with existing calibre plugins
//!
//! Upstream's `FileTypePlugin`s are Python that import calibre
//! internals. **DeDRM and friends will not run here.** This is a new
//! ecosystem with the same hook shape, not a compatibility layer.
//! Stated plainly so it is not assumed later.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use calibre_customize::{FileTypePlugin, Plugin, PluginInstallationType};

use crate::host::{LoadedPlugin, PluginPackage, WasmPluginError};
use crate::manifest::PluginType;

/// The export a file-type plugin must provide.
pub const RUN_EXPORT: &str = "run_file_type";

/// A loaded WASM plugin presented as a real
/// [`calibre_customize::FileTypePlugin`].
///
/// The `Mutex` is not for cross-thread sharing so much as for
/// interior mutability: `FileTypePlugin::run` takes `&self`, while
/// calling into a WASM instance needs `&mut`. One instance is not
/// re-entrant, so serializing calls to it is also correct.
pub struct WasmFileTypePlugin {
    name: String,
    version: (u32, u32, u32),
    author: String,
    description: String,
    file_types: Vec<String>,
    plugin: Mutex<LoadedPlugin>,
}

#[derive(Debug, thiserror::Error)]
pub enum FileTypeAbiError {
    #[error("plugin {name:?} is a {actual:?} plugin, not a file-type plugin")]
    WrongPluginType { name: String, actual: PluginType },
    #[error(transparent)]
    Wasm(#[from] WasmPluginError),
}

impl WasmFileTypePlugin {
    /// Loads a package as a file-type plugin.
    ///
    /// Refuses a package declaring a different [`PluginType`], rather
    /// than loading it and failing later when its missing export is
    /// called.
    pub fn load(package: &PluginPackage) -> Result<WasmFileTypePlugin, FileTypeAbiError> {
        if package.manifest.plugin_type != PluginType::FileType {
            return Err(FileTypeAbiError::WrongPluginType { name: package.manifest.name.clone(), actual: package.manifest.plugin_type });
        }
        let loaded = package.load()?;
        let m = &package.manifest;

        Ok(WasmFileTypePlugin {
            name: m.name.clone(),
            version: parse_version(&m.version),
            author: m.author.clone(),
            description: m.description.clone(),
            file_types: m.file_types.iter().map(|t| t.to_lowercase()).collect(),
            plugin: Mutex::new(loaded),
        })
    }

    /// Runs the transform over raw content, exposed separately from
    /// the trait's path-based [`FileTypePlugin::run`] so a caller (and
    /// the tests) can exercise the ABI without touching the disk.
    pub fn transform(&self, content: &[u8]) -> Result<Vec<u8>, WasmPluginError> {
        let mut guard = self.plugin.lock().expect("plugin mutex poisoned by a previous panic");
        guard.call(RUN_EXPORT, content)
    }
}

/// Best-effort `major.minor.patch`; a version string that isn't in
/// that shape degrades to `(0, 0, 0)` rather than failing the load.
/// The version is metadata for display, not something the host makes
/// decisions on -- [`crate::manifest::ABI_VERSION`] is what gates
/// compatibility.
fn parse_version(raw: &str) -> (u32, u32, u32) {
    let mut parts = raw.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

impl Plugin for WasmFileTypePlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> (u32, u32, u32) {
        self.version
    }
    fn author(&self) -> &str {
        &self.author
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn installation_type(&self) -> Option<PluginInstallationType> {
        Some(PluginInstallationType::External)
    }
    fn type_name(&self) -> &str {
        "File type"
    }
}

impl FileTypePlugin for WasmFileTypePlugin {
    fn file_types(&self) -> Vec<String> {
        self.file_types.clone()
    }

    fn on_import(&self) -> bool {
        true
    }

    /// Reads the file, hands its *content* to the sandboxed plugin, and
    /// writes the result back beside it.
    ///
    /// On any failure the original path is returned unchanged --
    /// matching both this crate's own host-level isolation and
    /// upstream's tolerance of a plugin that raises (`ui.py` logs and
    /// keeps whatever path the previous plugin produced).
    fn run(&self, path_to_ebook: &Path) -> PathBuf {
        match self.run_inner(path_to_ebook) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("WASM file-type plugin {:?} failed on {}: {e}", self.name, path_to_ebook.display());
                path_to_ebook.to_path_buf()
            }
        }
    }
}

impl WasmFileTypePlugin {
    fn run_inner(&self, path_to_ebook: &Path) -> anyhow::Result<PathBuf> {
        let content = std::fs::read(path_to_ebook)?;
        let transformed = self.transform(&content)?;

        // Write beside the input rather than over it: the dispatch
        // threads each plugin's output path into the next, and
        // overwriting in place would make a failure midway through a
        // chain unrecoverable.
        let stem = path_to_ebook.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = path_to_ebook.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let out = path_to_ebook.with_file_name(format!("{stem}.plugin-out.{ext}"));
        std::fs::write(&out, transformed)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_string_parses_into_its_parts() {
        assert_eq!(parse_version("2.1.7"), (2, 1, 7));
        assert_eq!(parse_version("1.0"), (1, 0, 0));
    }

    #[test]
    fn a_nonsense_version_degrades_instead_of_failing_the_load() {
        // Version is display metadata; ABI_VERSION is what actually
        // gates compatibility, so a weird version must not be fatal.
        assert_eq!(parse_version("not-a-version"), (0, 0, 0));
        assert_eq!(parse_version(""), (0, 0, 0));
    }
}
