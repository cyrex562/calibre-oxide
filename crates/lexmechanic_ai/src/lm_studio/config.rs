//! LM Studio provider configuration.
//!
//! Port of `old_src/src/calibre/ai/lm_studio/config.py`. LM Studio
//! is a local LLM server — no API key, but a required model ID and
//! optional URL/timeout/temperature.

use std::collections::HashMap;

use serde_json::{json, Value};
use thiserror::Error;

use crate::lm_studio::{DEFAULT_URL, PLUGIN_NAME};
use crate::prefs::{pref_for_provider, set_prefs_for_provider};

/// Python range: 15..=600 seconds; default 120.
const TIMEOUT_MIN: u32 = 15;
const TIMEOUT_MAX: u32 = 600;
const TIMEOUT_DEFAULT: u32 = 120;

/// Python range: 0.0..=2.0; default 0.7.
const TEMPERATURE_MIN: f64 = 0.0;
const TEMPERATURE_MAX: f64 = 2.0;
const TEMPERATURE_DEFAULT: f64 = 0.7;

#[derive(Debug, Error, PartialEq)]
pub enum LmStudioConfigError {
    #[error("You must specify a model ID.")]
    MissingModel,
    #[error("Timeout {given} seconds is out of range ({}..={}).", TIMEOUT_MIN, TIMEOUT_MAX)]
    TimeoutOutOfRange { given: u32 },
    #[error("Temperature {given} is out of range ({}..={}).", TEMPERATURE_MIN, TEMPERATURE_MAX)]
    TemperatureOutOfRange { given: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LmStudioConfig {
    /// Optional. Empty string means "use LMStudioAI::DEFAULT_URL".
    pub api_url: String,
    /// Range 15..=600 s.
    pub timeout_s: u32,
    /// Range 0.0..=2.0.
    pub temperature: f64,
    /// Required. Empty is a validation error.
    pub text_model: String,
}

impl Default for LmStudioConfig {
    fn default() -> Self {
        Self {
            api_url: String::new(),
            timeout_s: TIMEOUT_DEFAULT,
            temperature: TEMPERATURE_DEFAULT,
            text_model: String::new(),
        }
    }
}

impl LmStudioConfig {
    pub fn default_url() -> &'static str {
        DEFAULT_URL
    }

    pub fn effective_url(&self) -> &str {
        if self.api_url.is_empty() {
            DEFAULT_URL
        } else {
            &self.api_url
        }
    }

    pub fn from_prefs() -> Self {
        let api_url = pref_for_provider(PLUGIN_NAME, "api_url", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let timeout_s = pref_for_provider(PLUGIN_NAME, "timeout", None)
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(TIMEOUT_DEFAULT);
        let temperature = pref_for_provider(PLUGIN_NAME, "temperature", None)
            .and_then(|v| v.as_f64())
            .unwrap_or(TEMPERATURE_DEFAULT);
        let text_model = pref_for_provider(PLUGIN_NAME, "text_model", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        Self {
            api_url,
            timeout_s,
            temperature,
            text_model,
        }
    }

    pub fn is_ready_for_use(&self) -> bool {
        !self.text_model.trim().is_empty()
    }

    pub fn validate(&self) -> Result<(), LmStudioConfigError> {
        if self.text_model.trim().is_empty() {
            return Err(LmStudioConfigError::MissingModel);
        }
        if !(TIMEOUT_MIN..=TIMEOUT_MAX).contains(&self.timeout_s) {
            return Err(LmStudioConfigError::TimeoutOutOfRange {
                given: self.timeout_s,
            });
        }
        if !(TEMPERATURE_MIN..=TEMPERATURE_MAX).contains(&self.temperature) {
            return Err(LmStudioConfigError::TemperatureOutOfRange {
                given: self.temperature,
            });
        }
        Ok(())
    }

    /// Match the Python `settings` shape byte-for-byte: `api_url` is
    /// only included when non-empty (Python `if url := self.api_url`).
    pub fn settings(&self) -> HashMap<String, Value> {
        let mut ans = HashMap::new();
        ans.insert("text_model".to_string(), json!(self.text_model.trim()));
        ans.insert("timeout".to_string(), json!(self.timeout_s));
        ans.insert("temperature".to_string(), json!(self.temperature));
        let url = self.api_url.trim();
        if !url.is_empty() {
            ans.insert("api_url".to_string(), json!(url));
        }
        ans
    }

    pub fn commit(&self) -> Result<(), LmStudioConfigError> {
        self.validate()?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(model: &str) -> LmStudioConfig {
        LmStudioConfig {
            text_model: model.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_matches_python() {
        let c = LmStudioConfig::default();
        assert_eq!(c.timeout_s, 120);
        assert!((c.temperature - 0.7).abs() < f64::EPSILON);
        assert_eq!(c.text_model, "");
        assert_eq!(c.api_url, "");
    }

    #[test]
    fn default_url_falls_back() {
        assert_eq!(cfg("m").effective_url(), "http://localhost:1234");
        let mut c = cfg("m");
        c.api_url = "http://elsewhere:1234".into();
        assert_eq!(c.effective_url(), "http://elsewhere:1234");
    }

    #[test]
    fn validate_missing_model_errors() {
        assert_eq!(cfg("").validate(), Err(LmStudioConfigError::MissingModel));
        assert_eq!(cfg("   ").validate(), Err(LmStudioConfigError::MissingModel));
    }

    #[test]
    fn validate_timeout_bounds() {
        let mut c = cfg("m");
        c.timeout_s = TIMEOUT_MIN - 1;
        assert!(matches!(
            c.validate(),
            Err(LmStudioConfigError::TimeoutOutOfRange { .. })
        ));
        c.timeout_s = TIMEOUT_MAX + 1;
        assert!(matches!(
            c.validate(),
            Err(LmStudioConfigError::TimeoutOutOfRange { .. })
        ));
        c.timeout_s = TIMEOUT_MIN;
        assert!(c.validate().is_ok());
        c.timeout_s = TIMEOUT_MAX;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_temperature_bounds() {
        let mut c = cfg("m");
        c.temperature = -0.1;
        assert!(matches!(
            c.validate(),
            Err(LmStudioConfigError::TemperatureOutOfRange { .. })
        ));
        c.temperature = 2.1;
        assert!(matches!(
            c.validate(),
            Err(LmStudioConfigError::TemperatureOutOfRange { .. })
        ));
        c.temperature = 0.0;
        assert!(c.validate().is_ok());
        c.temperature = 2.0;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn settings_omits_api_url_when_empty() {
        let s = cfg("qwen2.5").settings();
        assert!(s.get("api_url").is_none());
        assert_eq!(s.get("text_model").unwrap().as_str().unwrap(), "qwen2.5");
        assert_eq!(s.get("timeout").unwrap().as_u64().unwrap(), 120);
    }

    #[test]
    fn settings_includes_api_url_when_present() {
        let mut c = cfg("m");
        c.api_url = "http://custom:1234".into();
        let s = c.settings();
        assert_eq!(s.get("api_url").unwrap().as_str().unwrap(), "http://custom:1234");
    }

    #[test]
    fn settings_trims_model_and_url() {
        let mut c = cfg("  qwen2.5  ");
        c.api_url = "  http://x:1234  ".into();
        let s = c.settings();
        assert_eq!(s.get("text_model").unwrap().as_str().unwrap(), "qwen2.5");
        assert_eq!(s.get("api_url").unwrap().as_str().unwrap(), "http://x:1234");
    }
}
