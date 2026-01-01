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

pub fn pref_for_provider(name: &str, key: &str, defval: Option<Value>) -> Option<Value> {
    let prefs = PREFS.read().unwrap();
    prefs.providers.get(name)
        .and_then(|p| p.get(key).cloned())
        .or(defval)
}

pub fn set_prefs_for_provider(name: &str, pref_map: HashMap<String, Value>) {
    let mut prefs = PREFS.write().unwrap();
    prefs.providers.insert(name.to_string(), pref_map);
    // In real impl, save to disk here
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
    let q = prefs.purpose_map.get(&purpose.purpose()).map(|s| s.as_str()).unwrap_or("");
    
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
