use calibre_customize::builtins::BuiltinPlugins;
use calibre_customize::ui::{InterfaceAction, StubInterfaceAction};
use calibre_customize::zipplugin::ZipPluginLoader;
use std::path::Path;

#[test]
fn test_builtins() {
    let builtins = BuiltinPlugins::new();
    let list = builtins.list_plugins();
    assert!(list.contains(&"MOBI Output".to_string()));
}

#[test]
fn test_zipplugin() {
    let loader = ZipPluginLoader::new();
    let p = Path::new("dummy.zip");
    assert!(loader.load_from_zip(p).is_ok());
}

#[test]
fn test_ui_stub() {
    let action = StubInterfaceAction::new("Test Action");
    assert_eq!(action.name(), "Test Action");
    action.show_main_window(); // Should print to stdout, mostly checking it runs
}
