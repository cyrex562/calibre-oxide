//! OpenAI provider configuration.
//!
//! Port of `old_src/src/calibre/ai/openai/config.py`. Structurally
//! identical to `google::config` — same four fields (api_key,
//! model_choice_strategy, reasoning_strategy, allow_web_searches).

use std::collections::HashMap;

use serde_json::{json, Value};
use thiserror::Error;

use crate::openai::PLUGIN_NAME;
use crate::prefs::{decode_secret, encode_secret, pref_for_provider, set_prefs_for_provider};
use crate::utils::{ModelChoiceStrategy, ReasoningStrategy};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpenAiConfigError {
    #[error("You must supply an API key to use OpenAI.")]
    MissingApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub model_choice_strategy: ModelChoiceStrategy,
    pub reasoning_strategy: ReasoningStrategy,
    pub allow_web_searches: bool,
}

impl Default for OpenAiConfig {
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

impl OpenAiConfig {
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

    pub fn validate(&self) -> Result<(), OpenAiConfigError> {
        if !self.is_ready_for_use() {
            return Err(OpenAiConfigError::MissingApiKey);
        }
        Ok(())
    }

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

    pub fn commit(&self) -> Result<(), OpenAiConfigError> {
        self.validate()?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_python_defaults() {
        let c = OpenAiConfig::default();
        assert!(c.allow_web_searches);
        assert_eq!(c.model_choice_strategy, ModelChoiceStrategy::Medium);
        assert_eq!(c.reasoning_strategy, ReasoningStrategy::Auto);
    }

    #[test]
    fn validate_rejects_missing_or_whitespace_key() {
        for bad in ["", "   ", "\t\n"] {
            let c = OpenAiConfig {
                api_key: bad.to_string(),
                ..Default::default()
            };
            assert_eq!(c.validate(), Err(OpenAiConfigError::MissingApiKey));
        }
    }

    #[test]
    fn validate_accepts_key() {
        let c = OpenAiConfig {
            api_key: "sk-abc".into(),
            ..Default::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn settings_shape_and_hex_round_trip() {
        let c = OpenAiConfig {
            api_key: "sk-test".into(),
            model_choice_strategy: ModelChoiceStrategy::High,
            reasoning_strategy: ReasoningStrategy::None,
            allow_web_searches: false,
        };
        let s = c.settings();
        assert_eq!(s.len(), 4);
        let hex = s.get("api_key").unwrap().as_str().unwrap();
        assert_eq!(decode_secret(hex).unwrap(), "sk-test");
        assert_eq!(
            s.get("model_choice_strategy").unwrap().as_str().unwrap(),
            "high"
        );
        assert_eq!(
            s.get("reasoning_strategy").unwrap().as_str().unwrap(),
            "none"
        );
        assert_eq!(s.get("allow_web_searches").unwrap().as_bool().unwrap(), false);
    }
}
