use crate::prefs::{pref_for_provider, AIProviderPlugin};
use crate::utils::{download_data, read_streaming_response};
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use url::Url;

const PLUGIN_NAME: &str = "LMStudio";
const DEFAULT_URL: &str = "http://localhost:1234";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub owner: String,
}

impl Model {
    pub fn from_json(x: &Value) -> Self {
        Model {
            id: x["id"].as_str().unwrap_or("").to_string(),
            owner: x["owned_by"].as_str().unwrap_or("local").to_string(),
        }
    }
}

pub struct LMStudioAI;

impl LMStudioAI {
    pub fn pref_api_url() -> String {
        pref_for_provider(PLUGIN_NAME, "api_url", None)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| DEFAULT_URL.to_string())
    }

    pub fn api_url(path: &str, use_api_url: Option<&str>) -> Result<String> {
        let base = use_api_url.unwrap_or(&Self::pref_api_url()).to_string();
        let mut url = Url::parse(&base)?;
        
        // LM Studio mounts under /v1
        if !url.path().ends_with("/v1") {
            let new_path = if url.path().ends_with('/') {
                format!("{}v1", url.path())
            } else {
                format!("{}/v1", url.path())
            };
            url.set_path(&new_path);
        }

        if !path.is_empty() {
             let current_path = url.path();
             let new_path = if current_path.ends_with('/') {
                 format!("{}{}", current_path, path)
             } else {
                 format!("{}/{}", current_path, path)
             };
             url.set_path(&new_path);
        }

        Ok(url.to_string())
    }

    pub fn get_available_models(use_api_url: Option<&str>) -> HashMap<String, Model> {
        let mut ans = HashMap::new();
        if let Ok(url) = Self::api_url("models", use_api_url) {
             if let Ok(data_bytes) = download_data(&url, vec![]) {
                 if let Ok(json) = serde_json::from_slice::<Value>(&data_bytes) {
                     if let Some(data) = json["data"].as_array() {
                         for m in data {
                             let model = Model::from_json(m);
                             ans.insert(model.id.clone(), model);
                         }
                     }
                 }
             }
        }
        ans
    }

    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let model_id = if !use_model.is_empty() {
            use_model.to_string()
        } else {
            pref_for_provider(PLUGIN_NAME, "text_model", None)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .ok_or_else(|| anyhow!("No model selected"))?
        };

        let temp = pref_for_provider(PLUGIN_NAME, "temperature", None)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);

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
            "temperature": temp
        });

        let url = Self::api_url("chat/completions", None)?;
        let client = reqwest::blocking::Client::new();
        let resp = client.post(&url)
            .header("Content-Type", "application/json")
            .json(&data)
            .send()?;

        Ok(read_streaming_response(resp).filter_map(move |res_result| {
             match res_result {
                 Ok(d) => {
                     let mut responses = Vec::new();
                     if let Some(choices) = d["choices"].as_array() {
                         for choice in choices {
                             if let Some(delta) = choice.get("delta") {
                                 if let Some(content) = delta["content"].as_str() {
                                     let role = delta["role"].as_str().unwrap_or("assistant");
                                     // Map role string to ChatMessageType best effort
                                     let msg_type = match role {
                                         "user" => ChatMessageType::User,
                                         "system" => ChatMessageType::System,
                                         _ => ChatMessageType::Assistant,
                                     };
                                     
                                     responses.push(ChatResponse {
                                         content: content.to_string(),
                                         message_type: msg_type,
                                         plugin_name: PLUGIN_NAME.to_string(),
                                         ..Default::default()
                                     });
                                 }
                             }
                         }
                     }
                     if let Some(_usage) = d.get("usage") {
                          // TODO: usage parsing
                          responses.push(ChatResponse {
                              has_metadata: true,
                              provider: "LM Studio".to_string(),
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

impl AIProviderPlugin for LMStudioAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT
    }
}
