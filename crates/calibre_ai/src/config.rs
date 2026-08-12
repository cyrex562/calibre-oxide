//! AI-provider configuration domain model.
//!
//! Port of `old_src/src/calibre/ai/config.py`. The original was a Qt
//! `QWidget` (`ConfigureAI`) that both (a) held the semantics of provider
//! selection and (b) rendered a combo box. In calibre-oxide the UI is
//! Tauri + Vue, so this module keeps only the *semantic* half — which
//! providers exist for a purpose, which is currently selected, how to
//! commit a new selection. Tauri commands in the app crate wrap these
//! for the front-end.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prefs::{
    available_ai_provider_plugins, plugin_for_purpose, plugins_for_purpose, AIProviderPlugin,
};
use crate::AICapabilities;

/// User-facing summary of a provider — safe to serialize across the
/// Tauri boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    /// The bit flags represented as their `.purpose()` string, e.g.
    /// `"AICapabilities.TEXT_TO_TEXT"`.
    pub capabilities: String,
}

impl ProviderInfo {
    fn from_plugin(p: &Arc<dyn AIProviderPlugin>) -> Self {
        Self {
            name: p.name().to_string(),
            capabilities: p.capabilities().purpose(),
        }
    }
}

/// Errors that a UI can surface directly to the user.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("no AI providers found for capability {capability}. Enable at least one provider plugin.")]
    NoProviders { capability: String },
    #[error("no provider named `{name}` supports capability {capability}")]
    UnknownProvider { name: String, capability: String },
}

/// Configuration state for a single AI purpose (e.g. `TEXT_TO_TEXT`).
///
/// Construction pulls the plugin list at that moment; it does not
/// re-read the global registry on every call. If a plugin is registered
/// later, call [`ConfigureAI::for_purpose`] again to see it.
pub struct ConfigureAI {
    purpose: AICapabilities,
    plugins: Vec<Arc<dyn AIProviderPlugin>>,
    /// Currently-selected plugin index into `plugins`. `None` iff
    /// `plugins.is_empty()`.
    selected: Option<usize>,
}

impl ConfigureAI {
    /// Build config from the global plugin registry. This is what the
    /// Tauri commands use in production.
    pub fn for_purpose(purpose: AICapabilities) -> Self {
        let plugins: Vec<Arc<dyn AIProviderPlugin>> = plugins_for_purpose(purpose).collect();
        Self::from_plugins(purpose, plugins)
    }

    /// Build config from an explicit plugin list. Intended for tests
    /// and for isolated command handlers that inject their own registry.
    pub fn from_plugins(purpose: AICapabilities, plugins: Vec<Arc<dyn AIProviderPlugin>>) -> Self {
        // Deterministic ordering — the UI relies on this so the combo
        // box doesn't shuffle between reloads.
        let mut plugins = plugins;
        plugins.sort_by(|a, b| a.name().cmp(b.name()));

        let selected = if plugins.is_empty() {
            None
        } else {
            let preferred = plugin_for_purpose(purpose).map(|p| p.name().to_string());
            let idx = preferred
                .as_deref()
                .and_then(|name| plugins.iter().position(|p| p.name() == name))
                .unwrap_or(0);
            Some(idx)
        };

        Self {
            purpose,
            plugins,
            selected,
        }
    }

    pub fn purpose(&self) -> AICapabilities {
        self.purpose
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.plugins.iter().map(ProviderInfo::from_plugin).collect()
    }

    pub fn selected(&self) -> Option<ProviderInfo> {
        self.selected
            .and_then(|i| self.plugins.get(i))
            .map(ProviderInfo::from_plugin)
    }

    /// Change the current selection to the named provider. Returns
    /// `UnknownProvider` if no such provider is registered for this
    /// purpose.
    pub fn select(&mut self, name: &str) -> Result<(), ConfigError> {
        let idx = self.plugins.iter().position(|p| p.name() == name).ok_or_else(|| {
            ConfigError::UnknownProvider {
                name: name.to_string(),
                capability: self.purpose.purpose(),
            }
        })?;
        self.selected = Some(idx);
        Ok(())
    }

    /// The Python `is_ready_for_use` property. False if no providers or
    /// if no provider is currently selected.
    pub fn is_ready_for_use(&self) -> bool {
        !self.plugins.is_empty() && self.selected.is_some()
    }

