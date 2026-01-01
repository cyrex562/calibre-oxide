use crate::prefs::{pref_for_provider, AIProviderPlugin};
use crate::utils::download_data;
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use url::Url;

const PLUGIN_NAME: &str = "OllamaAI";
const DEFAULT_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub id: String,
    pub family: String,
    pub families: Vec<String>,
    pub modified_at: String,
    #[serde(skip)]
    pub can_think: bool,
}

impl Model {
    pub fn from_json(x: &Value, details: &Value) -> Self {
        let mut families = Vec::new();
        if let Some(fams) = details["families"].as_array() {
            families = fams.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect();
        }
        
        let can_think = false;
        // Check for 'thinking' capability. In original logic: 'thinking' in details['capabilities']
        // Implementation detail: checking `details` json
        // For now, simplify or omit specific capability check unless we inspect deep structure
        
        Model {
            name: x["name"].as_str().unwrap_or("").to_string(),
            id: x["model"].as_str().unwrap_or("").to_string(),
            family: details["family"].as_str().unwrap_or("").to_string(),
            families,
            modified_at: x["modified_at"].as_str().unwrap_or("").to_string(),
            can_think,
        }
    }
}

pub struct OllamaAI;

impl OllamaAI {
    pub fn pref_api_url() -> String {
        pref_for_provider(PLUGIN_NAME, "api_url", None)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| DEFAULT_URL.to_string())
    }

    pub fn api_url(path: &str, use_api_url: Option<&str>) -> Result<String> {
        let base = use_api_url.unwrap_or(&Self::pref_api_url()).to_string();
        let mut url = Url::parse(&base)?;
        
        // Ollama API is relative to base, mostly
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
        if let Ok(url) = Self::api_url("api/tags", use_api_url) {
             if let Ok(data_bytes) = download_data(&url, vec![]) {
                 if let Ok(json) = serde_json::from_slice::<Value>(&data_bytes) {
                     if let Some(models) = json["models"].as_array() {
                         let show_url = Self::api_url("api/show", use_api_url).unwrap_or_default();
                         for m in models {
                             let model_name = m["model"].as_str().unwrap_or("");
                             // Fetch details
                             let client = reqwest::blocking::Client::new();
                             let details = if !show_url.is_empty() {
                                 let res = client.post(&show_url)
                                     .json(&json!({"model": model_name}))
                                     .send();
                                 match res {
                                     Ok(r) => r.json().unwrap_or(json!({})),
                                     Err(_) => json!({})
                                 }
                             } else {
                                 json!({})
                             };

                             let model = Model::from_json(m, &details);
                             ans.insert(model.id.clone(), model);
                         }
                     }
                 }
             }
        }
        ans
    }
    
    // Ollama uses NDJSON (NewLine Delimited JSON) not SSE "data: ..." format
    // We need a custom reader or adapt the utils one.
    // For now I'll implement a local reader here.
    pub fn read_ndjson_response(reader: impl Read) -> impl Iterator<Item = Result<Value>> {
        let reader = BufReader::new(reader);
        reader.lines().map(|line| {
            let l = line?;
            Ok(serde_json::from_str(&l)?)
        })
    }

    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let model_id = if !use_model.is_empty() {
            use_model.to_string()
        } else {
            pref_for_provider(PLUGIN_NAME, "text_model", None)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .ok_or_else(|| anyhow!("No model selected"))?
        };

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

        let url = Self::api_url("api/chat", None)?;
        let client = reqwest::blocking::Client::new();
        let resp = client.post(&url)
            .header("Content-Type", "application/json")
            .json(&data)
            .send()?;

        if !resp.status().is_success() {
            return Err(anyhow!("Request failed: {}", resp.status()));
        }

        Ok(Self::read_ndjson_response(resp).filter_map(move |res_result| {
             match res_result {
                 Ok(d) => {
                     let mut responses = Vec::new();
                     
                     let content = d["message"]["content"].as_str().unwrap_or("").to_string();
                     // Ollama might send "done": true
                     let done = d["done"].as_bool().unwrap_or(false);
                     
                     if !content.is_empty() || done {
                         responses.push(ChatResponse {
                             content,
                             message_type: ChatMessageType::Assistant,
                             model: model_id.clone(),
                             plugin_name: PLUGIN_NAME.to_string(),
                             has_metadata: done,
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

impl AIProviderPlugin for OllamaAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT
    }
}
