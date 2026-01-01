use crate::{ChatMessage, ChatMessageType, ChatResponse};
use anyhow::{anyhow, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};

// SSE Parser
pub fn read_streaming_response(reader: impl Read) -> impl Iterator<Item = Result<Value>> {
    let reader = BufReader::new(reader);
    let mut buffer = String::new();

    reader.lines().filter_map(move |line| {
        let line = match line {
            Ok(l) => l,
            Err(e) => return Some(Err(anyhow!(e))),
        };

        if line.trim().is_empty() {
            if !buffer.is_empty() {
                let res = parse_sse_buffer(&buffer);
                buffer.clear();
                return res; // Yield the parsed JSON
            }
            None
        } else {
            buffer.push_str(&line);
            buffer.push('\n');
            None
        }
    })
}

fn parse_sse_buffer(buffer: &str) -> Option<Result<Value>> {
    if !buffer.starts_with("data: ") {
        return None;
    }
    let content = buffer[6..].trim(); // remove "data: " and trailing newline
    if content == "[DONE]" {
        return None;
    }
    Some(serde_json::from_str(content).map_err(|e| anyhow!(e)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Unknown,
    Markdown,
}

lazy_static! {
    static ref MARKDOWN_PATTERNS: HashMap<&'static str, f64> = {
        let mut m = HashMap::new();
        m.insert(r"(?m)^#{1,6}\s+.+$", 0.15); // Headers
        m.insert(r"(?m)^\[\.+?\]: ", 0.15);   // References
        m.insert(r"\*\*.+?\*\*", 0.05);       // Bold
        m.insert(r"\*[^*\n]+\*", 0.05);       // Italic
        m.insert(r"(?m)^[\s]*[-*+][\s]+.+$", 0.1); // Unordered list
        m.insert(r"(?m)^[\s]*\d+\.[\s]+.+$", 0.1); // Ordered list
        m.insert(r"(?m)^[\s]*>[\s]*.+$", 0.1);     // Blockquote
        m.insert(r"\[.+?\]\(.+?\)", 0.15);         // Links
        m.insert(r"\|.+\|[\s]*\n\|[\s]*[-:]+[-|\s:]+[\s]*\n", 0.1); // Tables
        m
    };
    
    static ref MARKDOWN_REGEXES: HashMap<&'static str, Regex> = {
        MARKDOWN_PATTERNS.keys().map(|&pat| (pat, Regex::new(pat).unwrap())).collect()
    };
}

pub fn is_probably_markdown(text: &str, threshold: Option<f64>) -> bool {
    let threshold = threshold.unwrap_or(0.2);
    if text.is_empty() {
        return false;
    }
    let mut score = 0.0;
    for (pat, regex) in MARKDOWN_REGEXES.iter() {
        if regex.is_match(text) {
             if let Some(&pscore) = MARKDOWN_PATTERNS.get(pat) {
                 score += pscore;
                 if score >= threshold {
                     return true;
                 }
             }
        }
    }
    false
}

pub struct StreamedResponseAccumulator {
    pub all_reasoning: String,
    pub all_content: String,
    pub all_reasoning_details: Vec<Value>,
    pub metadata: ChatResponse,
    pub messages: Vec<ChatMessage>,
    pub response_id: String,
}

impl StreamedResponseAccumulator {
    pub fn new() -> Self {
        Self {
            all_reasoning: String::new(),
            all_content: String::new(),
            all_reasoning_details: Vec::new(),
            metadata: ChatResponse::default(),
            messages: Vec::new(),
            response_id: String::new(),
        }
    }

    pub fn content_type(&self) -> ContentType {
        if !self.metadata.citations.is_empty() {
            ContentType::Markdown
        } else {
            ContentType::Unknown
        }
    }

    pub fn accumulate(&mut self, m: ChatResponse) {
        if m.has_metadata {
            self.metadata = m.clone();
        }
        if !m.reasoning.is_empty() {
            self.all_reasoning.push_str(&m.reasoning);
            self.all_reasoning_details.extend(m.reasoning_details.clone());
        }
        if !m.content.is_empty() {
            self.all_content.push_str(&m.content);
        }
        if !m.id.is_empty() {
            self.response_id = m.id.clone();
        }
    }

