//! Ollama provider configuration.
//!
//! Port of `old_src/src/calibre/ai/ollama/config.py`. Ollama is a
//! local LLM runner, similar to LM Studio, but with per-request HTTP
//! headers and a live model-existence check via callback.

use std::collections::HashMap;

use serde_json::{json, Value};
use thiserror::Error;

use crate::ollama::{DEFAULT_URL, PLUGIN_NAME};
use crate::prefs::{pref_for_provider, set_prefs_for_provider};

const TIMEOUT_MIN: u32 = 15;
const TIMEOUT_MAX: u32 = 600;
const TIMEOUT_DEFAULT: u32 = 120;

#[derive(Debug, Error, PartialEq)]
pub enum OllamaConfigError {
    #[error("You must specify a model to use for text based tasks.")]
    MissingModel,
    #[error("Timeout {given} seconds is out of range ({}..={}).", TIMEOUT_MIN, TIMEOUT_MAX)]
    TimeoutOutOfRange { given: u32 },
    #[error("No model named `{name}` found in Ollama.")]
    ModelNotInstalled { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaConfig {
    pub api_url: String,
    pub timeout_s: u32,
    pub text_model: String,
    /// Ordered list of `(header_name, header_value)`. Kept as a Vec of
    /// tuples so the on-disk shape matches the Python tuple-of-tuples.
    pub headers: Vec<(String, String)>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            api_url: String::new(),
            timeout_s: TIMEOUT_DEFAULT,
            text_model: String::new(),
            headers: Vec::new(),
        }
    }
}

impl OllamaConfig {
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
        let text_model = pref_for_provider(PLUGIN_NAME, "text_model", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let headers = pref_for_provider(PLUGIN_NAME, "headers", None)
            .and_then(|v| parse_headers_from_value(&v))
            .unwrap_or_default();
        Self {
            api_url,
            timeout_s,
            text_model,
            headers,
        }
    }

    pub fn is_ready_for_use(&self) -> bool {
        !self.text_model.trim().is_empty()
    }

