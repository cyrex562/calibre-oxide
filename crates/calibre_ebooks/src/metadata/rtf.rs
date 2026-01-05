use crate::metadata::MetaInformation;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::bytes::Regex;
use std::io::{Read, Seek, SeekFrom};

lazy_static! {
    static ref TITLE_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\title(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref AUTHOR_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\author(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref COMMENT_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\subject(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref TAGS_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\category(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref PUBLISHER_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\manager(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref CODEPAGE_PAT: Regex = Regex::new(r"\\ansicpg(\d+)").unwrap();
    static ref MATCH_HEX: Regex = Regex::new(r"\\'([0-9a-fA-F]{2})").unwrap();
}

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    stream.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 5];
    stream.read_exact(&mut header)?;
    if &header != b"{\\rtf" {
        // Not RTF
        return Ok(MetaInformation::default());
    }

    // Read initial chunk to find metadata
    // RTF headers usually in first few KB
    stream.seek(SeekFrom::Start(0))?;
    let mut buffer = Vec::with_capacity(8192);
    stream.take(8192).read_to_end(&mut buffer)?;

    let mut mi = MetaInformation::default();
    mi.title = "Unknown".to_string();

    if let Some(cap) = TITLE_PAT.captures(&buffer) {
        let title_raw = &cap[1];
        let title = decode_rtf_string(title_raw);
        if !title.trim().is_empty() {
            mi.title = title.trim().to_string();
        }
    }

    if let Some(cap) = AUTHOR_PAT.captures(&buffer) {
        let auth_raw = &cap[1];
        let auth_str = decode_rtf_string(auth_raw);
        mi.authors = auth_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    if mi.authors.is_empty() {
        mi.authors.push("Unknown".to_string());
    }

    Ok(mi)
}

fn decode_rtf_string(raw: &[u8]) -> String {
    // 1. Convert \'xx to byte
    // 2. Decode bytes as Latin1/CP1252 (Map u8 -> char directly covers Latin1)
    // 3. Handle unicode \uXXXX? (Skipped for now, complex parsing)

    let mut decoded_bytes = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' && i + 3 < raw.len() && raw[i + 1] == b'\'' {
            // Hex escape \'XX
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&raw[i + 2..i + 4]).unwrap_or("00"), 16)
            {
                decoded_bytes.push(byte);
                i += 4;
                continue;
            }
        }
        decoded_bytes.push(raw[i]);
        i += 1;
    }

    // Convert to String assuming Latin1 (1-1 mapping for byte to char)
    // Rust String is UTF-8.
    decoded_bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_rtf_metadata() {
        // Use raw byte string to ensure backslashes are literal
        let rtf_content = br#"{\rtf1\ansi{\info{\title My Title}{\author Me, Myself}}}"#;
        let mut stream = Cursor::new(rtf_content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "My Title", "Title mismatch. Got: '{}'", mi.title);
        assert_eq!(mi.authors, vec!["Me", "Myself"]);
    }

    #[test]
    fn test_rtf_escapes() {
        // \'41 = A
        let rtf_content = br#"{\rtf1\ansi{\info{\title \'41 Title}}}"#;
        let mut stream = Cursor::new(rtf_content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(
            mi.title, "A Title",
            "Title mismatch with escapes. Got: '{}'",
            mi.title
        );
    }

    #[test]
    fn test_no_header() {
        let content = br"Not RTF";
        let mut stream = Cursor::new(content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "Unknown");
    }
}
