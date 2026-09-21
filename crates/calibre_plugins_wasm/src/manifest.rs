//! The plugin package format and its manifest (issue #798).
//!
//! # Package format
//!
//! A calibre-oxide plugin is a **`.zip` containing a
//! `calibre-plugin.json` manifest and the `.wasm` module it names**.
//! A single self-describing file, so installing is "point at this
//! file" rather than "put these two things in the right places".
//!
//! Real upstream also ships plugins as zips, but identifies them by a
//! marker file (`plugin-import-name-<name>.txt`) whose *filename*
//! carries the import name, because Python needs the name to build an
//! import path. Nothing here needs that, so the metadata lives in a
//! real manifest instead of being encoded in a filename.
//!
//! # ABI versioning
//!
//! Upstream has effectively no ABI: plugins subclass live Python
//! classes and import calibre internals freely, so compatibility is
//! source-level and unversioned, gated only by a declared
//! `minimum_calibre_version` integer. A plugin either happens to work
//! against the internals it finds, or breaks at runtime.
//!
//! This port does better: [`ABI_VERSION`] is explicit and negotiated at
//! load time, and a plugin declaring an unsupported version is refused
//! with a clear error instead of being loaded and failing later in
//! some arbitrary way.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The plugin ABI version this host implements.
///
/// Bump only for a genuinely incompatible change to how a plugin is
/// called. A plugin declaring a different version is refused by
/// [`Manifest::validate`].
pub const ABI_VERSION: u32 = 1;

/// The manifest filename inside a plugin `.zip`.
pub const MANIFEST_FILENAME: &str = "calibre-plugin.json";

/// Which plugin ABI a package implements.
///
/// Deliberately a closed enum rather than a free string: an unknown
/// plugin type is a load-time error with a clear message, not a plugin
/// that installs successfully and then never does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    /// Transforms a book file (issue #799).
    FileType,
    /// Supplies online metadata candidates (issue #800).
    MetadataSource,
}

/// What a plugin is allowed to reach outside its sandbox.
///
/// **Default is deny.** An omitted or empty field grants nothing --
/// `Capabilities::default()` is a plugin with no filesystem and no
/// network access at all, which is the correct default for
/// third-party code.
///
/// This is the main place this port is deliberately stronger than
/// upstream calibre, which runs third-party plugin Python with the
/// full privileges of the user (no restricted builtins, no import
/// allowlist, and `sys.path` access that even permits shipping native
/// `.so` extensions).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Hosts the plugin may make HTTP requests to, e.g.
    /// `["www.googleapis.com"]`. Empty means no network.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Host path -> in-sandbox path mappings the plugin may read/write.
    /// Empty means no filesystem.
    #[serde(default)]
    pub allowed_paths: BTreeMap<String, String>,
}

impl Capabilities {
    /// True when this plugin asks for nothing beyond pure computation.
    /// A management UI (#801) can use this to show the common case
    /// without a scary-looking permissions list.
    pub fn is_fully_sandboxed(&self) -> bool {
        self.allowed_hosts.is_empty() && self.allowed_paths.is_empty()
    }
}

/// Execution limits, so a buggy or hostile plugin cannot hang or
/// exhaust the host.
///
/// Upstream has no equivalent: a plugin that loops forever hangs
/// calibre, and one that allocates without bound takes the process
/// down with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Wall-clock budget for a single plugin call.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum linear-memory pages (64KiB each) the plugin may use.
    #[serde(default = "default_max_pages")]
    pub max_pages: u32,
}

fn default_timeout_ms() -> u64 {
    5_000
}

fn default_max_pages() -> u32 {
    // 64 KiB * 1024 = 64 MiB, comfortably more than a text-transforming
    // plugin needs and far less than the host can be hurt by.
    1024
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { timeout_ms: default_timeout_ms(), max_pages: default_max_pages() }
    }
}

