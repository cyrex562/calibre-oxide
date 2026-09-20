//! The real plugin registry (issue #795, foundation of the #754 epic).
//!
//! # What this replaces
//!
//! Before this module, [`crate::ui::run_plugins_on_import`]'s own doc
//! said it outright: *"There is no global plugin registry yet"*. Every
//! caller had to construct and pass its own `&[Box<dyn FileTypePlugin>]`
//! slice, so there was no way for a plugin to be *registered* anywhere,
//! and [`crate::builtins`] returned three hardcoded strings naming
//! plugins that did not exist.
//!
//! # Shape: one flat list, filtered by type
//!
//! Real upstream (`calibre/customize/ui.py`) keeps a single flat
//! `_initialized_plugins` list, sorts it by `priority` descending
//! (`sort(key=lambda x: x.priority, reverse=True)`), and finds plugins
//! of a given kind with `isinstance()` scans over it. This module
//! reproduces that behavior rather than keeping a separate list per
//! plugin type, because the flat list is what gives a single, stable,
//! cross-type priority order and a single place for per-plugin
//! enabled/disabled state.
//!
//! Rust has no `isinstance`, so [`RegisteredPlugin`] is an enum with one
//! variant per supported plugin trait. Adding a new plugin type is a new
//! variant plus a new accessor -- deliberately explicit, so that a
//! plugin type can never be registered that no accessor can find again.
//!
//! # Owned, not global
//!
//! [`PluginRegistry`] is a plain owned struct. The one existing
//! register-and-discover pattern in this workspace
//! (`calibre_ai::prefs`'s `REGISTERED_PLUGINS`) is a process-global
//! `lazy_static`, and its own tests have to serialize themselves behind
//! a `Mutex` to avoid racing on it. An owned registry avoids that whole
//! class of problem, lets a caller (e.g. `calibre_srv`'s `AppState`)
//! hold one per server, and keeps tests independent. A process-global
//! default can be layered on later if a caller genuinely needs one --
//! nothing here prevents it, and no caller needs it yet.
//!
//! # Not here
//!
//! Loading a plugin from disk, from a `.zip`, or from a `.wasm` module
//! is issue #798. This module only holds plugins that were registered
//! in-process.

use std::sync::Arc;

use crate::conversion::{InputFormatPlugin, OutputFormatPlugin};
use crate::{FileTypePlugin, Plugin, PluginInstallationType};

/// A plugin of one of the concrete kinds the registry can store and
/// hand back out again.
///
/// The variants mirror the plugin traits in this crate. `Arc` (not
/// `Box`) so the registry can hand out cheap clones without giving up
/// ownership -- callers like [`crate::ui::run_plugins_on_import_from_registry`]
/// need an owned collection to iterate.
#[derive(Clone)]
pub enum RegisteredPlugin {
    FileType(Arc<dyn FileTypePlugin>),
    InputFormat(Arc<dyn InputFormatPlugin>),
    OutputFormat(Arc<dyn OutputFormatPlugin>),
}

impl RegisteredPlugin {
    /// The common [`Plugin`] view, for the metadata every plugin has
    /// regardless of kind. Relies on trait upcasting (stable since Rust
    /// 1.86) -- each of these traits has [`Plugin`] as a supertrait.
    pub fn as_plugin(&self) -> &dyn Plugin {
        match self {
            RegisteredPlugin::FileType(p) => p.as_ref(),
            RegisteredPlugin::InputFormat(p) => p.as_ref(),
            RegisteredPlugin::OutputFormat(p) => p.as_ref(),
        }
    }

    /// A stable identifier for the plugin's kind, for listings and error
    /// messages. Distinct from [`Plugin::type_name`], which a plugin can
    /// override to anything it likes.
    pub fn kind(&self) -> &'static str {
        match self {
            RegisteredPlugin::FileType(_) => "FileType",
            RegisteredPlugin::InputFormat(_) => "InputFormat",
            RegisteredPlugin::OutputFormat(_) => "OutputFormat",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    /// Port of `add_plugin`'s own name-collision rejection upstream,
    /// which refuses a plugin whose name matches an already-registered
    /// builtin or system plugin.
    #[error("a plugin named {0:?} is already registered")]
    DuplicateName(String),
    #[error("no plugin named {0:?} is registered")]
    NotFound(String),
    /// Port of upstream's `can_be_disabled = False`, which it sets on
    /// plugin types that the user must not be able to turn off.
    #[error("the plugin {0:?} declares that it cannot be disabled")]
    CannotBeDisabled(String),
}

struct Entry {
    plugin: RegisteredPlugin,
    enabled: bool,
}

/// A flat, priority-ordered set of registered plugins.
///
/// See the module doc for why this is owned rather than global, and why
/// it is one list rather than one list per plugin type.
#[derive(Default)]
pub struct PluginRegistry {
    entries: Vec<Entry>,
}

/// A registered plugin's metadata, flattened for listing (the plugin
/// management UI, issue #801, is the intended consumer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub version: (u32, u32, u32),
    pub author: String,
    pub description: String,
    pub kind: &'static str,
    pub enabled: bool,
    pub can_be_disabled: bool,
    pub installation_type: Option<PluginInstallationType>,
}

