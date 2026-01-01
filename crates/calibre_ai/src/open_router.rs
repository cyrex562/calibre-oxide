use crate::prefs::{decode_secret, pref_for_provider, AIProviderPlugin};
use crate::utils::{get_cached_resource, read_streaming_response};
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const PLUGIN_NAME: &str = "OpenRouter";
const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(default)]
    pub input_token: f64,
    #[serde(default)]
    pub output_token: f64,
    #[serde(default)]
    pub request: f64,
    #[serde(default)]
    pub image: f64,
    #[serde(default)]
    pub web_search: f64,
    #[serde(default)]
    pub internal_reasoning: f64,
}

impl Pricing {
    pub fn from_json(x: &Value) -> Self {
        Pricing {
            input_token: x["prompt"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            output_token: x["completion"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            request: x["request"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            image: x["image"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            web_search: x["web_search"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            internal_reasoning: x["internal_reasoning"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
        }
    }
    
    pub fn is_free(&self) -> bool {
        self.input_token == 0.0 && self.output_token == 0.0 && self.request == 0.0 && self.image == 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub id: String,
    pub description: String,
    pub context_length: u64,
    pub created: u64,
    #[serde(default)]
    pub pricing: Option<Pricing>,
    #[serde(skip)]
    pub capabilities: AICapabilities,
}

impl Model {
    pub fn from_json(x: &Value) -> Self {
        let arch = &x["architecture"];
        let mut caps = AICapabilities::NONE;
        
        // Check modalities in 'input_modalities' and 'output_modalities'
        // Simplified check
        if let Some(out) = arch["output_modalities"].as_array() {
            if out.iter().any(|v| v == "text") { caps |= AICapabilities::TEXT_TO_TEXT; }
            if out.iter().any(|v| v == "image") { caps |= AICapabilities::TEXT_TO_IMAGE; }
        }

        Model {
            name: x["name"].as_str().unwrap_or("").to_string(),
            id: x["id"].as_str().unwrap_or("").to_string(),
            description: x["description"].as_str().unwrap_or("").to_string(),
            context_length: x["context_length"].as_u64().unwrap_or(0),
            created: x["created"].as_u64().unwrap_or(0),
            pricing: Some(Pricing::from_json(&x["pricing"])),
            capabilities: caps,
        }
    }
    
    pub fn creator(&self) -> String {
        self.name.split(':').next().unwrap_or("").to_lowercase()
    }
}

pub struct OpenRouterAI;

impl OpenRouterAI {
    pub fn api_key() -> Option<String> {
        pref_for_provider(PLUGIN_NAME, "api_key", None).and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    pub fn decoded_api_key() -> Result<String> {
        let key = Self::api_key().ok_or_else(|| anyhow!("API key required for OpenRouter"))?;
        decode_secret(&key).map_err(|e| anyhow!(e))
    }

    pub fn get_available_models() -> Result<HashMap<String, Model>> {
        let mut cache_path = dirs::cache_dir().unwrap_or(PathBuf::from("."));
        cache_path.push("calibre-oxide/ai");
        cache_path.push(format!("{}-models-v1.json", PLUGIN_NAME));

        let data = get_cached_resource(&cache_path, MODELS_URL, vec![])?; 
        let json: Value = serde_json::from_slice(&data)?;
        
        let mut ans = HashMap::new();
        if let Some(data_arr) = json["data"].as_array() {
            for entry in data_arr {
                let m = Model::from_json(entry);
                ans.insert(m.id.clone(), m);
            }
        }
        Ok(ans)
    }
    
    // Simplification: We omit the complex free/paid filtering logic for now and assume user picks model/auto
    // In full port, that logic would go here.

    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let _models = Self::get_available_models()?;
        
        let model_id = if !use_model.is_empty() {
             use_model.to_string()
        } else {
             pref_for_provider(PLUGIN_NAME, "text_model", None)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or("openrouter/auto".to_string())
        };

        // If auto/fallback logic needed, it goes here.
        
        let key = Self::decoded_api_key()?;
        
        let msgs: Vec<Value> = messages.iter().map(|m| {
            json!({
                "role": m.message_type.to_string(), 
                "content": m.query
            })
        }).collect();

        let data = json!({
            "model": model_id,
            "messages": msgs,
            "stream": true,
            "provider": {"data_collection": "deny"}, // Default deny
        });

        let client = reqwest::blocking::Client::new();
        let resp = client.post(CHAT_URL)
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://calibre-ebook.com")
            .header("X-Title", "calibre")
            .json(&data)
            .send()?;

        if !resp.status().is_success() {
            return Err(anyhow!("Request failed: {}", resp.status()));
        }

        Ok(read_streaming_response(resp).filter_map(move |res_result| {
             match res_result {
                 Ok(d) => {
                     let mut responses = Vec::new();
                     if let Some(choices) = d["choices"].as_array() {
                         for choice in choices {
                             let delta = &choice["delta"];
                             if let Some(content) = delta["content"].as_str() {
                                 responses.push(ChatResponse {
                                     content: content.to_string(),
                                     message_type: ChatMessageType::Assistant,
                                     plugin_name: PLUGIN_NAME.to_string(),
                                     model: model_id.clone(), // Best effort
                                     ..Default::default()
                                 });
                             }
                             // Handling reasoning etc. omitted for brevity
                         }
                     }
                     // usage
                     if let Some(_usage) = d.get("usage") {
                         responses.push(ChatResponse {
                             has_metadata: true,
                             plugin_name: PLUGIN_NAME.to_string(),
                             ..Default::default()
                         });
                     }
                     Some(responses)
                 },
                 Err(e) => Some(vec![ChatResponse {
                     exception: Some(e.to_string()),
                     ..Default::default()
                 }])
             }
        }).flatten())
    }
}

impl AIProviderPlugin for OpenRouterAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT | AICapabilities::TEXT_TO_IMAGE
    }
}
