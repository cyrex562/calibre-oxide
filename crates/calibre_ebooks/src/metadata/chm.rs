use crate::metadata::MetaInformation;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref META_PATTERN: Regex = Regex::new(r#"(?i)<meta\s+name="([^"]+)"\s+content="([^"]+)"#).unwrap();
    static ref TITLE_TAG_PATTERN: Regex = Regex::new(r"(?i)<title>([^<]+)</title>").unwrap();
    // Patterns simulating the legacy BeautifulSoup finds in _metadata_from_table/span
    static ref AUTHOR_PATTERN: Regex = Regex::new(r"(?i)author|by\s*:?\s+").unwrap();
    static ref PUBLISHER_PATTERN: Regex = Regex::new(r"(?i)imprint|publisher").unwrap();
    static ref ISBN_PATTERN: Regex = Regex::new(r"(?i)isbn").unwrap();
}

use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(_stream: R) -> Result<MetaInformation> {
    // CHM is a complex binary format (ITSS/LZX).
    // Without a CHM reader implementation (which is complex),
    // we cannot extract the "home" HTML file to parse.
    // This is a placeholder that would normally read the CHM, extract the home HTML,
    // and pass it to parsing logic.
    // For now, we return empty metadata or error.
    // In a real implementation with a CHM crate, we would:
    // 1. Read CHM directory.
    // 2. Find home/index file.
    // 3. metadata_from_html(content)

    // Returning default for now to satisfy interface,
    // or we could error "CHM reading not supported".
    Ok(MetaInformation::default())
}

/// Extracts metadata from the CHM "home" HTML content.
/// This attempts to replicate the logic of `_metadata_from_table`, `_metadata_from_span`, etc.
/// using Regex since we lack a full HTML parser.
pub fn metadata_from_html(html: &str) -> MetaInformation {
    let mut mi = MetaInformation::default();
    mi.title = String::new();
    mi.authors.clear();

    // 1. Basic Meta Tags
    // Robust approach: Find <meta ...> tag content, then extract name/content attributes from it.
    let meta_tag_re = Regex::new(r"(?i)<meta([^>]+)>").unwrap();
    let name_re = Regex::new(r#"(?i)name=["']([^"']+)["']"#).unwrap();
    let content_re = Regex::new(r#"(?i)content=["']([^"']+)["']"#).unwrap();

    for cap in meta_tag_re.captures_iter(html) {
        if let Some(attrs) = cap.get(1) {
            let attrs_str = attrs.as_str();
            let name = name_re
                .captures(attrs_str)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .unwrap_or("");
            let content = content_re
                .captures(attrs_str)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .unwrap_or("");

            if !name.is_empty() && !content.is_empty() {
                if name.eq_ignore_ascii_case("title") {
                    mi.title = content.trim().to_string();
                } else if name.eq_ignore_ascii_case("author")
                    || name.eq_ignore_ascii_case("creator")
                {
                    mi.authors.push(content.trim().to_string());
                } else if name.eq_ignore_ascii_case("isbn") {
                    mi.set_identifier("isbn", content.trim());
                }
            }
        }
    }

    // 2. Title Tag fallback
    if mi.title.is_empty() {
        if let Some(cap) = TITLE_TAG_PATTERN.captures(html) {
            if let Some(m) = cap.get(1) {
                mi.title = m.as_str().trim().to_string();
            }
        }
    }
    mi
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chm_html_parsing() {
        eprintln!("DEBUG: Starting CHM test");
        let html = r#"<html><head><title>Test Book Title</title><meta name="Author" content="John Doe"></head><body><h1>Welcome</h1></body></html>"#;

        let mi = metadata_from_html(html);
        eprintln!("Parsed Title: '{}'", mi.title);
        eprintln!("Parsed Authors: {:?}", mi.authors);
        assert_eq!(mi.title, "Test Book Title");
        assert_eq!(mi.authors, vec!["John Doe"]);
    }
}
