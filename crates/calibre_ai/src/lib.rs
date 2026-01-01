use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMessageType {
    System,
    User,
    Assistant,
    Tool,
    Developer,
}

impl fmt::Display for ChatMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ChatMessageType::System => "system",
            ChatMessageType::User => "user",
            ChatMessageType::Assistant => "assistant",
            ChatMessageType::Tool => "tool",
            ChatMessageType::Developer => "developer",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub query: String,
    #[serde(rename = "type")]
    pub message_type: ChatMessageType,
    pub extra_data: Option<serde_json::Value>,
    #[serde(default)]
    pub reasoning_details: Vec<serde_json::Value>,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub response_id: String,
}

impl ChatMessage {
    pub fn new(query: impl Into<String>, message_type: ChatMessageType) -> Self {
        Self {
            query: query.into(),
            message_type,
            extra_data: None,
            reasoning_details: Vec::new(),
            reasoning: String::new(),
            response_id: String::new(),
        }
    }

    pub fn from_assistant(&self) -> bool {
        self.message_type == ChatMessageType::Assistant
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLink {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub uri: String,
}

impl WebLink {
    pub fn is_valid(&self) -> bool {
        !self.title.is_empty() && !self.uri.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub links: Vec<usize>,
    pub start_offset: usize,
    pub end_offset: usize,
    #[serde(default)]
    pub text: String,
}

impl Default for ChatMessageType {
    fn default() -> Self {
        ChatMessageType::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub reasoning_details: Vec<serde_json::Value>,
    #[serde(rename = "type", default = "default_chat_message_type")]
    pub message_type: ChatMessageType,
    #[serde(default)]
    pub id: String,

    #[serde(skip)]
    pub exception: Option<String>,
    #[serde(default)]
    pub error_details: String,

    #[serde(default)]
    pub has_metadata: bool,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub plugin_name: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
    #[serde(default)]
    pub web_links: Vec<WebLink>,
}

impl Default for ChatResponse {
    fn default() -> Self {
        Self {
            content: String::new(),
            reasoning: String::new(),
            reasoning_details: Vec::new(),
            message_type: ChatMessageType::Assistant,
            id: String::new(),
            exception: None,
            error_details: String::new(),
            has_metadata: false,
            cost: 0.0,
            currency: String::new(),
            provider: String::new(),
            model: String::new(),
            plugin_name: String::new(),
            citations: Vec::new(),
            web_links: Vec::new(),
        }
    }
}

fn default_chat_message_type() -> ChatMessageType {
    ChatMessageType::Assistant
}

#[derive(Debug, Error)]
#[error("No free models available")]
pub struct NoFreeModels;

#[derive(Debug, Error)]
#[error("No API key provided")]
pub struct NoAPIKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockReason {
    Unknown,
    Safety,
    Blocklist,
    ProhibitedContent,
    UnsafeImageGenerated,
}

impl PromptBlockReason {
    pub fn for_human(&self) -> &'static str {
        match self {
            PromptBlockReason::Safety => "Prompt would cause dangerous content to be generated",
            PromptBlockReason::Blocklist => "Prompt contains terms from a blocklist",
            PromptBlockReason::ProhibitedContent => "Prompt would cause prohibited content to be generated",
            PromptBlockReason::UnsafeImageGenerated => "Prompt would cause unsafe image content to be generated",
            PromptBlockReason::Unknown => "Prompt was blocked for an unknown reason",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultBlockReason {
    Unknown,
    MaxTokens,
    Safety,
    Recitation,
    UnsupportedLanguage,
    Blocklist,
    ProhibitedContent,
    PersonallyIdentifiableInfo,
    MalformedFunctionCall,
    UnsafeImageGenerated,
    UnexpectedToolCall,
    TooManyToolCalls,
}

impl ResultBlockReason {
    pub fn for_human(&self) -> &'static str {
        match self {
            ResultBlockReason::MaxTokens => "Result would contain too many tokens",
            ResultBlockReason::Safety => "Result would contain dangerous content",
            ResultBlockReason::Recitation => "Result would contain copyrighted content",
            ResultBlockReason::UnsupportedLanguage => "Result would contain an unsupported language",
            ResultBlockReason::PersonallyIdentifiableInfo => "Result would contain personally identifiable information",
            ResultBlockReason::Blocklist => "Result contains terms from a blocklist",
            ResultBlockReason::ProhibitedContent => "Result would contain prohibited content",
            ResultBlockReason::UnsafeImageGenerated => "Result would contain unsafe image content",
            ResultBlockReason::MalformedFunctionCall => "Result would contain a malformed function call/tool invocation",
            ResultBlockReason::UnexpectedToolCall => "Model tried to use a tool with no tools configured",
            ResultBlockReason::TooManyToolCalls => "Model tried to use too many tools",
            ResultBlockReason::Unknown => "Result was blocked for an unknown reason",
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct PromptBlocked {
    pub reason: PromptBlockReason,
    pub message: String,
}

impl PromptBlocked {
    pub fn new(reason: PromptBlockReason, custom_message: Option<String>) -> Self {
        let message = custom_message.unwrap_or_else(|| reason.for_human().to_string());
        Self { reason, message }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ResultBlocked {
    pub reason: ResultBlockReason,
    pub message: String,
}

impl ResultBlocked {
    pub fn new(reason: ResultBlockReason, custom_message: Option<String>) -> Self {
        let message = custom_message.unwrap_or_else(|| reason.for_human().to_string());
        Self { reason, message }
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct AICapabilities: u32 {
        const NONE = 1 << 0;
        const TEXT_TO_TEXT = 1 << 1;
        const TEXT_TO_IMAGE = 1 << 2;
        const TEXT_AND_IMAGE_TO_IMAGE = 1 << 3;
        const TTS = 1 << 4;
        const EMBEDDING = 1 << 5;
    }
}

impl AICapabilities {
    pub fn supports_text_to_text(&self) -> bool {
        self.contains(AICapabilities::TEXT_TO_TEXT)
    }

    pub fn supports_text_to_image(&self) -> bool {
        self.contains(AICapabilities::TEXT_TO_IMAGE)
    }

    pub fn purpose(&self) -> String {
        // Simple implementation, iterate and join names
        let mut names = Vec::new();
        if self.contains(AICapabilities::NONE) { names.push("NONE"); }
        if self.contains(AICapabilities::TEXT_TO_TEXT) { names.push("TEXT_TO_TEXT"); }
        if self.contains(AICapabilities::TEXT_TO_IMAGE) { names.push("TEXT_TO_IMAGE"); }
        if self.contains(AICapabilities::TEXT_AND_IMAGE_TO_IMAGE) { names.push("TEXT_AND_IMAGE_TO_IMAGE"); }
        if self.contains(AICapabilities::TTS) { names.push("TTS"); }
        if self.contains(AICapabilities::EMBEDDING) { names.push("EMBEDDING"); }
        
        format!("AICapabilities.{}", names.join("|"))
    }
}

// Module declarations
pub mod prefs;
pub mod utils;
pub mod github;
pub mod google;
pub mod lm_studio;
pub mod ollama;
pub mod open_router;
pub mod openai;
