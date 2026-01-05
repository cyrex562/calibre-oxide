use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginInstallationType {
    External = 1,
    System = 2,
    Builtin = 3,
}

pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> (u32, u32, u32) {
        (1, 0, 0)
    }
    fn description(&self) -> &str {
        "Does absolutely nothing"
    }
    fn author(&self) -> &str {
        "Unknown"
    }
    fn priority(&self) -> u64 {
        1
    }
    fn minimum_calibre_version(&self) -> (u32, u32, u32) {
        (0, 4, 118)
    }
    fn installation_type(&self) -> Option<PluginInstallationType> {
        None
    }
    fn can_be_disabled(&self) -> bool {
        true
    }
    fn type_name(&self) -> &str {
        "Base"
    }

    // Lifecycle methods
    fn initialize(&mut self) {}
    fn is_customizable(&self) -> bool {
        false
    }

    // In Python there's config_widget, save_settings etc. returning QWidgets.
    // In Rust we might return data definitions or trait objects for UI.
    // For now, we omit UI specific methods or stub them.
    fn customization_help(&self, _gui: bool) -> String {
        String::new()
    }
}

pub trait FileTypePlugin: Plugin {
    fn file_types(&self) -> Vec<String> {
        Vec::new()
    }
    fn on_import(&self) -> bool {
        false
    }
    fn on_postimport(&self) -> bool {
        false
    }
    fn on_postconvert(&self) -> bool {
        false
    }
    fn on_postdelete(&self) -> bool {
        false
    }
    fn on_preprocess(&self) -> bool {
        false
    }
    fn on_postprocess(&self) -> bool {
        false
    }

    fn run(&self, path_to_ebook: &std::path::Path) -> std::path::PathBuf {
        path_to_ebook.to_path_buf()
    }
}

pub mod builtins;
pub mod conversion;
pub mod profiles;
pub mod ui;
pub mod zipplugin;