    pub fn finalize(&mut self) {
        let query = add_citations(&self.all_content, &self.metadata);
        let mut msg = ChatMessage::new(query, ChatMessageType::Assistant);
        msg.reasoning = self.all_reasoning.clone();
        msg.reasoning_details = self.all_reasoning_details.clone();
        msg.response_id = self.response_id.clone();
        self.messages.push(msg);
    }
}

pub fn add_citations(text: &str, metadata: &ChatResponse) -> String {
    let citations = &metadata.citations;
    let web_links = &metadata.web_links;
    
    if citations.is_empty() || web_links.is_empty() {
        return text.to_string();
    }

    let escaped_titles: Vec<String> = web_links.iter()
        .map(|wl| wl.title.replace("\"", "\\\""))
        .collect();

    // Sort citations in reverse order of offset to insert safely
    let mut sorted_citations = citations.clone();
    sorted_citations.sort_by(|a, b| b.end_offset.cmp(&a.end_offset));

    let mut result: Vec<char> = text.chars().collect();
    
    for citation in sorted_citations {
        if citation.links.is_empty() { continue; }

        if citation.links.len() == 1 {
            let link_idx = citation.links[0];
            if link_idx >= web_links.len() { continue; }
            let wl = &web_links[link_idx];
            let title = &escaped_titles[link_idx];
            
            // Logic: 
            // text[:start] + f'[{text[start:end]}]({wl.uri} "{title}")' + text[end:]
            
            // Rust char indices
            // We need to keep track of the text as chars.
            // result is currently the full text.
            
            let start = citation.start_offset;
            let end = citation.end_offset;
            
            if start > end || end > result.len() { continue; }
            
            let anchor_text: String = result[start..end].iter().collect();
            let replacement = format!("[{}]({} \"{}\")", anchor_text, wl.uri, title);
            
            // Replace range
            let right_part: Vec<char> = result[end..].to_vec();
            result.truncate(start);
            result.extend(replacement.chars());
            result.extend(right_part);

        } else {
            let mut citation_links = Vec::new();
            for (i, &link_num) in citation.links.iter().enumerate() {
                 if link_num >= web_links.len() { continue; }
                 let wl = &web_links[link_num];
                 let title = &escaped_titles[link_num];
                 citation_links.push(format!("[{}]({} \"{}\")", i+1, wl.uri, title));
            }
            
            let end = citation.end_offset;
            if end > result.len() { continue; }
            
            let insertion = format!("<sup>{}</sup>", citation_links.join(", "));
            
            // Insert at end
            let right_part: Vec<char> = result[end..].to_vec();
            result.truncate(end);
            result.extend(insertion.chars());
            result.extend(right_part);
        }
    }

    result.into_iter().collect()
}

pub fn response_to_html(text: &str) -> String {
    // Simple mock for now
    html_escape::encode_text(text).replace('\n', "<br>")
}

pub fn download_data(url: &str, headers: Vec<(&str, &str)>) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::new();
    let mut req = client.get(url);
    for (key, value) in headers {
        req = req.header(key, value);
    }
    let resp = req.send()?;
    if !resp.status().is_success() {
        return Err(anyhow!("Request failed with status: {}", resp.status()));
    }
    Ok(resp.bytes()?.to_vec())
}

pub fn atomic_write(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(std::path::Path::new(".")))?;
    tmp.write_all(data)?;
    tmp.persist(path)?;
    Ok(())
}

pub fn get_cached_resource(path: &std::path::Path, url: &str, headers: Vec<(&str, &str)>) -> Result<Vec<u8>> {
    if path.exists() {
        // Simple check: if exists, return it. Background update omitted for brevity.
        // In a real impl, we might check mtime or fire a background thread.
        match std::fs::read(path) {
            Ok(data) => return Ok(data),
            Err(_) => {}, // fallback to download
        }
    }
    
    let data = download_data(url, headers)?;
    atomic_write(path, &data)?;
    Ok(data)
}
