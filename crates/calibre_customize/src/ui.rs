use crate::FileTypePlugin;
use std::path::{Path, PathBuf};

/// Port of `calibre.customize.ui._run_filetype_plugins`, specialized to
/// the `occasion='import'` case (`run_plugins_on_import` in the
/// Python).
///
/// Runs every plugin whose [`FileTypePlugin::file_types`] contains
/// `path`'s extension (or `"*"`) and whose [`FileTypePlugin::on_import`]
/// is true, in order, threading the output of each into the next. A
/// plugin that panics is treated the way the Python's `except Exception`
/// treats a raised exception: logged to stderr and skipped, keeping
/// whatever path the previous plugin (or the original file) produced.
///
/// There is no global plugin registry yet — calibre-oxide has no
/// built-in `FileTypePlugin`s wired up, matching the current state of
/// `calibre_customize::builtins`. Callers pass whichever plugins they
/// have; an empty slice reproduces the Python's no-plugins-registered
/// behavior exactly (the loop runs zero times and the original path is
/// returned unchanged).
pub fn run_plugins_on_import(path_to_file: &Path, plugins: &[Box<dyn FileTypePlugin>]) -> PathBuf {
    run_filetype_plugins(path_to_file, plugins, |p| p.on_import())
}

fn run_filetype_plugins(
    path_to_file: &Path,
    plugins: &[Box<dyn FileTypePlugin>],
    occasion: impl Fn(&dyn FileTypePlugin) -> bool,
) -> PathBuf {
    let ext = path_to_file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let mut nfp = path_to_file.to_path_buf();
    for plugin in plugins {
        if !occasion(plugin.as_ref()) {
            continue;
        }
        let types = plugin.file_types();
        if !types.iter().any(|t| t == &ext || t == "*") {
            continue;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.run(&nfp)));
        match result {
            Ok(path) => nfp = path,
            Err(_) => {
                eprintln!(
                    "Running file type plugin {} failed with traceback:",
                    plugin.name()
                );
            }
        }
    }
    nfp
}

pub trait InterfaceAction {
    fn name(&self) -> &str;
    fn show_main_window(&self);
}

pub struct StubInterfaceAction {
    name: String,
}

impl StubInterfaceAction {
    pub fn new(name: &str) -> Self {
        StubInterfaceAction {
            name: name.to_string(),
        }
    }
}

impl InterfaceAction for StubInterfaceAction {
    fn name(&self) -> &str {
        &self.name
    }

    fn show_main_window(&self) {
        // Stub: Perform GUI action
        println!("Showing main window for action: {}", self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Renames the file to `<stem>.replaced`, mirroring the
    /// `import_test` fixture in `db/tests/add_remove.py`.
    struct RenamingImportPlugin {
        calls: std::sync::Arc<AtomicUsize>,
    }

    impl Plugin for RenamingImportPlugin {
        fn name(&self) -> &str {
            "renaming import plugin"
        }
    }

    impl FileTypePlugin for RenamingImportPlugin {
        fn file_types(&self) -> Vec<String> {
            vec!["txt".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, path_to_ebook: &Path) -> PathBuf {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let renamed = path_to_ebook.with_extension("replaced");
            std::fs::copy(path_to_ebook, &renamed).expect("copy");
            renamed
        }
    }

    struct PreprocessOnlyPlugin;

    impl Plugin for PreprocessOnlyPlugin {
        fn name(&self) -> &str {
            "preprocess only"
        }
    }

    impl FileTypePlugin for PreprocessOnlyPlugin {
        fn file_types(&self) -> Vec<String> {
            vec!["txt".to_string()]
        }
        fn on_import(&self) -> bool {
            false
        }
        fn run(&self, _path_to_ebook: &Path) -> PathBuf {
            panic!("must not run on import");
        }
    }

    struct WrongTypePlugin;

    impl Plugin for WrongTypePlugin {
        fn name(&self) -> &str {
            "wrong type"
        }
    }

    impl FileTypePlugin for WrongTypePlugin {
        fn file_types(&self) -> Vec<String> {
            vec!["epub".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, _path_to_ebook: &Path) -> PathBuf {
            panic!("must not run for a non-matching extension");
        }
    }

    struct PanickingPlugin;

    impl Plugin for PanickingPlugin {
        fn name(&self) -> &str {
            "panicking plugin"
        }
    }

    impl FileTypePlugin for PanickingPlugin {
        fn file_types(&self) -> Vec<String> {
            vec!["*".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, _path_to_ebook: &Path) -> PathBuf {
            panic!("boom");
        }
    }

    #[test]
    fn with_no_plugins_the_original_path_is_returned() {
        let path = Path::new("/tmp/book.txt");
        assert_eq!(run_plugins_on_import(path, &[]), path);
    }

    #[test]
    fn a_matching_import_plugin_transforms_the_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("book.txt");
        std::fs::write(&src, b"hello").expect("write");

        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(RenamingImportPlugin {
            calls: calls.clone(),
        })];
        let out = run_plugins_on_import(&src, &plugins);
        assert_eq!(out, src.with_extension("replaced"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn plugins_not_registered_for_import_are_skipped() {
        let path = Path::new("/tmp/book.txt");
        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(PreprocessOnlyPlugin)];
        // Would panic if it ran; reaching this line is the assertion.
        assert_eq!(run_plugins_on_import(path, &plugins), path);
    }

    #[test]
    fn plugins_for_other_extensions_are_skipped() {
        let path = Path::new("/tmp/book.txt");
        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(WrongTypePlugin)];
        assert_eq!(run_plugins_on_import(path, &plugins), path);
    }

    #[test]
    fn a_plugin_that_panics_is_skipped_not_propagated() {
        let path = Path::new("/tmp/book.txt");
        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(PanickingPlugin)];
        // The Python catches the exception, prints a traceback, and
        // keeps the path unchanged; this must not unwind past here.
        assert_eq!(run_plugins_on_import(path, &plugins), path);
    }

    #[test]
    fn a_wildcard_plugin_matches_every_extension() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("book.txt");
        std::fs::write(&src, b"hello").expect("write");

        struct WildcardRename;
        impl Plugin for WildcardRename {
            fn name(&self) -> &str {
                "wildcard"
            }
        }
        impl FileTypePlugin for WildcardRename {
            fn file_types(&self) -> Vec<String> {
                vec!["*".to_string()]
            }
            fn on_import(&self) -> bool {
                true
            }
            fn run(&self, path_to_ebook: &Path) -> PathBuf {
                let renamed = path_to_ebook.with_extension("wild");
                std::fs::copy(path_to_ebook, &renamed).expect("copy");
                renamed
            }
        }

        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(WildcardRename)];
        let out = run_plugins_on_import(&src, &plugins);
        assert_eq!(out, src.with_extension("wild"));
    }
}
