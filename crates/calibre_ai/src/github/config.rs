//! GitHub AI provider configuration.
//!
//! Port of `old_src/src/calibre/ai/github/config.py`. Same split as
//! `ai/config.rs`: this module owns the *semantic* half (validated form
//! state, load-from-prefs, commit-to-prefs); the Vue side owns the
//! actual form widget.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::github::PLUGIN_NAME;
use crate::prefs::{
    decode_secret, encode_secret, pref_for_provider, set_prefs_for_provider,
};
use crate::utils::ModelChoiceStrategy;

/// A user-chosen text model. Kept as `{name, id}` for round-trip
/// compatibility with the Python prefs shape (`text_model: {name, id}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TextModelPin {
    pub name: String,
    pub id: String,
}

impl TextModelPin {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.id.is_empty()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GitHubConfigError {
    #[error("You must supply a Personal access token to use GitHub AI.")]
    MissingApiKey,
    #[error("No model named `{name}` found on GitHub.")]
    NoMatchingModel { name: String },
    #[error("The name `{name}` matches more than one model on GitHub. Be more specific.")]
    AmbiguousModel { name: String },
}

/// Editable form state for the GitHub AI provider.
///
/// The Python original was a `QWidget` bag of fields plus a
/// `save_settings()`. This is the same bag as data + validation +
/// commit, without any DOM concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubConfig {
    pub api_key: String,
    pub model_choice_strategy: ModelChoiceStrategy,
    /// The name the user typed. Empty string means "auto-pick".
    pub text_model_name: String,
    /// The pin loaded from prefs at construction time. Used to
    /// short-circuit lookup when the user hasn't changed the name.
    initial_text_model: TextModelPin,
}

impl GitHubConfig {
    /// Read the current configuration from the global prefs store.
    pub fn from_prefs() -> Self {
        let api_key = pref_for_provider(PLUGIN_NAME, "api_key", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|hex| decode_secret(&hex).ok())
            .unwrap_or_default();

        let strategy_str = pref_for_provider(PLUGIN_NAME, "model_choice_strategy", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "medium".to_string());
        let model_choice_strategy = ModelChoiceStrategy::parse(&strategy_str);

        let initial_text_model = pref_for_provider(PLUGIN_NAME, "text_model", None)
            .and_then(|v| serde_json::from_value::<TextModelPin>(v).ok())
            .unwrap_or_default();

        Self {
            api_key,
            model_choice_strategy,
            text_model_name: initial_text_model.name.clone(),
            initial_text_model,
        }
    }

