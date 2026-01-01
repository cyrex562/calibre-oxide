use crate::prefs::{decode_secret, pref_for_provider, AIProviderPlugin};
use crate::utils::{get_cached_resource, read_streaming_response};
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=500";
const PLUGIN_NAME: &str = "GoogleAI";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    pub above: f64,
    #[serde(default)]
    pub threshold: u64,
    #[serde(default)]
    pub below: f64,
}

impl Price {
    pub fn get_cost(&self, num_tokens: u64) -> f64 {
        if num_tokens <= self.threshold {
            self.below * num_tokens as f64
        } else {
            self.above * num_tokens as f64
        }
    }
}

// Simplified Pricing struct for now, can be expanded
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    pub input: Price,
    pub output: Price,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub id: String,
    pub slug: String,
    pub description: String,
    pub version: String,
    pub context_length: u64,
    pub output_token_limit: u64,
    #[serde(skip)]
    pub capabilities: AICapabilities,
    pub family: String,
    pub family_version: f64,
    pub name_parts: Vec<String>,
    pub thinking: bool,
    #[serde(default)]
    pub pricing: Option<Pricing>,
}

impl Model {
    pub fn from_json(x: &Value) -> Self {
        let mut caps = AICapabilities::TEXT_TO_TEXT;
        let mid = x["name"].as_str().unwrap_or("").to_string();
        
        let methods = x["supportedGenerationMethods"].as_array();
        if methods.map(|a| a.iter().any(|v| v == "embedContent")).unwrap_or(false) {
            caps |= AICapabilities::EMBEDDING;
        }

        let name_parts: Vec<String> = mid.split('/').last().unwrap_or("").split('-').map(|s| s.to_string()).collect();
        let mut family = String::new();
        let mut family_version = 0.0;
        
        if name_parts.len() > 1 {
            family = name_parts[0].clone();
            family_version = name_parts[1].parse().unwrap_or(0.0);
        }

        if family == "imagen" {
            caps |= AICapabilities::TEXT_TO_IMAGE;
        } else if family == "gemini" {
             if family_version >= 2.5 {
                 caps |= AICapabilities::TEXT_AND_IMAGE_TO_IMAGE;
             }
             if name_parts.contains(&"tts".to_string()) {
                 caps |= AICapabilities::TTS;
             }
        }

        Model {
            name: x["displayName"].as_str().unwrap_or("").to_string(),
            slug: mid.clone(),
            id: mid,
            description: x["description"].as_str().unwrap_or("").to_string(),
            version: x["version"].as_str().unwrap_or("").to_string(),
            context_length: x["inputTokenLimit"].as_u64().unwrap_or(0),
            output_token_limit: x["outputTokenLimit"].as_u64().unwrap_or(0),
            capabilities: caps,
            family,
            family_version,
            name_parts,
            thinking: x["thinking"].as_bool().unwrap_or(false),
            pricing: None, // Simplified: pricing hardcoded logic omitted for brevity in port
        }
    }
}

pub struct GoogleAI;

impl GoogleAI {
    pub fn api_key() -> Option<String> {
        pref_for_provider(PLUGIN_NAME, "api_key", None).and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    pub fn decoded_api_key() -> Result<String> {
        let key = Self::api_key().ok_or_else(|| anyhow!("API key required for Google AI"))?;
        decode_secret(&key).map_err(|e| anyhow!(e))
    }

    pub fn get_available_models() -> Result<HashMap<String, Model>> {
        let key = Self::decoded_api_key()?;
        let mut cache_path = dirs::cache_dir().unwrap_or(PathBuf::from("."));
        cache_path.push("calibre-oxide/ai");
        cache_path.push(format!("{}-models-v1.json", PLUGIN_NAME));

        let data = get_cached_resource(&cache_path, MODELS_URL, vec![("X-goog-api-key", &key)])?;
        let json: Value = serde_json::from_slice(&data)?;
        
        let mut ans = HashMap::new();
        if let Some(models) = json["models"].as_array() {
            for entry in models {
                let m = Model::from_json(entry);
                ans.insert(m.id.clone(), m);
            }
        }
        Ok(ans)
    }

    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let models = Self::get_available_models()?;
        let model = models.get(use_model).ok_or_else(|| anyhow!("Model {} not found", use_model))?;
        
        // Prepare content
        let mut contents = Vec::new();
        for m in messages {
            let role = if m.message_type == ChatMessageType::User { "user" } else { "model" };
             contents.push(json!({
                 "role": role,
                 "parts": [{"text": m.query}]
             }));
        }

        let data = json!({
             "contents": contents,
             "generationConfig": {
                 "thinkingConfig": { "includeThoughts": true }
             }
        });

        // Request logic
        let url = format!("{}/{}:streamGenerateContent?alt=sse", API_BASE_URL, model.slug);
        let key = Self::decoded_api_key()?;
        let client = reqwest::blocking::Client::new();
        
        let resp = client.post(&url)
            .header("X-goog-api-key", key)
            .header("Content-Type", "application/json")
            .json(&data)
            .send()?;

        if !resp.status().is_success() {
             return Err(anyhow!("Request failed: {}", resp.status()));
        }
        
        let model_id = model.id.clone();

        Ok(read_streaming_response(resp).filter_map(move |res_result| {
             match res_result {
                 Ok(d) => {
                     let mut responses = Vec::new();
                     if let Some(candidates) = d["candidates"].as_array() {
                         for c in candidates {
                             if let Some(content) = &c.get("content") {
                                 let mut text_parts = Vec::new();
                                 if let Some(parts) = content["parts"].as_array() {
                                     for part in parts {
                                         if let Some(text) = part["text"].as_str() {
                                             text_parts.push(text);
                                         }
                                     }
                                 }
                                 responses.push(ChatResponse {
                                     content: text_parts.join(""),
                                     message_type: ChatMessageType::Assistant,
                                     model: model_id.clone(),
                                     plugin_name: PLUGIN_NAME.to_string(),
                                     ..Default::default()
                                 });
                             }
                             // Citations logic simplified vs python for initial port
                         }
                     }
                     if let Some(pf) = d.get("promptFeedback") {
                         if let Some(br) = pf.get("blockReason") {
                              responses.push(ChatResponse {
                                  exception: Some(format!("Prompt blocked: {}", br)),
                                  ..Default::default()
                              });
                         }
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

impl AIProviderPlugin for GoogleAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT | AICapabilities::TEXT_TO_IMAGE | AICapabilities::TEXT_AND_IMAGE_TO_IMAGE | AICapabilities::EMBEDDING | AICapabilities::TTS
    }
}
