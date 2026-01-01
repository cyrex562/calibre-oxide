use crate::prefs::{decode_secret, pref_for_provider, AIProviderPlugin};
use crate::utils::{get_cached_resource, read_streaming_response};
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const PLUGIN_NAME: &str = "OpenAI";
const MODELS_URL: &str = "https://api.openai.com/v1/models";
const CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub created: u64,
    pub owned_by: String,
}

impl Model {
    pub fn from_json(x: &Value) -> Self {
        Model {
            id: x["id"].as_str().unwrap_or("").to_string(),
            created: x["created"].as_u64().unwrap_or(0),
            owned_by: x["owned_by"].as_str().unwrap_or("").to_string(),
        }
    }
}

pub struct OpenAI;

impl OpenAI {
    pub fn api_key() -> Option<String> {
        pref_for_provider(PLUGIN_NAME, "api_key", None).and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    pub fn decoded_api_key() -> Result<String> {
        let key = Self::api_key().ok_or_else(|| anyhow!("API key required for OpenAI"))?;
        decode_secret(&key).map_err(|e| anyhow!(e))
    }

    pub fn get_available_models() -> Result<HashMap<String, Model>> {
        let key = Self::decoded_api_key()?;
        let mut cache_path = dirs::cache_dir().unwrap_or(PathBuf::from("."));
        cache_path.push("calibre-oxide/ai");
        cache_path.push(format!("{}-models-v1.json", PLUGIN_NAME));

        let data = get_cached_resource(&cache_path, MODELS_URL, vec![("Authorization", &format!("Bearer {}", key))])?;
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

    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let model_id = if !use_model.is_empty() {
            use_model.to_string()
        } else {
            pref_for_provider(PLUGIN_NAME, "text_model", None)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or("gpt-3.5-turbo".to_string())
        };
        
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
        });

        let client = reqwest::blocking::Client::new();
        let resp = client.post(CHAT_URL)
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
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
                             if let Some(delta) = choice.get("delta") {
                                 if let Some(content) = delta["content"].as_str() {
                                     responses.push(ChatResponse {
                                         content: content.to_string(),
                                         message_type: ChatMessageType::Assistant,
                                         plugin_name: PLUGIN_NAME.to_string(),
                                         model: model_id.clone(),
                                         ..Default::default()
                                     });
                                 }
                             }
                         }
                     }
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

impl AIProviderPlugin for OpenAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT | AICapabilities::TEXT_TO_IMAGE | AICapabilities::EMBEDDING | AICapabilities::TTS
    }
}
