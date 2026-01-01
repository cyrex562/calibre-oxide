use crate::prefs::{decode_secret, pref_for_provider, AIProviderPlugin};
use crate::utils::{get_cached_resource, read_streaming_response};
use crate::{AICapabilities, ChatMessage, ChatMessageType, ChatResponse, ResultBlocked};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// Constants
const MODELS_URL: &str = "https://models.github.ai/catalog/models";
const CHAT_URL: &str = "https://models.github.ai/inference/chat/completions";
const API_VERSION: &str = "2022-11-28";
const PLUGIN_NAME: &str = "GitHubABI";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub id: String,
    pub url: String,
    pub description: String,
    pub version: String,
    pub context_length: u64,
    pub output_token_limit: u64,
    #[serde(skip)]
    pub capabilities: AICapabilities,
    pub thinking: bool,
    pub publisher: String,
}

impl Model {
    pub fn from_json(x: &Value) -> Self {
        let id = x["id"].as_str().unwrap_or("").to_string();
        let mut caps = AICapabilities::NONE;
        
        let caps_json = &x["capabilities"];
        let output_modalities = &x["supported_output_modalities"];
        let input_modalities = &x["supported_input_modalities"];

        if caps_json["embedding"].as_bool().unwrap_or(false) 
           || output_modalities.as_array().map(|a| a.iter().any(|v| v == "embeddings")).unwrap_or(false) {
            caps |= AICapabilities::EMBEDDING;
        } else {
            let input_has_text = input_modalities.as_array().map(|a| !a.is_empty()).unwrap_or(false); // Simplified check
             if input_has_text {
                 caps |= AICapabilities::TEXT_TO_TEXT;
             }
        }

        let thinking = caps_json["reasoning"].as_bool().unwrap_or(false);

        Model {
            name: x["name"].as_str().unwrap_or("").to_string(),
            id,
            description: x["summary"].as_str().unwrap_or("").to_string(),
            version: x["version"].as_str().unwrap_or("").to_string(),
            context_length: x["limits"]["max_input_tokens"].as_u64().unwrap_or(0),
            output_token_limit: x["limits"]["max_output_tokens"].as_u64().unwrap_or(0),
            publisher: x["publisher"].as_str().unwrap_or("").to_string(),
            capabilities: caps,
            url: x["html_url"].as_str().unwrap_or("").to_string(),
            thinking,
        }
    }
}

pub struct GitHubAI {
    // We can store state here if needed
}

impl GitHubAI {
    pub fn new() -> Self {
        GitHubAI {}
    }

    pub fn api_key() -> Option<String> {
        pref_for_provider(PLUGIN_NAME, "api_key", None).and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    pub fn decoded_api_key() -> Result<String> {
        let key = Self::api_key().ok_or_else(|| anyhow!("Personal access token required for GitHub AI"))?;
        decode_secret(&key).map_err(|e| anyhow!(e))
    }

    pub fn headers() -> Result<Vec<(&'static str, String)>> {
        let key = Self::decoded_api_key()?;
        Ok(vec![
            ("Authorization", format!("Bearer {}", key)),
            ("X-GitHub-Api-Version", API_VERSION.to_string()),
            ("Accept", "application/vnd.github+json".to_string()),
            ("Content-Type", "application/json".to_string()),
        ])
    }

    pub fn get_available_models() -> Result<HashMap<String, Model>> {
        // Need cache dir. For now use a temp dir or look up XDG cache.
        // Assuming a `cache_dir()` function exists or we mock it.
        // I'll assume standard cache structure: ~/.cache/calibre-oxide/ai/...
        let mut cache_path = dirs::cache_dir().unwrap_or(PathBuf::from("."));
        cache_path.push("calibre-oxide/ai");
        cache_path.push(format!("{}-models-v1.json", PLUGIN_NAME));

        let data = get_cached_resource(&cache_path, MODELS_URL, vec![])?; // Models URL is public?
        let entries: Vec<Value> = serde_json::from_slice(&data)?;
        
        let mut ans = HashMap::new();
        for entry in entries {
            let m = Model::from_json(&entry);
            ans.insert(m.id.clone(), m);
        }
        Ok(ans)
    }

    pub fn chat_request(data: &mut Value, _model: &Model) -> Result<reqwest::blocking::Response> {
        data["stream"] = json!(true);
        data["stream_options"] = json!({"include_usage": true});
        
        // Need to convert our Vec<(&str, String)> to reqwest headers
        let headers_list = Self::headers()?;
        let client = reqwest::blocking::Client::new();
        let mut req = client.post(CHAT_URL);
        
        for (k, v) in headers_list {
            req = req.header(k, v);
        }
        
        Ok(req.json(data).send()?)
    }
    
    pub fn text_chat(messages: &[ChatMessage], use_model: &str) -> Result<impl Iterator<Item = ChatResponse>> {
        let models = Self::get_available_models()?;
        let model = models.get(use_model).ok_or_else(|| anyhow!("Model {} not found", use_model))?;
        
        let msgs: Vec<Value> = messages.iter().map(|m| {
            json!({
                "role": m.message_type.to_string(), // Ensure fmt::Display matches API expected roles
                "content": m.query
            })
        }).collect();

        let mut data = json!({
            "model": model.id,
            "messages": msgs
        });

        let resp = Self::chat_request(&mut data, model)?;
        
        let model_id = model.id.clone();
        
        // This iterator logic is tricky because we need to yield values from the streaming response.
        // `read_streaming_response` takes a Reader. `resp` is a Reader.
        // We'll map the output.
        
        Ok(read_streaming_response(resp).filter_map(move |res_result| {
             match res_result {
                 Ok(d) => {
                     // Parse d (which is one SSE JSON object) into ChatResponse(s)
                     // Logic from as_chat_responses
                     let mut responses = Vec::new();
                     if let Some(choices) = d["choices"].as_array() {
                         for choice in choices {
                             if let Some(content) = choice["delta"]["content"].as_str() {
                                 responses.push(ChatResponse {
                                     content: content.to_string(),
                                     message_type: ChatMessageType::Assistant, // partial
                                     model: model_id.clone(),
                                     plugin_name: PLUGIN_NAME.to_string(),
                                     ..Default::default()
                                 });
                             }
                             if let Some(reason) = choice["finish_reason"].as_str() {
                                 if reason != "stop" && !reason.is_empty() {
                                      // Blocked
                                      let blocked = ResultBlocked::new(crate::ResultBlockReason::Unknown, Some(format!("Blocked: {}", reason)));
                                      responses.push(ChatResponse {
                                          exception: Some(blocked.message), // simplified path
                                          ..Default::default()
                                      });
                                 }
                             }
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

impl AIProviderPlugin for GitHubAI {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn capabilities(&self) -> AICapabilities {
        AICapabilities::TEXT_TO_TEXT | AICapabilities::EMBEDDING
    }
}
