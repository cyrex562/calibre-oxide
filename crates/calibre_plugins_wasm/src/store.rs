//! On-disk installed-plugin storage (issue #798).
//!
//! Mirrors upstream's model -- installing copies the package into a
//! per-user plugins directory and the app loads from there on startup
//! (`add_plugin(path_to_zip)` in `customize/ui.py`) -- without
//! upstream's global mutable plugin list.
//!
//! A [`PluginStore`] is an owned handle on one directory, for the same
//! reason [`calibre_customize::registry::PluginRegistry`] is owned
//! rather than global: it makes the whole class of shared-mutable-state
//! test races impossible, and lets a caller scope plugins per server.

use std::path::{Path, PathBuf};

use crate::host::{PluginPackage, WasmPluginError};

/// Packages are stored as `<name>.zip`, so the installed set is
/// inspectable with ordinary tools and a name collision is a real file
/// collision rather than silent shadowing.
const PACKAGE_EXTENSION: &str = "zip";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("plugin store directory {path:?} is unusable: {reason}")]
    Directory { path: PathBuf, reason: String },
    #[error(transparent)]
    Plugin(#[from] WasmPluginError),
    #[error("a plugin named {0:?} is already installed")]
    AlreadyInstalled(String),
    #[error("no plugin named {0:?} is installed")]
    NotInstalled(String),
    /// A plugin name becomes a filename, so it must not be able to
    /// point anywhere but inside the store directory.
    #[error("plugin name {0:?} cannot be used as a file name")]
    UnsafeName(String),
}

/// A directory of installed plugin packages.
pub struct PluginStore {
    dir: PathBuf,
}

impl PluginStore {
    /// Opens (creating if needed) a plugin store rooted at `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<PluginStore, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Directory { path: dir.clone(), reason: e.to_string() })?;
        Ok(PluginStore { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A plugin's name comes from its own manifest, i.e. from
    /// untrusted package content, and is used to build a path. Reject
    /// anything that is not a plain, single-component file stem --
    /// matching the hardening applied to untrusted path components
    /// elsewhere in this workspace (`save_to_disk.rs`,
    /// `library_import.rs`).
    fn package_path(&self, name: &str) -> Result<PathBuf, StoreError> {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || trimmed == "."
            || trimmed == ".."
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.contains('\0')
        {
            return Err(StoreError::UnsafeName(name.to_string()));
        }
        Ok(self.dir.join(format!("{trimmed}.{PACKAGE_EXTENSION}")))
    }

    /// Installs the package at `src`, returning the package it read.
    ///
    /// Validates before copying, so a malformed or
    /// unsupported-ABI package never lands in the store at all.
    pub fn install(&self, src: &Path) -> Result<PluginPackage, StoreError> {
        let pkg = PluginPackage::read_zip(src)?;
        let dest = self.package_path(&pkg.manifest.name)?;
        if dest.exists() {
            return Err(StoreError::AlreadyInstalled(pkg.manifest.name.clone()));
        }
        std::fs::copy(src, &dest).map_err(|e| StoreError::Directory { path: dest, reason: e.to_string() })?;
        Ok(pkg)
    }

    /// Every installed package, by name.
    ///
    /// A package that no longer reads back (corrupted on disk, or from
    /// a newer ABI after a downgrade) is **skipped with a warning**
    /// rather than failing the whole listing -- one bad plugin must not
    /// make the rest un-listable, which is also what makes it
    /// removable through [`PluginStore::remove`].
    pub fn list(&self) -> Result<Vec<PluginPackage>, StoreError> {
        let entries = std::fs::read_dir(&self.dir).map_err(|e| StoreError::Directory { path: self.dir.clone(), reason: e.to_string() })?;

        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(PACKAGE_EXTENSION) {
                continue;
            }
            match PluginPackage::read_zip(&path) {
                Ok(pkg) => out.push(pkg),
                Err(e) => eprintln!("skipping unreadable plugin package {}: {e}", path.display()),
            }
        }
        out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        Ok(out)
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.package_path(name).map(|p| p.exists()).unwrap_or(false)
    }

    /// Removes an installed plugin.
    pub fn remove(&self, name: &str) -> Result<(), StoreError> {
        let path = self.package_path(name)?;
        if !path.exists() {
            return Err(StoreError::NotInstalled(name.to_string()));
        }
        std::fs::remove_file(&path).map_err(|e| StoreError::Directory { path, reason: e.to_string() })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ABI_VERSION;
    use std::io::Write;

    fn write_package(dir: &Path, file_name: &str, plugin_name: &str) -> PathBuf {
        let json = format!(
            r#"{{"abi_version": {ABI_VERSION}, "name": "{plugin_name}", "version": "1.0.0",
                 "plugin_type": "file_type", "wasm": "p.wasm", "file_types": ["txt"]}}"#
        );
        let path = dir.join(file_name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file("calibre-plugin.json", opts).unwrap();
        zip.write_all(json.as_bytes()).unwrap();
        zip.start_file("p.wasm", opts).unwrap();
        zip.write_all(b"\0asm\x01\x00\x00\x00").unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn installing_then_listing_returns_the_real_package() {
        let tmp = tempfile::tempdir().unwrap();
        let src = write_package(tmp.path(), "src.zip", "Alpha");
        let store = PluginStore::open(tmp.path().join("store")).unwrap();

        let installed = store.install(&src).unwrap();
        assert_eq!(installed.manifest.name, "Alpha");
        assert!(store.is_installed("Alpha"));

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.name, "Alpha");
    }

    #[test]
    fn listing_is_sorted_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        store.install(&write_package(tmp.path(), "b.zip", "Zulu")).unwrap();
        store.install(&write_package(tmp.path(), "a.zip", "Alpha")).unwrap();

        let names: Vec<String> = store.list().unwrap().into_iter().map(|p| p.manifest.name).collect();
        assert_eq!(names, ["Alpha", "Zulu"]);
    }