    /// The Python `is_ready_for_use` property.
    pub fn is_ready_for_use(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// Validate against the model lookup. The `resolve_model_ids`
    /// callback returns the model IDs matching a name — in production
    /// it hits the GitHub models cache; in tests it's mocked.
    ///
    /// Empty `text_model_name` means "auto-pick" and skips the model
    /// existence check entirely (matches Python behavior).
    pub fn validate_with<F>(&self, resolve_model_ids: F) -> Result<(), GitHubConfigError>
    where
        F: FnOnce(&str) -> Vec<String>,
    {
        if !self.is_ready_for_use() {
            return Err(GitHubConfigError::MissingApiKey);
        }
        let name = self.text_model_name.trim();
        if name.is_empty() {
            return Ok(());
        }
        // Short-circuit: if the user hasn't changed the name from the
        // initial pin, trust that pin (the pin's id was validated when
        // it was first stored).
        if name == self.initial_text_model.name {
            return Ok(());
        }
        let matches = resolve_model_ids(name);
        match matches.len() {
            0 => Err(GitHubConfigError::NoMatchingModel {
                name: name.to_string(),
            }),
            1 => Ok(()),
            _ => Err(GitHubConfigError::AmbiguousModel {
                name: name.to_string(),
            }),
        }
    }

    /// The Python `settings` property. Emits the exact shape the Python
    /// version wrote so an existing Calibre install can still read the
    /// prefs after a round-trip.
    ///
    /// If `text_model_name` is non-empty, `resolve_first_id` returns
    /// the first model ID matching it (see `validate_with` for the
    /// callback shape). If empty, `text_model` is omitted from the map.
    pub fn settings_with<F>(&self, resolve_first_id: F) -> HashMap<String, Value>
    where
        F: FnOnce(&str) -> Option<String>,
    {
        let mut ans = HashMap::new();
        ans.insert("api_key".to_string(), json!(encode_secret(&self.api_key)));
        ans.insert(
            "model_choice_strategy".to_string(),
            json!(self.model_choice_strategy.as_str()),
        );
        let name = self.text_model_name.trim();
        if !name.is_empty() {
            let id = if name == self.initial_text_model.name {
                self.initial_text_model.id.clone()
            } else {
                resolve_first_id(name).unwrap_or_default()
            };
            let pin = TextModelPin {
                name: name.to_string(),
                id,
            };
            ans.insert(
                "text_model".to_string(),
                serde_json::to_value(pin).expect("TextModelPin serializes"),
            );
        }
        ans
    }

    /// The Python `save_settings`: validate, then write into prefs.
    pub fn commit_with<V, R>(
        &self,
        validate_with: V,
        resolve_first_id: R,
    ) -> Result<(), GitHubConfigError>
    where
        V: FnOnce(&str) -> Vec<String>,
        R: FnOnce(&str) -> Option<String>,
    {
        self.validate_with(validate_with)?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings_with(resolve_first_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_parses_known_values() {
        assert_eq!(ModelChoiceStrategy::parse("low"), ModelChoiceStrategy::Low);
        assert_eq!(
            ModelChoiceStrategy::parse("medium"),
            ModelChoiceStrategy::Medium
        );
        assert_eq!(
            ModelChoiceStrategy::parse("high"),
            ModelChoiceStrategy::High
        );
    }

    #[test]
    fn strategy_falls_back_to_medium_on_unknown() {
        // Bit of a defensive default — the Python version would
        // silently pick "low" (index 0), which is a mild
        // trap-for-the-user. We prefer Medium as the safe default.
        assert_eq!(ModelChoiceStrategy::parse(""), ModelChoiceStrategy::Medium);
        assert_eq!(
            ModelChoiceStrategy::parse("nonsense"),
            ModelChoiceStrategy::Medium
        );
    }

    #[test]
    fn strategy_default_is_medium() {
        assert_eq!(ModelChoiceStrategy::default(), ModelChoiceStrategy::Medium);
    }

    #[test]
    fn validate_missing_key_errors() {
        let cfg = cfg_with_key("");
        let err = cfg.validate_with(|_| unreachable!()).unwrap_err();
        assert_eq!(err, GitHubConfigError::MissingApiKey);
    }

    #[test]
    fn validate_skips_model_lookup_when_name_empty() {
        let cfg = cfg_with_key("abc");
        // Callback must NOT be called.
        cfg.validate_with(|_| unreachable!()).unwrap();
    }

    #[test]
    fn validate_short_circuits_when_name_matches_pin() {
        let mut cfg = cfg_with_key("abc");
        cfg.text_model_name = "gpt-4o".into();
        cfg.initial_text_model = TextModelPin {
            name: "gpt-4o".into(),
            id: "openai/gpt-4o".into(),
        };
        // The pin already has an id; skip the lookup.
        cfg.validate_with(|_| unreachable!()).unwrap();
    }

    #[test]
    fn validate_rejects_unknown_model_name() {
        let mut cfg = cfg_with_key("abc");
        cfg.text_model_name = "no-such-model".into();
        let err = cfg.validate_with(|_| Vec::new()).unwrap_err();
        assert_eq!(
            err,
            GitHubConfigError::NoMatchingModel {
                name: "no-such-model".into()
            }
        );
    }

    #[test]
    fn validate_rejects_ambiguous_model_name() {
        let mut cfg = cfg_with_key("abc");
        cfg.text_model_name = "gpt".into();
        let err = cfg
            .validate_with(|_| vec!["openai/gpt-4o".into(), "openai/gpt-4o-mini".into()])
            .unwrap_err();
        assert!(matches!(
            err,
            GitHubConfigError::AmbiguousModel { .. }
        ));
    }

    #[test]
    fn validate_accepts_single_match() {
        let mut cfg = cfg_with_key("abc");
        cfg.text_model_name = "gpt-4o-mini".into();
        cfg.validate_with(|_| vec!["openai/gpt-4o-mini".into()])
            .unwrap();
    }

    #[test]
    fn settings_shape_matches_python() {
        let cfg = cfg_with_key("secret-token");
        let s = cfg.settings_with(|_| None);
        // api_key must be hex-encoded (per encode_secret) — verify we
        // can decode back to the original.
        let hex = s.get("api_key").unwrap().as_str().unwrap();
        assert_eq!(decode_secret(hex).unwrap(), "secret-token");
        assert_eq!(
            s.get("model_choice_strategy").unwrap().as_str().unwrap(),
            "medium"
        );
        assert!(s.get("text_model").is_none(), "empty text_model_name => omit key");
    }

    #[test]
    fn settings_includes_text_model_when_name_present() {
        let mut cfg = cfg_with_key("secret");
        cfg.text_model_name = "gpt-4o-mini".into();
        let s = cfg.settings_with(|_| Some("openai/gpt-4o-mini".to_string()));
        let pin: TextModelPin =
            serde_json::from_value(s.get("text_model").unwrap().clone()).unwrap();
        assert_eq!(pin.name, "gpt-4o-mini");
        assert_eq!(pin.id, "openai/gpt-4o-mini");
    }

    #[test]
    fn settings_reuses_initial_pin_id_when_name_unchanged() {
        let mut cfg = cfg_with_key("secret");
        cfg.text_model_name = "gpt-4o".into();
        cfg.initial_text_model = TextModelPin {
            name: "gpt-4o".into(),
            id: "openai/gpt-4o-preserved".into(),
        };
        // Resolver must NOT be called — we already have the id.
        let s = cfg.settings_with(|_| unreachable!());
        let pin: TextModelPin =
            serde_json::from_value(s.get("text_model").unwrap().clone()).unwrap();
        assert_eq!(pin.id, "openai/gpt-4o-preserved");
    }

    fn cfg_with_key(key: &str) -> GitHubConfig {
        GitHubConfig {
            api_key: key.to_string(),
            model_choice_strategy: ModelChoiceStrategy::Medium,
            text_model_name: String::new(),
            initial_text_model: TextModelPin::default(),
        }
    }
}
