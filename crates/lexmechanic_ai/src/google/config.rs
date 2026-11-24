//! Google AI provider configuration.
//!
//! Port of `old_src/src/calibre/ai/google/config.py`. Same split as
//! `ai/config.rs` and `ai/github/config.rs` — this owns the semantic
//! half; Vue owns the widget.
//!
//! Simpler than the GitHub port: no text-model-name lookup, so no
//! callback-based resolver plumbing is needed.

use std::collections::HashMap;

use serde_json::{json, Value};
use thiserror::Error;

use crate::google::PLUGIN_NAME;
use crate::prefs::{decode_secret, encode_secret, pref_for_provider, set_prefs_for_provider};
use crate::utils::{ModelChoiceStrategy, ReasoningStrategy};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GoogleConfigError {
    #[error("You must supply an API key to use Google AI.")]
    MissingApiKey,
}

/// Editable form state for the Google AI provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleConfig {
    pub api_key: String,
    pub model_choice_strategy: ModelChoiceStrategy,
    pub reasoning_strategy: ReasoningStrategy,
    pub allow_web_searches: bool,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model_choice_strategy: ModelChoiceStrategy::default(),
            reasoning_strategy: ReasoningStrategy::default(),
            // Matches the Python `pref('allow_web_searches', True)`.
            allow_web_searches: true,
        }
    }
}

impl GoogleConfig {
    /// Read the current configuration from the global prefs store.
    pub fn from_prefs() -> Self {
        let api_key = pref_for_provider(PLUGIN_NAME, "api_key", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|hex| decode_secret(&hex).ok())
            .unwrap_or_default();

        let model_choice_strategy = pref_for_provider(PLUGIN_NAME, "model_choice_strategy", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .map(|s| ModelChoiceStrategy::parse(&s))
            .unwrap_or_default();

        let reasoning_strategy = pref_for_provider(PLUGIN_NAME, "reasoning_strategy", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .map(|s| ReasoningStrategy::parse(&s))
            .unwrap_or_default();

        let allow_web_searches = pref_for_provider(PLUGIN_NAME, "allow_web_searches", None)
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Self {
            api_key,
            model_choice_strategy,
            reasoning_strategy,
            allow_web_searches,
        }
    }

    pub fn is_ready_for_use(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub fn validate(&self) -> Result<(), GoogleConfigError> {
        if !self.is_ready_for_use() {
            return Err(GoogleConfigError::MissingApiKey);
        }
        Ok(())
    }

    /// The Python `settings` property. Emits the exact shape the Python
    /// version wrote so an existing Calibre install can still read the
    /// prefs after a round-trip.
    pub fn settings(&self) -> HashMap<String, Value> {
        let mut ans = HashMap::new();
        ans.insert("api_key".to_string(), json!(encode_secret(&self.api_key)));
        ans.insert(
            "model_choice_strategy".to_string(),
            json!(self.model_choice_strategy.as_str()),
        );
        ans.insert(
            "reasoning_strategy".to_string(),
            json!(self.reasoning_strategy.as_str()),
        );
        ans.insert(
            "allow_web_searches".to_string(),
            json!(self.allow_web_searches),
        );
        ans
    }

    /// The Python `save_settings`: validate, then write.
    pub fn commit(&self) -> Result<(), GoogleConfigError> {
        self.validate()?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(key: &str) -> GoogleConfig {
        GoogleConfig {
            api_key: key.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_has_web_searches_on() {
        let c = GoogleConfig::default();
        assert!(c.allow_web_searches, "Python default is True");
        assert_eq!(c.model_choice_strategy, ModelChoiceStrategy::Medium);
        assert_eq!(c.reasoning_strategy, ReasoningStrategy::Auto);
        assert_eq!(c.api_key, "");
    }

    #[test]
    fn validate_missing_key_errors() {
        let err = cfg("").validate().unwrap_err();
        assert_eq!(err, GoogleConfigError::MissingApiKey);
    }

    #[test]
    fn validate_key_with_only_whitespace_errors() {
        // A common footgun: user pastes spaces. Trim must catch it.
        let err = cfg("   \t\n").validate().unwrap_err();
        assert_eq!(err, GoogleConfigError::MissingApiKey);
    }

    #[test]
    fn validate_passes_with_key() {
        cfg("abc").validate().unwrap();
    }

    #[test]
    fn settings_round_trips_api_key_via_hex() {
        let s = cfg("secret-google-key").settings();
        let hex = s.get("api_key").unwrap().as_str().unwrap();
        assert_eq!(decode_secret(hex).unwrap(), "secret-google-key");
    }

    #[test]
    fn settings_contains_all_four_fields_in_python_shape() {
        let mut c = cfg("k");
        c.model_choice_strategy = ModelChoiceStrategy::High;
        c.reasoning_strategy = ReasoningStrategy::None;
        c.allow_web_searches = false;
        let s = c.settings();
        assert_eq!(
            s.get("model_choice_strategy").unwrap().as_str().unwrap(),
            "high"
        );
        assert_eq!(
            s.get("reasoning_strategy").unwrap().as_str().unwrap(),
            "none"
        );
        assert_eq!(s.get("allow_web_searches").unwrap().as_bool().unwrap(), false);
        // No extra unexpected keys.
        assert_eq!(s.len(), 4, "settings = {:?}", s);
    }

    #[test]
    fn reasoning_strategy_parses_all_values() {
        for (input, expected) in [
            ("auto", ReasoningStrategy::Auto),
            ("low", ReasoningStrategy::Low),
            ("medium", ReasoningStrategy::Medium),
            ("high", ReasoningStrategy::High),
            ("none", ReasoningStrategy::None),
        ] {
            assert_eq!(ReasoningStrategy::parse(input), expected, "input {}", input);
        }
    }

    #[test]
    fn reasoning_strategy_unknown_falls_back_to_auto() {
        assert_eq!(ReasoningStrategy::parse("nonsense"), ReasoningStrategy::Auto);
        assert_eq!(ReasoningStrategy::parse(""), ReasoningStrategy::Auto);
    }
}