impl PluginRegistry {
    pub fn new() -> PluginRegistry {
        PluginRegistry::default()
    }

    /// Registers `plugin`, newly enabled.
    ///
    /// Rejects a name already present, matching upstream's `add_plugin`.
    /// Names are compared case-sensitively and verbatim, the way
    /// upstream compares them.
    pub fn register(&mut self, plugin: RegisteredPlugin) -> Result<(), RegistryError> {
        let name = plugin.as_plugin().name().to_string();
        if self.entries.iter().any(|e| e.plugin.as_plugin().name() == name) {
            return Err(RegistryError::DuplicateName(name));
        }
        self.entries.push(Entry { plugin, enabled: true });
        Ok(())
    }

    /// Enables or disables the plugin named `name`.
    ///
    /// Disabling a plugin whose [`Plugin::can_be_disabled`] is false is
    /// an error rather than a silent no-op, so a UI can report it
    /// instead of showing a toggle that appears to work and doesn't.
    /// *Enabling* is always allowed -- `can_be_disabled` constrains only
    /// the disable direction, matching what the flag actually means.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), RegistryError> {
        let entry = self.entries.iter_mut().find(|e| e.plugin.as_plugin().name() == name).ok_or_else(|| RegistryError::NotFound(name.to_string()))?;
        if !enabled && !entry.plugin.as_plugin().can_be_disabled() {
            return Err(RegistryError::CannotBeDisabled(name.to_string()));
        }
        entry.enabled = enabled;
        Ok(())
    }

    pub fn is_enabled(&self, name: &str) -> Option<bool> {
        self.entries.iter().find(|e| e.plugin.as_plugin().name() == name).map(|e| e.enabled)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every registered plugin's metadata, enabled or not, in priority
    /// order. Disabled plugins are included deliberately: a management
    /// UI has to list them in order to offer re-enabling them.
    pub fn list(&self) -> Vec<PluginInfo> {
        let mut entries: Vec<&Entry> = self.entries.iter().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.plugin.as_plugin().priority()));
        entries
            .into_iter()
            .map(|e| {
                let p = e.plugin.as_plugin();
                PluginInfo {
                    name: p.name().to_string(),
                    version: p.version(),
                    author: p.author().to_string(),
                    description: p.description().to_string(),
                    kind: e.plugin.kind(),
                    enabled: e.enabled,
                    can_be_disabled: p.can_be_disabled(),
                    installation_type: p.installation_type(),
                }
            })
            .collect()
    }

    /// Enabled plugins of one kind, highest [`Plugin::priority`] first.
    ///
    /// Upstream sorts its single plugin list by priority descending
    /// (`ui.py`: `sort(key=lambda x: x.priority, reverse=True)`); this
    /// preserves that order within each kind. The sort is stable, so
    /// equal-priority plugins keep registration order rather than
    /// reordering unpredictably between calls.
    fn enabled_of_kind<T: ?Sized>(&self, extract: impl Fn(&RegisteredPlugin) -> Option<Arc<T>>) -> Vec<Arc<T>> {
        let mut matching: Vec<(u64, Arc<T>)> = self.entries.iter().filter(|e| e.enabled).filter_map(|e| extract(&e.plugin).map(|p| (e.plugin.as_plugin().priority(), p))).collect();
        matching.sort_by_key(|(priority, _)| std::cmp::Reverse(*priority));
        matching.into_iter().map(|(_, p)| p).collect()
    }

    pub fn file_type_plugins(&self) -> Vec<Arc<dyn FileTypePlugin>> {
        self.enabled_of_kind(|p| match p {
            RegisteredPlugin::FileType(p) => Some(Arc::clone(p)),
            _ => None,
        })
    }

    pub fn input_format_plugins(&self) -> Vec<Arc<dyn InputFormatPlugin>> {
        self.enabled_of_kind(|p| match p {
            RegisteredPlugin::InputFormat(p) => Some(Arc::clone(p)),
            _ => None,
        })
    }

    pub fn output_format_plugins(&self) -> Vec<Arc<dyn OutputFormatPlugin>> {
        self.enabled_of_kind(|p| match p {
            RegisteredPlugin::OutputFormat(p) => Some(Arc::clone(p)),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A real (if small) `FileTypePlugin`: it renames the file it is
    /// given, so a test can observe that it actually ran, in what order,
    /// and whether it was skipped.
    struct Marker {
        name: String,
        priority: u64,
        can_be_disabled: bool,
    }

    impl Marker {
        fn new(name: &str, priority: u64) -> Marker {
            Marker { name: name.to_string(), priority, can_be_disabled: true }
        }
        fn undisableable(name: &str) -> Marker {
            Marker { name: name.to_string(), priority: 1, can_be_disabled: false }
        }
    }

    impl Plugin for Marker {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "a test marker plugin"
        }
        fn priority(&self) -> u64 {
            self.priority
        }
        fn can_be_disabled(&self) -> bool {
            self.can_be_disabled
        }
    }

    impl FileTypePlugin for Marker {
        fn file_types(&self) -> Vec<String> {
            vec!["txt".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, path: &Path) -> PathBuf {
            path.with_extension(format!("{}.txt", self.name))
        }
    }

    fn file_type(m: Marker) -> RegisteredPlugin {
        RegisteredPlugin::FileType(Arc::new(m))
    }

    #[test]
    fn a_registered_plugin_is_discoverable_by_its_kind() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("A", 1))).unwrap();

        let found = reg.file_type_plugins();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), "A");
        // ...and is not returned as some other kind.
        assert!(reg.input_format_plugins().is_empty());
        assert!(reg.output_format_plugins().is_empty());
    }

    #[test]
    fn plugins_come_back_in_descending_priority_order_like_upstream() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("low", 1))).unwrap();
        reg.register(file_type(Marker::new("high", 100))).unwrap();
        reg.register(file_type(Marker::new("mid", 50))).unwrap();

        let names: Vec<String> = reg.file_type_plugins().iter().map(|p| p.name().to_string()).collect();
        assert_eq!(names, ["high", "mid", "low"]);
    }

    #[test]
    fn equal_priority_plugins_keep_registration_order() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("first", 7))).unwrap();
        reg.register(file_type(Marker::new("second", 7))).unwrap();

        let found = reg.file_type_plugins();
        assert_eq!(found[0].name(), "first");
        assert_eq!(found[1].name(), "second");
    }

    #[test]
    fn registering_a_duplicate_name_is_refused() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("dupe", 1))).unwrap();
        let err = reg.register(file_type(Marker::new("dupe", 9))).unwrap_err();
        assert_eq!(err, RegistryError::DuplicateName("dupe".to_string()));
        assert_eq!(reg.len(), 1, "the rejected plugin must not have been stored");
    }

    #[test]
    fn a_disabled_plugin_is_not_discoverable_but_is_still_listed() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("off", 1))).unwrap();
        reg.set_enabled("off", false).unwrap();

        assert!(reg.file_type_plugins().is_empty(), "a disabled plugin must not be handed out for execution");
        assert_eq!(reg.list().len(), 1, "but it must still be listed, so a UI can offer re-enabling it");
        assert_eq!(reg.is_enabled("off"), Some(false));
    }

    #[test]
    fn a_re_enabled_plugin_becomes_discoverable_again() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("toggle", 1))).unwrap();
        reg.set_enabled("toggle", false).unwrap();
        reg.set_enabled("toggle", true).unwrap();
        assert_eq!(reg.file_type_plugins().len(), 1);
    }

    #[test]
    fn a_plugin_that_declares_it_cannot_be_disabled_refuses_to_be() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::undisableable("essential"))).unwrap();

        let err = reg.set_enabled("essential", false).unwrap_err();
        assert_eq!(err, RegistryError::CannotBeDisabled("essential".to_string()));
        assert_eq!(reg.file_type_plugins().len(), 1, "it must still be active after the refused disable");
        // Enabling is always allowed -- the flag constrains only disabling.
        reg.set_enabled("essential", true).unwrap();
    }

    #[test]
    fn toggling_an_unknown_plugin_reports_not_found() {
        let mut reg = PluginRegistry::new();
        assert_eq!(reg.set_enabled("ghost", false).unwrap_err(), RegistryError::NotFound("ghost".to_string()));
        assert_eq!(reg.is_enabled("ghost"), None);
    }

    #[test]
    fn list_reports_real_metadata_in_priority_order() {
        let mut reg = PluginRegistry::new();
        reg.register(file_type(Marker::new("low", 1))).unwrap();
        reg.register(file_type(Marker::new("high", 10))).unwrap();

        let listed = reg.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "high");
        assert_eq!(listed[0].kind, "FileType");
        assert_eq!(listed[0].description, "a test marker plugin");
        assert!(listed[0].enabled);
        assert!(listed[0].can_be_disabled);
        assert_eq!(listed[1].name, "low");
    }

    #[test]
    fn an_empty_registry_is_empty_rather_than_reporting_plugins_that_do_not_exist() {
        // Guards the specific regression this issue fixed: `builtins.rs`
        // used to return three hardcoded plugin names for plugins that
        // were never registered and did not exist.
        let reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert!(reg.list().is_empty());
        assert!(reg.file_type_plugins().is_empty());
    }
}