    /// Parse a UI-typed multiline "Header: Value" block into
    /// `(header, value)` pairs. Blank lines and lines missing a colon
    /// are silently dropped, matching the Python behavior.
    pub fn parse_headers_block(block: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for raw in block.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !v.is_empty() {
                out.push((k.to_string(), v.to_string()));
            }
        }
        out
    }

    /// Two-phase validation: (a) required-field checks, then (b) a
    /// caller-supplied predicate that hits the live Ollama server to
    /// check that `text_model` is actually installed. Callback style
    /// keeps the domain model pure and testable.
    pub fn validate_with<F>(&self, does_model_exist_locally: F) -> Result<(), OllamaConfigError>
    where
        F: FnOnce(&str, &str, &[(String, String)]) -> bool,
    {
        let name = self.text_model.trim();
        if name.is_empty() {
            return Err(OllamaConfigError::MissingModel);
        }
        if !(TIMEOUT_MIN..=TIMEOUT_MAX).contains(&self.timeout_s) {
            return Err(OllamaConfigError::TimeoutOutOfRange {
                given: self.timeout_s,
            });
        }
        if !does_model_exist_locally(name, self.effective_url(), &self.headers) {
            return Err(OllamaConfigError::ModelNotInstalled {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// Python `settings` shape. `api_url` and `headers` are only
    /// included when non-empty (Python `if url := ...` /
    /// `if headers := ...`).
    pub fn settings(&self) -> HashMap<String, Value> {
        let mut ans = HashMap::new();
        ans.insert("text_model".to_string(), json!(self.text_model.trim()));
        ans.insert("timeout".to_string(), json!(self.timeout_s));
        let url = self.api_url.trim();
        if !url.is_empty() {
            ans.insert("api_url".to_string(), json!(url));
        }
        if !self.headers.is_empty() {
            let pairs: Vec<Value> = self
                .headers
                .iter()
                .map(|(k, v)| json!([k, v]))
                .collect();
            ans.insert("headers".to_string(), Value::Array(pairs));
        }
        ans
    }

    pub fn commit_with<F>(&self, exists: F) -> Result<(), OllamaConfigError>
    where
        F: FnOnce(&str, &str, &[(String, String)]) -> bool,
    {
        self.validate_with(exists)?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings());
        Ok(())
    }
}

/// The prefs JSON stores headers as either an array-of-two-arrays
/// (`[["Header","Value"], ...]`, matching Python's tuple-of-tuples)
/// or as an array-of-objects. Accept both defensively.
fn parse_headers_from_value(v: &Value) -> Option<Vec<(String, String)>> {
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        if let Some(pair) = entry.as_array() {
            if pair.len() == 2 {
                if let (Some(k), Some(v)) = (pair[0].as_str(), pair[1].as_str()) {
                    out.push((k.to_string(), v.to_string()));
                    continue;
                }
            }
        }
        if let Some(obj) = entry.as_object() {
            if let (Some(k), Some(v)) = (
                obj.get("name").and_then(|x| x.as_str()),
                obj.get("value").and_then(|x| x.as_str()),
            ) {
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(model: &str) -> OllamaConfig {
        OllamaConfig {
            text_model: model.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_matches_python() {
        let c = OllamaConfig::default();
        assert_eq!(c.timeout_s, 120);
        assert_eq!(c.text_model, "");
        assert_eq!(c.api_url, "");
        assert!(c.headers.is_empty());
    }

    #[test]
    fn default_url_falls_back_to_localhost_11434() {
        assert_eq!(cfg("m").effective_url(), "http://localhost:11434");
    }

    #[test]
    fn parse_headers_block_drops_blank_and_malformed() {
        let block = "\
Authorization: Bearer abc

X-Trace: 42
malformed line without colon
:missing key
X-Empty:
Header-With-Colons: value: with: colons\n";
        let got = OllamaConfig::parse_headers_block(block);
        assert_eq!(
            got,
            vec![
                ("Authorization".to_string(), "Bearer abc".to_string()),
                ("X-Trace".to_string(), "42".to_string()),
                (
                    "Header-With-Colons".to_string(),
                    "value: with: colons".to_string()
                ),
            ]
        );
    }

    #[test]
    fn validate_missing_model_errors_without_calling_exists() {
        let err = cfg("")
            .validate_with(|_, _, _| unreachable!("exists must not be called"))
            .unwrap_err();
        assert_eq!(err, OllamaConfigError::MissingModel);
    }

    #[test]
    fn validate_timeout_out_of_range_errors_without_calling_exists() {
        let mut c = cfg("qwen2.5");
        c.timeout_s = 5;
        let err = c
            .validate_with(|_, _, _| unreachable!())
            .unwrap_err();
        assert!(matches!(err, OllamaConfigError::TimeoutOutOfRange { .. }));
    }

    #[test]
    fn validate_model_not_installed_error() {
        let err = cfg("qwen2.5")
            .validate_with(|_, _, _| false)
            .unwrap_err();
        assert_eq!(
            err,
            OllamaConfigError::ModelNotInstalled {
                name: "qwen2.5".into()
            }
        );
    }

    #[test]
    fn validate_ok_passes_url_and_headers_to_callback() {
        let mut c = cfg("qwen2.5");
        c.api_url = "http://custom:11434".into();
        c.headers = vec![("Authorization".into(), "Bearer x".into())];
        let mut seen_url = String::new();
        let mut seen_headers: Vec<(String, String)> = Vec::new();
        c.validate_with(|name, url, headers| {
            assert_eq!(name, "qwen2.5");
            seen_url = url.to_string();
            seen_headers = headers.to_vec();
            true
        })
        .unwrap();
        assert_eq!(seen_url, "http://custom:11434");
        assert_eq!(seen_headers.len(), 1);
    }

    #[test]
    fn settings_omits_api_url_and_headers_when_empty() {
        let s = cfg("qwen2.5").settings();
        assert!(s.get("api_url").is_none());
        assert!(s.get("headers").is_none());
        assert_eq!(s.get("text_model").unwrap().as_str().unwrap(), "qwen2.5");
    }

    #[test]
    fn settings_headers_shape_is_array_of_two_element_arrays() {
        let mut c = cfg("m");
        c.headers = vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())];
        let s = c.settings();
        let arr = s.get("headers").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], json!(["X-A", "1"]));
        assert_eq!(arr[1], json!(["X-B", "2"]));
    }

    #[test]
    fn parse_headers_from_value_accepts_tuple_style() {
        let v = json!([["X-A", "1"], ["X-B", "2"]]);
        assert_eq!(
            parse_headers_from_value(&v).unwrap(),
            vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())]
        );
    }

    #[test]
    fn parse_headers_from_value_accepts_object_style() {
        let v = json!([{"name": "X-A", "value": "1"}]);
        assert_eq!(
            parse_headers_from_value(&v).unwrap(),
            vec![("X-A".into(), "1".into())]
        );
    }
}