/// A plugin package's `calibre-plugin.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Must equal [`ABI_VERSION`]; see this module's doc.
    pub abi_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    pub plugin_type: PluginType,
    /// Path of the `.wasm` module inside the package zip.
    pub wasm: String,
    /// Extensions a `FileType` plugin handles. Ignored for other types.
    #[serde(default)]
    pub file_types: Vec<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub limits: Limits,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("plugin declares ABI version {found}, but this host implements version {expected}")]
    UnsupportedAbiVersion { found: u32, expected: u32 },
    #[error("plugin manifest is missing a {0}")]
    MissingField(&'static str),
    /// The `wasm` field is used to look up an entry inside the package
    /// zip, so it must not be able to climb out of it.
    #[error("plugin manifest's wasm path {0:?} is not a plain relative path inside the package")]
    UnsafeWasmPath(String),
    #[error("a FileType plugin must declare at least one file type")]
    FileTypePluginWithNoFileTypes,
}

impl Manifest {
    /// Checks everything that can be checked from the manifest alone,
    /// before any WASM is loaded or executed.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.abi_version != ABI_VERSION {
            return Err(ManifestError::UnsupportedAbiVersion { found: self.abi_version, expected: ABI_VERSION });
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::MissingField("name"));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::MissingField("version"));
        }
        if self.wasm.trim().is_empty() {
            return Err(ManifestError::MissingField("wasm"));
        }
        // The zip reader resolves this against entries in the archive;
        // an absolute path or one containing `..` must never be
        // accepted, matching the zip-slip hardening used elsewhere in
        // this workspace (save_to_disk.rs, library_import.rs).
        let wasm = self.wasm.replace('\\', "/");
        if wasm.starts_with('/') || wasm.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(ManifestError::UnsafeWasmPath(self.wasm.clone()));
        }
        if self.plugin_type == PluginType::FileType && self.file_types.is_empty() {
            return Err(ManifestError::FileTypePluginWithNoFileTypes);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Manifest {
        Manifest {
            abi_version: ABI_VERSION,
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "tests".to_string(),
            description: "does a thing".to_string(),
            plugin_type: PluginType::FileType,
            wasm: "plugin.wasm".to_string(),
            file_types: vec!["txt".to_string()],
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        }
    }

    #[test]
    fn a_well_formed_manifest_validates() {
        valid().validate().unwrap();
    }

    #[test]
    fn a_manifest_declaring_another_abi_version_is_refused() {
        let mut m = valid();
        m.abi_version = ABI_VERSION + 1;
        assert_eq!(m.validate().unwrap_err(), ManifestError::UnsupportedAbiVersion { found: ABI_VERSION + 1, expected: ABI_VERSION });
    }

    #[test]
    fn capabilities_default_to_granting_nothing() {
        let caps = Capabilities::default();
        assert!(caps.allowed_hosts.is_empty());
        assert!(caps.allowed_paths.is_empty());
        assert!(caps.is_fully_sandboxed());
    }

    #[test]
    fn a_manifest_omitting_capabilities_entirely_still_grants_nothing() {
        // The important case: a plugin author who writes no
        // `capabilities` block must not thereby get more access than
        // one who writes an empty one.
        let json = r#"{
            "abi_version": 1, "name": "N", "version": "1", "plugin_type": "file_type",
            "wasm": "p.wasm", "file_types": ["txt"]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        m.validate().unwrap();
        assert!(m.capabilities.is_fully_sandboxed());
    }

    #[test]
    fn a_manifest_omitting_limits_gets_real_bounded_defaults() {
        let json = r#"{
            "abi_version": 1, "name": "N", "version": "1", "plugin_type": "file_type",
            "wasm": "p.wasm", "file_types": ["txt"]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.limits.timeout_ms > 0, "an unbounded default timeout would let a plugin hang the host");
        assert!(m.limits.max_pages > 0, "an unbounded default memory cap would let a plugin exhaust the host");
    }

    #[test]
    fn an_absolute_or_traversing_wasm_path_is_refused() {
        for bad in ["/etc/passwd", "../../escape.wasm", "a/../../b.wasm"] {
            let mut m = valid();
            m.wasm = bad.to_string();
            assert!(matches!(m.validate(), Err(ManifestError::UnsafeWasmPath(_))), "{bad:?} should have been refused");
        }
    }

    #[test]
    fn a_backslash_traversal_is_also_refused() {
        let mut m = valid();
        m.wasm = r"..\..\escape.wasm".to_string();
        assert!(matches!(m.validate(), Err(ManifestError::UnsafeWasmPath(_))));
    }

    #[test]
    fn a_file_type_plugin_must_declare_its_file_types() {
        let mut m = valid();
        m.file_types.clear();
        assert_eq!(m.validate().unwrap_err(), ManifestError::FileTypePluginWithNoFileTypes);
    }

    #[test]
    fn a_metadata_source_plugin_needs_no_file_types() {
        let mut m = valid();
        m.plugin_type = PluginType::MetadataSource;
        m.file_types.clear();
        m.validate().unwrap();
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let m = valid();
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn declared_capabilities_are_preserved_so_a_ui_can_show_them() {
        let json = r#"{
            "abi_version": 1, "name": "N", "version": "1", "plugin_type": "metadata_source",
            "wasm": "p.wasm",
            "capabilities": {"allowed_hosts": ["www.googleapis.com"]}
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        m.validate().unwrap();
        assert_eq!(m.capabilities.allowed_hosts, ["www.googleapis.com"]);
        assert!(!m.capabilities.is_fully_sandboxed(), "a plugin asking for network access must not look fully sandboxed");
    }
}

