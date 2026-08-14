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

use std::io::{Read, Seek, Write};
use std::path::Path;

use crate::chm::ChmReader;

/// Port of `old_src/src/calibre/ebooks/chm/metadata.py::get_metadata`.
///
/// The Python version wrote the incoming stream to a temp file then
/// opened it with `CHMReader`. Rust does the same — libchm needs a
/// path, and copying via a temp file keeps callers stream-only.
///
/// If the reader fails to open or the home page isn't readable, we
/// fall back to `MetaInformation::default()` matching the Python
/// silent-fallback behavior for unreadable CHMs.
pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let tmp = tempfile::Builder::new()
        .suffix(".chm")
        .tempfile()
        .map_err(|e| anyhow::anyhow!("chm: create tempfile: {e}"))?;
    {
        let mut w = std::fs::File::create(tmp.path())
            .map_err(|e| anyhow::anyhow!("chm: open tempfile for write: {e}"))?;
        std::io::copy(&mut stream, &mut w)
            .map_err(|e| anyhow::anyhow!("chm: copy stream to tempfile: {e}"))?;
        w.flush().ok();
    }
    Ok(get_metadata_from_path(tmp.path()).unwrap_or_default())
}

/// Direct-from-path variant. Preferred when the caller already has a
/// path on disk (avoids the stream→tempfile copy).
pub fn get_metadata_from_path(path: &Path) -> Result<MetaInformation> {
    let mut reader = ChmReader::open(path).map_err(|e| anyhow::anyhow!("chm: open reader: {e}"))?;
    let home_bytes = reader
        .get_home()
        .map_err(|e| anyhow::anyhow!("chm: read home page: {e}"))?;
    // The home page is HTML with an unknown encoding. Try the
    // reader's declared encoding via /#SYSTEM if we ever wire it;
    // for now decode as UTF-8 with lossy fallback.
    let html = String::from_utf8_lossy(&home_bytes).into_owned();
    let mut mi = metadata_from_html(&html);
    // Prefer the CHM's declared title over any title in the HTML.
    if let Some(bytes) = reader.system().title_bytes.as_ref() {
        let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
        let title = decoded.trim().to_string();
        if !title.is_empty() {
            mi.title = title;
        }
    }
    Ok(mi)
}

// Kept for callers that still use the placeholder-shaped fn.
#[allow(dead_code)]
pub fn get_metadata_placeholder<R: Read + Seek>(_stream: R) -> Result<MetaInformation> {
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
        let html = r#"<html><head><title>Test Book Title</title><meta name="Author" content="John Doe"></head><body><h1>Welcome</h1></body></html>"#;

        let mi = metadata_from_html(html);
        assert_eq!(mi.title, "Test Book Title");
        assert_eq!(mi.authors, vec!["John Doe"]);
    }

    #[test]
    fn get_metadata_from_bogus_stream_returns_default() {
        // Stream that isn't a CHM — the port should silently fall
        // back to MetaInformation::default() rather than propagating
        // an error, matching the Python behavior.
        use std::io::Cursor;
        let bogus = Cursor::new(b"not a CHM file".to_vec());
        let mi = get_metadata(bogus).unwrap();
        assert_eq!(mi.title, MetaInformation::default().title);
    }

    #[test]
    fn get_metadata_from_missing_path_errors() {
        // Direct-from-path variant surfaces the error rather than
        // masking — this is where callers who care about failure can
        // observe it.
        let err = get_metadata_from_path(Path::new("/does-not-exist.chm"));
        assert!(err.is_err());
    }
}