    #[test]
    fn installing_the_same_plugin_name_twice_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        store.install(&write_package(tmp.path(), "a.zip", "Alpha")).unwrap();

        let again = write_package(tmp.path(), "b.zip", "Alpha");
        assert!(matches!(store.install(&again), Err(StoreError::AlreadyInstalled(_))));
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn a_malformed_package_is_refused_and_never_lands_in_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad.zip");
        std::fs::write(&bad, b"not a zip at all").unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();

        assert!(store.install(&bad).is_err());
        assert!(store.list().unwrap().is_empty(), "a rejected package must not be copied into the store");
    }

    #[test]
    fn removing_an_installed_plugin_really_deletes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        store.install(&write_package(tmp.path(), "a.zip", "Alpha")).unwrap();

        store.remove("Alpha").unwrap();
        assert!(!store.is_installed("Alpha"));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn removing_something_not_installed_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        assert!(matches!(store.remove("Ghost"), Err(StoreError::NotInstalled(_))));
    }

    #[test]
    fn a_plugin_name_cannot_escape_the_store_directory() {
        // The name comes from untrusted package content and becomes a
        // filename, so traversal must be refused outright.
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();

        for evil in ["../escape", "a/b", r"a\b", "..", "", "   "] {
            assert!(matches!(store.package_path(evil), Err(StoreError::UnsafeName(_))), "{evil:?} should have been refused");
        }
    }

    #[test]
    fn installing_a_package_whose_manifest_name_traverses_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let src = write_package(tmp.path(), "evil.zip", "../../escaped");
        let store = PluginStore::open(tmp.path().join("store")).unwrap();

        assert!(matches!(store.install(&src), Err(StoreError::UnsafeName(_))));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn one_corrupt_package_does_not_make_the_rest_unlistable() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        store.install(&write_package(tmp.path(), "good.zip", "Good")).unwrap();
        // Drop a corrupt file straight into the store directory.
        std::fs::write(store.dir().join("corrupt.zip"), b"garbage").unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.name, "Good");
    }

    #[test]
    fn non_package_files_in_the_directory_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PluginStore::open(tmp.path().join("store")).unwrap();
        std::fs::write(store.dir().join("README.txt"), b"not a plugin").unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