/// Compares two plugin version strings, newest-wins.
///
/// Versions in a manifest are free-form strings, so a plain string
/// comparison is wrong in the most common case there is: `"1.10.0"`
/// sorts *before* `"1.9.0"` lexically, which would tell a user their
/// newer plugin is out of date and offer to downgrade it.
///
/// Numeric components are compared numerically, and a missing
/// component counts as zero so `"1.2"` and `"1.2.0"` are equal. A
/// non-numeric component (`"1.0-beta"`) falls back to comparing that
/// component as text, which is not a full semver pre-release ordering
/// but is predictable and never claims two different versions are the
/// same.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let split = |v: &str| -> Vec<String> { v.trim().split('.').map(str::to_string).collect() };
    let (a_parts, b_parts) = (split(a), split(b));

    for i in 0..a_parts.len().max(b_parts.len()) {
        let ap = a_parts.get(i).map(String::as_str).unwrap_or("0");
        let bp = b_parts.get(i).map(String::as_str).unwrap_or("0");
        if ap == bp {
            continue;
        }
        let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            // A numeric component outranks a non-numeric one at the
            // same position: "1.0" is a release, "1.0-rc" is not.
            (Ok(_), Err(_)) => Ordering::Greater,
            (Err(_), Ok(_)) => Ordering::Less,
            (Err(_), Err(_)) => ap.cmp(bp),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod version_tests {
    use super::compare_versions;
    use std::cmp::Ordering;

    /// The case that makes this function necessary at all.
    #[test]
    fn ten_is_newer_than_nine() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert!("1.10.0" < "1.9.0", "a plain string compare really does get this backwards");
    }

    #[test]
    fn compares_component_by_component() {
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.2.3", "1.2.4"), Ordering::Less);
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
    }

    #[test]
    fn a_missing_component_counts_as_zero() {
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.1", "1.2"), Ordering::Greater);
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        assert_eq!(compare_versions(" 1.0.0 ", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn a_release_outranks_a_prerelease_at_the_same_position() {
        assert_eq!(compare_versions("1.0.0", "1.0.0-rc1"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-rc1", "1.0.0"), Ordering::Less);
    }

    /// Never claim two different version strings are the same -- that
    /// would hide an available update.
    #[test]
    fn different_non_numeric_versions_are_not_equal() {
        assert_ne!(compare_versions("1.0.0-alpha", "1.0.0-beta"), Ordering::Equal);
    }
}
