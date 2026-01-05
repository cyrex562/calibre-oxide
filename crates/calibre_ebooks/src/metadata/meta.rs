use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

lazy_static! {
    static ref TITLE_PFX_PAT: Regex = Regex::new(r"^(A|The|An)\s+").unwrap();
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
    if chars.len() < 9 {
        return "X".to_string();
    }
    let sum: u32 = chars
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, &d)| (i as u32 + 1) * d)
        .sum();
    let check = sum % 11;
    if check == 10 {
        "X".to_string()
    } else {
        check.to_string()
    }
}

pub fn check_digit_isbn13(isbn: &str) -> String {
    let chars: Vec<u32> = isbn.chars().filter_map(|c| c.to_digit(10)).collect();
    if chars.len() < 12 {
        return "0".to_string();
    }
    let sum: u32 = chars
        .iter()
        .take(12)
        .enumerate()
        .map(|(i, &d)| if i % 2 == 0 { d } else { d * 3 })
        .sum();
    let check = 10 - (sum % 10);
    if check == 10 {
        "0".to_string()
    } else {
        check.to_string()
    }
}

pub fn check_isbn(isbn: &str) -> Option<String> {
    if isbn.is_empty() {
        return None;
    }
    let clean: String = isbn
        .chars()
        .filter(|c| c.is_digit(10) || *c == 'X' || *c == 'x')
        .collect();
    let upper = clean.to_uppercase();
    match upper.len() {
        10 => {
            let last = upper.chars().last()?;
            let check = check_digit_isbn10(&upper);
            if check == last.to_string() {
                Some(upper)
            } else {
                None
            }
        }
        13 => {
            let last = upper.chars().last()?;
            let check = check_digit_isbn13(&upper);
            if check == last.to_string() {
                Some(upper)
            } else {
                None
            }
        }
        _ => None,
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

    pub cover_id: Option<String>,              // ID in OPF manifest
    pub cover_data: (Option<String>, Vec<u8>), // (Extension, Data)
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
            cover_data: (None, Vec::new()),
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

    pub fn set_identifier(&mut self, key: &str, value: &str) {
        self.identifiers.insert(key.to_string(), value.to_string());
    }

    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\">\n  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");

        // Title
        out.push_str(&format!(
            "    <dc:title>{}</dc:title>\n",
            xml_escape(&self.title)
        ));

        // Authors
        for author in &self.authors {
            let sort = self
                .author_sort_map
                .get(author)
                .cloned()
                .unwrap_or_else(|| author.clone());
            out.push_str(&format!(
                "    <dc:creator opf:role=\"aut\" opf:file-as=\"{}\">{}</dc:creator>\n",
                xml_escape(&sort),
                xml_escape(author)
            ));
        }

        // UUID
        if let Some(uuid) = &self.uuid {
            out.push_str(&format!(
                "    <dc:identifier id=\"uuid_id\" opf:scheme=\"uuid\">{}</dc:identifier>\n",
                xml_escape(uuid)
            ));
        }

        // Description
        if let Some(desc) = &self.comments {
            out.push_str(&format!(
                "    <dc:description>{}</dc:description>\n",
                xml_escape(desc)
            ));
        }

        // Publisher
        if let Some(publ) = &self.publisher {
            out.push_str(&format!(
                "    <dc:publisher>{}</dc:publisher>\n",
                xml_escape(publ)
            ));
        }

        // Tags (Subjects)
        for tag in &self.tags {
            out.push_str(&format!(
                "    <dc:subject>{}</dc:subject>\n",
                xml_escape(tag)
            ));
        }

        // Series
        if let Some(series) = &self.series {
            out.push_str(&format!(
                "    <meta name=\"calibre:series\" content=\"{}\"/>\n",
                xml_escape(series)
            ));
            out.push_str(&format!(
                "    <meta name=\"calibre:series_index\" content=\"{}\"/>\n",
                self.series_index
            ));
        }

        // Timestamp
        if let Some(ts) = &self.timestamp {
            out.push_str(&format!(
                "    <meta name=\"calibre:timestamp\" content=\"{}\"/>\n",
                ts.to_rfc3339()
            ));
            out.push_str(&format!(
                "    <dc:date opf:event=\"modification\">{}</dc:date>\n",
                ts.to_rfc3339()
            ));
        }
        if let Some(pd) = &self.pubdate {
            out.push_str(&format!(
                "    <dc:date opf:event=\"publication\">{}</dc:date>\n",
                pd.to_rfc3339()
            ));
        }

        // Rating
        if let Some(rating) = self.rating {
            out.push_str(&format!(
                "    <meta name=\"calibre:rating\" content=\"{}\"/>\n",
                rating
            ));
        }

        out.push_str("  </metadata>\n  <guide/>\n</package>");
        out
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
