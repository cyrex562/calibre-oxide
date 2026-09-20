//! Integration-level checks of the plugin registry's public API
//! (issue #795).
//!
//! These replace three earlier tests that asserted stubs behaved like
//! stubs -- `BuiltinPlugins::list_plugins()` contained a hardcoded
//! string, `ZipPluginLoader::load_from_zip()` returned `Ok(())` for a
//! path that did not exist, and a `StubInterfaceAction` returned the
//! name it was constructed with. All three passed while testing
//! nothing, and two of the three types they covered were no-op stubs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use calibre_customize::registry::{PluginRegistry, RegisteredPlugin, RegistryError};
use calibre_customize::ui::run_plugins_on_import_from_registry;
use calibre_customize::{FileTypePlugin, Plugin, PluginInstallationType};

/// A real `FileTypePlugin` that actually rewrites the file it is given,
/// so the assertions below observe real work rather than a marker.
struct UppercasePlugin;

impl Plugin for UppercasePlugin {
    fn name(&self) -> &str {
        "Uppercase TXT"
    }
    fn description(&self) -> &str {
        "Rewrites an imported .txt file in upper case"
    }
    fn author(&self) -> &str {
        "calibre-oxide tests"
    }
    fn version(&self) -> (u32, u32, u32) {
        (2, 1, 0)
    }
    fn installation_type(&self) -> Option<PluginInstallationType> {
        Some(PluginInstallationType::Builtin)
    }
}

impl FileTypePlugin for UppercasePlugin {
    fn file_types(&self) -> Vec<String> {
        vec!["txt".to_string()]
    }
    fn on_import(&self) -> bool {
        true
    }
    fn run(&self, path: &Path) -> PathBuf {
        let contents = std::fs::read_to_string(path).expect("read");
        let out = path.with_extension("upper.txt");
        std::fs::write(&out, contents.to_uppercase()).expect("write");
        out
    }
}

#[test]
fn a_registered_plugin_really_transforms_a_real_file_on_import() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "hello plugin").expect("write");

    let mut registry = PluginRegistry::new();
    registry.register(RegisteredPlugin::FileType(Arc::new(UppercasePlugin))).expect("register");

    let out = run_plugins_on_import_from_registry(&src, &registry);

    assert_ne!(out, src, "the plugin should have produced a new path");
    assert_eq!(std::fs::read_to_string(&out).expect("read output"), "HELLO PLUGIN");
}

#[test]
fn a_disabled_plugin_really_does_not_touch_the_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("book.txt");
    std::fs::write(&src, "hello plugin").expect("write");

    let mut registry = PluginRegistry::new();
    registry.register(RegisteredPlugin::FileType(Arc::new(UppercasePlugin))).expect("register");
    registry.set_enabled("Uppercase TXT", false).expect("disable");

    let out = run_plugins_on_import_from_registry(&src, &registry);

    assert_eq!(out, src, "a disabled plugin must leave the path untouched");
    assert_eq!(std::fs::read_to_string(&src).expect("read"), "hello plugin", "and must not have rewritten the file");
}

#[test]
fn the_registry_reports_the_plugins_real_metadata() {
    let mut registry = PluginRegistry::new();
    registry.register(RegisteredPlugin::FileType(Arc::new(UppercasePlugin))).expect("register");

    let listed = registry.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Uppercase TXT");
    assert_eq!(listed[0].version, (2, 1, 0));
    assert_eq!(listed[0].author, "calibre-oxide tests");
    assert_eq!(listed[0].kind, "FileType");
    assert_eq!(listed[0].installation_type, Some(PluginInstallationType::Builtin));
    assert!(listed[0].enabled);
}

#[test]
fn registering_the_same_plugin_name_twice_is_refused() {
    let mut registry = PluginRegistry::new();
    registry.register(RegisteredPlugin::FileType(Arc::new(UppercasePlugin))).expect("register");

    let err = registry.register(RegisteredPlugin::FileType(Arc::new(UppercasePlugin))).unwrap_err();
    assert_eq!(err, RegistryError::DuplicateName("Uppercase TXT".to_string()));
    assert_eq!(registry.len(), 1);
}

#[test]
fn this_crates_own_builtins_register_nothing_rather_than_inventing_names() {
    // The old stub this replaces claimed "MOBI Output", "EPUB Output"
    // and "PDF Output" existed. They did not. See `builtins`'s own doc
    // for why format plugins are registered from their owning crate.
    let mut registry = PluginRegistry::new();
    calibre_customize::builtins::register_builtins(&mut registry);
    assert!(registry.is_empty());
}
