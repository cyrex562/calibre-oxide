//! OpenRouter provider configuration.
//!
//! Port of `old_src/src/calibre/ai/open_router/config.py`. The Python
//! file is large (484 LOC) but most is the Qt model-picker dialog
//! (`Model`, `ModelsModel`, `ProxyModels`, `ChooseModel`). This port
//! covers the semantic core: the `ConfigWidget` fields, validation,
//! and settings serialization. The picker UI lives in Vue.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::open_router::PLUGIN_NAME;
use crate::prefs::{decode_secret, encode_secret, pref_for_provider, set_prefs_for_provider};
use crate::utils::ReasoningStrategy;

/// OpenRouter uses a different set of model-choice strategies than
/// GitHub/Google/OpenAI (`low`/`medium`/`high`). Its values are:
/// `free-only`, `free-or-paid`, `native`. Kept in the provider module
/// because the semantics are provider-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenRouterModelChoiceStrategy {
    FreeOnly,
    FreeOrPaid,
    Native,
}

impl Default for OpenRouterModelChoiceStrategy {
    fn default() -> Self {
        // Matches Python `pref('model_choice_strategy', 'free-or-paid')`.
        OpenRouterModelChoiceStrategy::FreeOrPaid
    }
}

impl OpenRouterModelChoiceStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpenRouterModelChoiceStrategy::FreeOnly => "free-only",
            OpenRouterModelChoiceStrategy::FreeOrPaid => "free-or-paid",
            OpenRouterModelChoiceStrategy::Native => "native",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "free-only" => OpenRouterModelChoiceStrategy::FreeOnly,
            "native" => OpenRouterModelChoiceStrategy::Native,
            // Any unknown value → the safer default.
            _ => OpenRouterModelChoiceStrategy::FreeOrPaid,
        }
    }

    pub fn human_label(&self) -> &'static str {
        match self {
            OpenRouterModelChoiceStrategy::FreeOnly => "Free only",
            OpenRouterModelChoiceStrategy::FreeOrPaid => "Free or paid",
            OpenRouterModelChoiceStrategy::Native => "High quality",
        }
    }
}

/// Whether the user has opted in to providers that may store prompts.
/// String-round-trips as `"allow"` / `"deny"` matching Python.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataCollection {
    Allow,
    Deny,
}

impl Default for DataCollection {
    fn default() -> Self {
        // Matches Python `pref('data_collection', 'deny')`.
        DataCollection::Deny
    }
}

impl DataCollection {
    pub fn as_str(&self) -> &'static str {
        match self {
            DataCollection::Allow => "allow",
            DataCollection::Deny => "deny",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "allow" => DataCollection::Allow,
            _ => DataCollection::Deny,
        }
    }

    pub fn allows(&self) -> bool {
        matches!(self, DataCollection::Allow)
    }
}

/// A pinned OpenRouter text model. On disk it's a 2-tuple
/// `(model_id, model_name)` — this struct serializes the same way via
/// the `Serialize`/`Deserialize` impls below.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenRouterTextModel {
    pub id: String,
    pub name: String,
}

impl Serialize for OpenRouterTextModel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut t = serializer.serialize_tuple(2)?;
        t.serialize_element(&self.id)?;
        t.serialize_element(&self.name)?;
        t.end()
    }
}

impl<'de> Deserialize<'de> for OpenRouterTextModel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (id, name): (String, String) = Deserialize::deserialize(deserializer)?;
        Ok(OpenRouterTextModel { id, name })
    }
}

impl OpenRouterTextModel {
    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpenRouterConfigError {
    #[error("You must supply an API key to use OpenRouter. Remember to also buy a few credits, even if you plan on using only free models.")]
    MissingApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub model_choice_strategy: OpenRouterModelChoiceStrategy,
    pub reasoning_strategy: ReasoningStrategy,
    pub data_collection: DataCollection,
    pub allow_web_searches: bool,
    pub text_model: OpenRouterTextModel,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model_choice_strategy: OpenRouterModelChoiceStrategy::default(),
            reasoning_strategy: ReasoningStrategy::default(),
            data_collection: DataCollection::default(),
            // Different from Google/OpenAI — Python default is False here.
            allow_web_searches: false,
            text_model: OpenRouterTextModel::default(),
        }
    }
}