    /// The Python `validate` method. Reports a specific error suitable
    /// for a UI to display rather than a boolean like the original.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.plugins.is_empty() {
            return Err(ConfigError::NoProviders {
                capability: self.purpose.purpose(),
            });
        }
        Ok(())
    }

    /// The Python `commit` method: validate, then write the current
    /// selection back to the prefs `purpose_map`. The Python version
    /// also called `plugin.save_settings(widget)`; that's a per-plugin
    /// UI dialog concern and moves into the individual Vue components.
    pub fn commit(&self) -> Result<ProviderInfo, ConfigError> {
        self.validate()?;
        let info = self.selected().expect("validate passed => selected is Some");
        crate::prefs::set_purpose_selection(&self.purpose, &info.name);
        Ok(info)
    }
}

/// Number of registered providers currently visible in the global
/// registry (regardless of capability). Handy for the app to decide
/// whether to show the "add an AI provider" onboarding flow.
pub fn registered_provider_count() -> usize {
    available_ai_provider_plugins().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AICapabilities;

    struct MockPlugin {
        name: &'static str,
        caps: AICapabilities,
    }
    impl AIProviderPlugin for MockPlugin {
        fn name(&self) -> &str {
            self.name
        }
        fn capabilities(&self) -> AICapabilities {
            self.caps
        }
    }

    fn plugin(name: &'static str, caps: AICapabilities) -> Arc<dyn AIProviderPlugin> {
        Arc::new(MockPlugin { name, caps })
    }

    #[test]
    fn from_plugins_sorts_by_name() {
        let cfg = ConfigureAI::from_plugins(
            AICapabilities::TEXT_TO_TEXT,
            vec![
                plugin("Ollama", AICapabilities::TEXT_TO_TEXT),
                plugin("Google", AICapabilities::TEXT_TO_TEXT),
                plugin("OpenAI", AICapabilities::TEXT_TO_TEXT),
            ],
        );
        let names: Vec<String> = cfg.providers().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Google", "Ollama", "OpenAI"]);
    }

    #[test]
    fn selects_first_by_default_when_no_prefs_hint() {
        let cfg = ConfigureAI::from_plugins(
            AICapabilities::TEXT_TO_TEXT,
            vec![
                plugin("Zebra", AICapabilities::TEXT_TO_TEXT),
                plugin("Apple", AICapabilities::TEXT_TO_TEXT),
            ],
        );
        assert_eq!(cfg.selected().unwrap().name, "Apple");
    }

    #[test]
    fn no_providers_yields_none_selected() {
        let cfg = ConfigureAI::from_plugins(AICapabilities::TTS, Vec::new());
        assert!(cfg.selected().is_none());
        assert!(!cfg.is_ready_for_use());
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::NoProviders { .. })
        ));
    }

    #[test]
    fn select_switches_selection() {
        let mut cfg = ConfigureAI::from_plugins(
            AICapabilities::TEXT_TO_TEXT,
            vec![
                plugin("Google", AICapabilities::TEXT_TO_TEXT),
                plugin("OpenAI", AICapabilities::TEXT_TO_TEXT),
            ],
        );
        cfg.select("OpenAI").unwrap();
        assert_eq!(cfg.selected().unwrap().name, "OpenAI");
    }

    #[test]
    fn select_unknown_provider_errors() {
        let mut cfg = ConfigureAI::from_plugins(
            AICapabilities::TEXT_TO_TEXT,
            vec![plugin("Google", AICapabilities::TEXT_TO_TEXT)],
        );
        let err = cfg.select("Nonexistent").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownProvider { .. }));
        // Selection must be unchanged after a failed select.
        assert_eq!(cfg.selected().unwrap().name, "Google");
    }

    #[test]
    fn validate_passes_when_providers_present() {
        let cfg = ConfigureAI::from_plugins(
            AICapabilities::TEXT_TO_TEXT,
            vec![plugin("Google", AICapabilities::TEXT_TO_TEXT)],
        );
        assert!(cfg.validate().is_ok());
        assert!(cfg.is_ready_for_use());
    }

    #[test]
    fn providers_serialize_stably() {
        let info = ProviderInfo {
            name: "Google".to_string(),
            capabilities: "AICapabilities.TEXT_TO_TEXT".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(
            json,
            r#"{"name":"Google","capabilities":"AICapabilities.TEXT_TO_TEXT"}"#
        );
    }
}
