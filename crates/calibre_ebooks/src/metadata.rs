use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

lazy_static! {
    static ref AUTHOR_PAT: Regex = Regex::new(r"(?i),?\s+(and|with)\s+").unwrap();
    static ref TITLE_PFX_PAT: Regex = Regex::new(r"^(A|The|An)\s+").unwrap();
}

pub fn string_to_authors(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    let raw = raw.replace("&&", "\u{ffff}");
    let raw = AUTHOR_PAT.replace_all(&raw, "&");
    raw.split('&')
        .map(|a| a.trim().replace("\u{ffff}", "&"))
        .filter(|a| !a.is_empty())
        .collect()
}

pub fn authors_to_string(authors: &[String]) -> String {
    authors.iter()
        .map(|a| a.replace('&', "&&"))
        .collect::<Vec<_>>()
        .join(" & ")
}

pub fn author_to_author_sort(author: &str) -> String {
    let tokens: Vec<&str> = author.split_whitespace().collect();
    if tokens.len() < 2 {
        return author.to_string();
    }
    let last = tokens.last().unwrap();
    let rest = &tokens[..tokens.len()-1];
    format!("{}, {}", last, rest.join(" "))
}

pub fn title_sort(title: &str) -> String {
    let mut title = title.trim().to_string();
    if let Some(mat) = TITLE_PFX_PAT.find(&title) {
        let pfx = mat.as_str();
        let pfx_len = pfx.len();
        title = format!("{}, {}", &title[pfx_len..], pfx.trim());
    }
    title
}

// ISBN Handling
pub fn check_digit_isbn10(isbn: &str) -> String {
    let chars: Vec<u32> = isbn.chars().filter_map(|c| c.to_digit(10)).collect();
    if chars.len() < 9 { return "X".to_string(); }
    let sum: u32 = chars.iter().take(9).enumerate().map(|(i, &d)| (i as u32 + 1) * d).sum();
    let check = sum % 11;
    if check == 10 { "X".to_string() } else { check.to_string() }
}

pub fn check_digit_isbn13(isbn: &str) -> String {
    let chars: Vec<u32> = isbn.chars().filter_map(|c| c.to_digit(10)).collect();
    if chars.len() < 12 { return "0".to_string(); }
    let sum: u32 = chars.iter().take(12).enumerate().map(|(i, &d)| {
        if i % 2 == 0 { d } else { d * 3 }
    }).sum();
    let check = 10 - (sum % 10);
    if check == 10 { "0".to_string() } else { check.to_string() }
}

pub fn check_isbn(isbn: &str) -> Option<String> {
    if isbn.is_empty() { return None; }
    let clean: String = isbn.chars().filter(|c| c.is_digit(10) || *c == 'X' || *c == 'x').collect();
    let upper = clean.to_uppercase();
    match upper.len() {
        10 => {
            let last = upper.chars().last()?;
            let check = check_digit_isbn10(&upper);
             if check == last.to_string() { Some(upper) } else { None }
        },
        13 => {
            let last = upper.chars().last()?;
            let check = check_digit_isbn13(&upper);
             if check == last.to_string() { Some(upper) } else { None }
        },
        _ => None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaInformation {
    pub title: String,
    pub authors: Vec<String>,
    pub title_sort: Option<String>,
    pub author_sort: Option<String>,
    pub author_sort_map: HashMap<String, String>,
    pub publisher: Option<String>,
    pub tags: Vec<String>,
    pub series: Option<String>,
    pub series_index: f64,
    pub rating: Option<f64>,
    pub pubdate: Option<DateTime<Utc>>,
    pub timestamp: Option<DateTime<Utc>>,
    pub comments: Option<String>,
    pub languages: Vec<String>,
    pub identifiers: HashMap<String, String>,
    
    // Simplification for user_metadata: storing as generic JSON-like structure or just string for now?
    // Using a simple map for custom fields
    pub user_metadata: HashMap<String, String>, 
    
    pub cover_id: Option<String>, // ID in OPF manifest
    pub uuid: Option<String>,
}

impl Default for MetaInformation {
    fn default() -> Self {
        MetaInformation {
            title: "Unknown".to_string(),
            authors: vec!["Unknown".to_string()],
            title_sort: None,
            author_sort: None,
            author_sort_map: HashMap::new(),
            publisher: None,
            tags: Vec::new(),
            series: None,
            series_index: 1.0,
            rating: None,
            pubdate: None,
            timestamp: Some(Utc::now()),
            comments: None,
            languages: vec!["und".to_string()],
            identifiers: HashMap::new(),
            user_metadata: HashMap::new(),
            cover_id: None,
            uuid: None,
        }
    }
}

impl MetaInformation {
    pub fn new(title: &str, authors: Vec<String>) -> Self {
        MetaInformation {
            title: title.to_string(),
            authors,
            ..Default::default()
        }
    }
}