impl OpenRouterConfig {
    pub fn from_prefs() -> Self {
        let api_key = pref_for_provider(PLUGIN_NAME, "api_key", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|hex| decode_secret(&hex).ok())
            .unwrap_or_default();
        let model_choice_strategy = pref_for_provider(PLUGIN_NAME, "model_choice_strategy", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .map(|s| OpenRouterModelChoiceStrategy::parse(&s))
            .unwrap_or_default();
        let reasoning_strategy = pref_for_provider(PLUGIN_NAME, "reasoning_strategy", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .map(|s| ReasoningStrategy::parse(&s))
            .unwrap_or_default();
        let data_collection = pref_for_provider(PLUGIN_NAME, "data_collection", None)
            .and_then(|v| v.as_str().map(str::to_string))
            .map(|s| DataCollection::parse(&s))
            .unwrap_or_default();
        let allow_web_searches = pref_for_provider(PLUGIN_NAME, "allow_web_searches", None)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let text_model = pref_for_provider(PLUGIN_NAME, "text_model", None)
            .and_then(|v| serde_json::from_value::<OpenRouterTextModel>(v).ok())
            .unwrap_or_default();
        Self {
            api_key,
            model_choice_strategy,
            reasoning_strategy,
            data_collection,
            allow_web_searches,
            text_model,
        }
    }

    pub fn is_ready_for_use(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub fn validate(&self) -> Result<(), OpenRouterConfigError> {
        if !self.is_ready_for_use() {
            return Err(OpenRouterConfigError::MissingApiKey);
        }
        Ok(())
    }

    /// Python `settings` shape. `text_model` is omitted when its id is
    /// empty (Python `if self.text_model.model_id`).
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
            "data_collection".to_string(),
            json!(self.data_collection.as_str()),
        );
        // NB: Python's ConfigWidget doesn't emit `allow_web_searches`
        // into `settings` even though it reads it. Preserve that
        // (surprising) behavior byte-for-byte so round-tripping doesn't
        // stomp existing prefs. A follow-up should decide whether this
        // is intentional or a Python bug; for now, match.
        if !self.text_model.is_empty() {
            ans.insert(
                "text_model".to_string(),
                serde_json::to_value(&self.text_model).expect("OpenRouterTextModel serializes"),
            );
        }
        ans
    }

    pub fn commit(&self) -> Result<(), OpenRouterConfigError> {
        self.validate()?;
        set_prefs_for_provider(PLUGIN_NAME, self.settings());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(key: &str) -> OpenRouterConfig {
        OpenRouterConfig {
            api_key: key.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn defaults_match_python() {
        let c = OpenRouterConfig::default();
        assert_eq!(
            c.model_choice_strategy,
            OpenRouterModelChoiceStrategy::FreeOrPaid
        );
        assert_eq!(c.data_collection, DataCollection::Deny);
        assert_eq!(c.reasoning_strategy, ReasoningStrategy::Auto);
        assert!(
            !c.allow_web_searches,
            "OpenRouter default is False, not True like Google/OpenAI"
        );
    }

    #[test]
    fn strategy_parses_known_values() {
        assert_eq!(
            OpenRouterModelChoiceStrategy::parse("free-only"),
            OpenRouterModelChoiceStrategy::FreeOnly
        );
        assert_eq!(
            OpenRouterModelChoiceStrategy::parse("free-or-paid"),
            OpenRouterModelChoiceStrategy::FreeOrPaid
        );
        assert_eq!(
            OpenRouterModelChoiceStrategy::parse("native"),
            OpenRouterModelChoiceStrategy::Native
        );
    }

    #[test]
    fn strategy_unknown_falls_back_to_free_or_paid() {
        assert_eq!(
            OpenRouterModelChoiceStrategy::parse(""),
            OpenRouterModelChoiceStrategy::FreeOrPaid
        );
        assert_eq!(
            OpenRouterModelChoiceStrategy::parse("nonsense"),
            OpenRouterModelChoiceStrategy::FreeOrPaid
        );
    }

    #[test]
    fn data_collection_parses_allow_deny() {
        assert_eq!(DataCollection::parse("allow"), DataCollection::Allow);
        assert_eq!(DataCollection::parse("deny"), DataCollection::Deny);
        assert_eq!(DataCollection::parse(""), DataCollection::Deny);
    }

    #[test]
    fn validate_rejects_missing_key() {
        assert_eq!(cfg("").validate(), Err(OpenRouterConfigError::MissingApiKey));
    }

    #[test]
    fn settings_hex_round_trips_and_omits_empty_text_model() {
        let s = cfg("sk-or-abc").settings();
        let hex = s.get("api_key").unwrap().as_str().unwrap();
        assert_eq!(decode_secret(hex).unwrap(), "sk-or-abc");
        assert_eq!(
            s.get("model_choice_strategy").unwrap().as_str().unwrap(),
            "free-or-paid"
        );
        assert_eq!(s.get("data_collection").unwrap().as_str().unwrap(), "deny");
        assert!(s.get("text_model").is_none());
    }

    #[test]
    fn settings_includes_text_model_as_tuple_when_id_present() {
        let mut c = cfg("k");
        c.text_model = OpenRouterTextModel {
            id: "anthropic/claude-3.5-sonnet".into(),
            name: "Claude 3.5 Sonnet".into(),
        };
        let s = c.settings();
        let tm = s.get("text_model").unwrap();
        // Tuple shape, not dict: [id, name].
        let arr = tm.as_array().unwrap();
        assert_eq!(arr[0].as_str().unwrap(), "anthropic/claude-3.5-sonnet");
        assert_eq!(arr[1].as_str().unwrap(), "Claude 3.5 Sonnet");
    }

    #[test]
    fn text_model_deserializes_from_python_tuple_shape() {
        let v = json!(["openai/gpt-4o", "GPT-4o"]);
        let tm: OpenRouterTextModel = serde_json::from_value(v).unwrap();
        assert_eq!(tm.id, "openai/gpt-4o");
        assert_eq!(tm.name, "GPT-4o");
    }

    #[test]
    fn settings_omits_allow_web_searches_matching_python() {
        // Documenting the (odd) Python behavior — see comment in
        // settings(). If we ever change this, cross-validation will
        // catch prefs shape drift for existing users.
        let s = cfg("k").settings();
        assert!(s.get("allow_web_searches").is_none());
    }
}
