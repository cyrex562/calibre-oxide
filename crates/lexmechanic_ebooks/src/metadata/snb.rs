use crate::metadata::MetaInformation;
use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

// Re-export reader for existing usage if needed, or just use it directly.
// The get_metadata function uses SnbReader internally.

use crate::snb::reader::SnbReader;

pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    let mut snb = SnbReader::new(stream)?;
    snb.parse()?;

    let mut mi = MetaInformation::default();

    // Extract snbf/book.snbf
    // The filename might be simplified or relative?
    // Python code: snbFile.GetFileStream('snbf/book.snbf')
    if let Some(data) = snb.get_file("snbf/book.snbf") {
        let xml_str = String::from_utf8_lossy(&data);
        // XML parsing similar to other modules
        let doc = roxmltree::Document::parse(&xml_str).context("Failed to parse SNB XML")?;

        // mi.title = meta.find('.//head/name').text
        if let Some(name) = doc.descendants().find(|n| n.has_tag_name("name")) {
            if let Some(text) = name.text() {
                mi.title = text.to_string();
            }
        }

        // head/author
        if let Some(author) = doc.descendants().find(|n| n.has_tag_name("author")) {
            if let Some(text) = author.text() {
                mi.authors = vec![text.to_string()];
            }
        }

        // head/language
        if let Some(lang) = doc.descendants().find(|n| n.has_tag_name("language")) {
            if let Some(text) = lang.text() {
                mi.languages = vec![text.to_lowercase().replace('_', "-")];
            }
        }

        // head/publisher
        if let Some(publ) = doc.descendants().find(|n| n.has_tag_name("publisher")) {
            if let Some(text) = publ.text() {
                mi.publisher = Some(text.to_string());
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snb_parsing() {
        // Construct a mock SNB file? Very complex.
        // Just verify basic struct or error on empty
        let dummy = vec![0u8; 100];
        let res = get_metadata(Cursor::new(dummy));
        assert!(res.is_err()); // Invalid Header
    }
}
