use crate::Plugin;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOption {
    pub name: String,
    pub help: String,
    pub long_switch: Option<String>,
    pub short_switch: Option<String>,
    pub choices: Option<Vec<String>>,
}

impl ConversionOption {
    pub fn new(name: &str, help: &str) -> Self {
        ConversionOption {
            name: name.to_string(),
            help: help.to_string(),
            long_switch: Some(name.replace('_', "-")),
            short_switch: None,
            choices: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OptionRecommendationLevel {
    Low = 1,
    Med = 2,
    High = 3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionRecommendation {
    pub option: ConversionOption,
    pub recommended_value: Option<String>, // Simplification: storage as string
    pub level: OptionRecommendationLevel,
}

pub trait InputFormatPlugin: Plugin {
    fn file_types(&self) -> HashSet<String> { HashSet::new() }
    fn is_image_collection(&self) -> bool { false }
    fn core_usage(&self) -> i32 { 1 }
    fn output_encoding(&self) -> Option<String> { Some("utf-8".to_string()) }
    
    // In Python this takes many args. In Rust we'd likely pass a context struct.
    // For now, signature matches the intent: convert stream to OEB (Open eBook format).
    // Dealing with "OEBBook" type which is complex. We Stub the result or use PathBuf (to OPF file).
    fn convert(&self, _stream_path: &std::path::Path, _options: &HashMap<String, String>) -> anyhow::Result<std::path::PathBuf> {
        Err(anyhow::anyhow!("Not implemented"))
    }
}

pub trait OutputFormatPlugin: Plugin {
    fn file_type(&self) -> &str; // e.g., "epub"
    
    fn convert(&self, _oeb_book_path: &std::path::Path, _output_path: &std::path::Path, _options: &HashMap<String, String>) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Not implemented"))
    }
}
