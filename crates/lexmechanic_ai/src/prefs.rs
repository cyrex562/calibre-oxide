use crate::AICapabilities;
use lazy_static::lazy_static;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Stub for the actual AIProviderPlugin which might be in another crate
pub trait AIProviderPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> AICapabilities;
}

// Global registry for plugins (replacing available_ai_provider_plugins)
// In a real app this might be injected or loaded dynamically.
lazy_static! {
    static ref REGISTERED_PLUGINS: RwLock<Vec<Arc<dyn AIProviderPlugin>>> = RwLock::new(Vec::new());
}

pub fn register_plugin(plugin: Arc<dyn AIProviderPlugin>) {
    let mut plugins = REGISTERED_PLUGINS.write().unwrap();
    plugins.push(plugin);
}

pub fn available_ai_provider_plugins() -> Vec<Arc<dyn AIProviderPlugin>> {
    REGISTERED_PLUGINS.read().unwrap().clone()
}

// Mocking JSONConfig
#[derive(Debug, Clone)]
pub struct ArtificialIntelligenceConfig {
    pub providers: HashMap<String, HashMap<String, Value>>,
    pub purpose_map: HashMap<String, String>,
    pub llm_localized_results: String,
}

impl Default for ArtificialIntelligenceConfig {
    fn default() -> Self {
        Self {
            providers: HashMap::new(),
            purpose_map: HashMap::new(),
            llm_localized_results: "never".to_string(),
        }
    }
}

lazy_static! {
    static ref PREFS: RwLock<ArtificialIntelligenceConfig> = RwLock::new(ArtificialIntelligenceConfig::default());
}

/// Provider names that shipped under a typo before being corrected
/// (issue #107: `"GitHubABI"` -> `"GitHubAI"`), mapped old -> canonical.
/// Prefs on disk from before the fix are keyed under the old name;
/// [`canonical_provider_name`] lets every lookup/write site treat both
/// spellings as the same provider without a one-shot migration pass.
const LEGACY_PROVIDER_NAME_ALIASES: &[(&str, &str)] = &[("GitHubABI", "GitHubAI")];

fn canonical_provider_name(name: &str) -> &str {
    LEGACY_PROVIDER_NAME_ALIASES.iter().find(|(old, _)| *old == name).map(|(_, new)| *new).unwrap_or(name)
}

pub fn pref_for_provider(name: &str, key: &str, defval: Option<Value>) -> Option<Value> {
    let name = canonical_provider_name(name);
    let prefs = PREFS.read().unwrap();
    prefs.providers.get(name)
        .and_then(|p| p.get(key).cloned())
        .or(defval)
}

pub fn set_prefs_for_provider(name: &str, pref_map: HashMap<String, Value>) {
    let name = canonical_provider_name(name);
    let mut prefs = PREFS.write().unwrap();
    prefs.providers.insert(name.to_string(), pref_map);
    // In real impl, save to disk here
}

/// Write the currently-selected provider for a given purpose into the
/// `purpose_map`. Called from `ConfigureAI::commit`.
pub fn set_purpose_selection(purpose: &AICapabilities, provider_name: &str) {
    let provider_name = canonical_provider_name(provider_name);
    let mut prefs = PREFS.write().unwrap();
    prefs
        .purpose_map
        .insert(purpose.purpose(), provider_name.to_string());
}

pub fn plugins_for_purpose(purpose: AICapabilities) -> impl Iterator<Item = Arc<dyn AIProviderPlugin>> {
    let plugins = available_ai_provider_plugins();
    // Sort by name (primary_sort_key in python, here just string sort)
    let mut sorted_plugins = plugins;
    sorted_plugins.sort_by(|a, b| a.name().cmp(b.name())); // Simple sort

    sorted_plugins.into_iter().filter(move |p| p.capabilities().contains(purpose))
}

pub fn plugin_for_purpose(purpose: AICapabilities) -> Option<Arc<dyn AIProviderPlugin>> {
    let compatible_plugins: HashMap<String, Arc<dyn AIProviderPlugin>> = 
        plugins_for_purpose(purpose).map(|p| (p.name().to_string(), p)).collect();
    
    let prefs = PREFS.read().unwrap();
    let q = prefs.purpose_map.get(&purpose.purpose()).map(|s| canonical_provider_name(s)).unwrap_or("");
    
    if let Some(p) = compatible_plugins.get(q) {
        return Some(p.clone());
    }

    if !compatible_plugins.is_empty() {
        // Prefer Google for text to text
        if purpose == AICapabilities::TEXT_TO_TEXT {
            if let Some(p) = compatible_plugins.get("Google") {
                return Some(p.clone());
            }
        }
        // Return first one (values iteration order is arbitrary in HashMap, so we should rely on sorted list)
        // Re-iterating for determinism
        return plugins_for_purpose(purpose).next();
    }

    None
}

pub fn encode_secret(text: &str) -> String {
    hex::encode(text)
}

pub fn decode_secret(text: &str) -> Result<String, hex::FromHexError> {
    let bytes = hex::decode(text)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

// Need hex crate or implement it.
// "polyglot.binary.as_hex_unicode" does hex encoding of utf-8 bytes.
// I'll add `hex` dependency to Cargo.toml.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // PREFS is process-global; serialize tests that touch it.
    static PREFS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn canonical_provider_name_maps_the_known_typo() {
        assert_eq!(canonical_provider_name("GitHubABI"), "GitHubAI");
        assert_eq!(canonical_provider_name("GitHubAI"), "GitHubAI");
        assert_eq!(canonical_provider_name("Google"), "Google");
    }

    #[test]
    fn a_pref_written_under_the_typo_is_readable_under_the_canonical_name() {
        let _guard = PREFS_LOCK.lock().unwrap();
        set_prefs_for_provider("GitHubABI", HashMap::from([("api_key".to_string(), json!("secret"))]));
        assert_eq!(pref_for_provider("GitHubAI", "api_key", None), Some(json!("secret")));
        assert_eq!(pref_for_provider("GitHubABI", "api_key", None), Some(json!("secret")));
    }

    #[test]
    fn a_purpose_selection_written_under_the_typo_still_resolves() {
        let _guard = PREFS_LOCK.lock().unwrap();
        set_purpose_selection(&AICapabilities::TEXT_TO_TEXT, "GitHubABI");
        let prefs = PREFS.read().unwrap();
        assert_eq!(
            prefs.purpose_map.get(&AICapabilities::TEXT_TO_TEXT.purpose()).map(String::as_str),
            Some("GitHubAI"),
            "writes should always store the canonical name, even when given the old one"
        );
    }
}
