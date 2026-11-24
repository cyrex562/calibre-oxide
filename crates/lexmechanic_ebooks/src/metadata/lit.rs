//! Read metadata from LIT files.
//!
//! Port of `src/calibre/ebooks/metadata/lit.py`, which opens the book
//! through `LitContainer` and parses the OPF the container
//! reconstructs from the tokenised `/meta` entry.

use crate::lit::reader::LitContainer;
use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use std::io::{Read, Seek};

/// `get_metadata` in `metadata/lit.py`.
pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    let mut container = LitContainer::new(stream, None).context("Not a valid LIT file")?;
    let opf = container
        .get_metadata()
        .context("Could not read LIT metadata")?;
    Ok(metadata_from_opf1(&opf))
}

/// Pull the Dublin Core fields out of the OEB 1.0.1 package LIT stores.
///
/// The tags are `dc:Title`, `dc:Creator` and friends — OEB 1.0.1 spells
/// them with initial capitals, unlike OPF 2.0.
fn metadata_from_opf1(opf: &str) -> MetaInformation {
    let mut mi = MetaInformation::default();
    // `MetaInformation::default()` pre-fills placeholders; clear the
    // fields this function owns so they reflect the OPF alone.
    mi.title = String::new();
    mi.authors.clear();
    mi.languages.clear();
    if let Ok(doc) = roxmltree::Document::parse(opf) {
        for node in doc.descendants().filter(|n| n.is_element()) {
            let name = node.tag_name().name();
            let text = node.text().unwrap_or("").trim().to_string();
            if text.is_empty() {
                continue;
            }
            match name {
                "Title" => {
                    if mi.title.is_empty() {
                        mi.title = text;
                    }
                }
                "Creator" => mi.authors.push(text),
                "Publisher" => mi.publisher = Some(text),
                "Description" => mi.comments = Some(text),
                "Language" => mi.languages.push(text),
                "Identifier" => {
                    let scheme = node
                        .attribute("scheme")
                        .map(str::to_lowercase)
                        .unwrap_or_else(|| {
                            if looks_like_isbn(&text) {
                                "isbn".to_string()
                            } else {
                                "unknown".to_string()
                            }
                        });
                    mi.identifiers.entry(scheme).or_insert(text);
                }
                _ => {}
            }
        }
    }
    if mi.title.is_empty() {
        mi.title = "Unknown".to_string();
    }
    if mi.authors.is_empty() {
        mi.authors.push("Unknown".to_string());
    }
    if mi.languages.is_empty() {
        mi.languages.push("und".to_string());
    }
    mi
}

/// Whether an identifier looks like an ISBN, so that arbitrary book ids
/// do not end up in the ISBN field.
fn looks_like_isbn(value: &str) -> bool {
    let digits: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    matches!(digits.len(), 10 | 13)
        && digits[..digits.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_files_that_are_not_lit() {
        let stream = Cursor::new(b"not a lit file at all".to_vec());
        assert!(get_metadata(stream).is_err());
    }

    #[test]
    fn reads_the_oeb1_dublin_core_element_names() {
        let opf = r#"<package><metadata><dc-metadata
            xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:Title>A Title</dc:Title>
            <dc:Creator>First Author</dc:Creator>
            <dc:Creator>Second Author</dc:Creator>
            <dc:Publisher>A Publisher</dc:Publisher>
            <dc:Language>en</dc:Language>
            <dc:Identifier>9780306406157</dc:Identifier>
            </dc-metadata></metadata></package>"#;
        let mi = metadata_from_opf1(opf);
        assert_eq!(mi.title, "A Title");
        assert_eq!(mi.authors, vec!["First Author", "Second Author"]);
        assert_eq!(mi.publisher.as_deref(), Some("A Publisher"));
        assert_eq!(mi.languages, vec!["en"]);
        assert_eq!(
            mi.identifiers.get("isbn").map(String::as_str),
            Some("9780306406157")
        );
    }

    #[test]
    fn falls_back_to_unknown_without_a_title() {
        let mi = metadata_from_opf1("<package><metadata /></package>");
        assert_eq!(mi.title, "Unknown");
        assert_eq!(mi.authors, vec!["Unknown".to_string()]);
        assert_eq!(mi.languages, vec!["und".to_string()]);
    }

    #[test]
    fn does_not_file_arbitrary_identifiers_as_isbns() {
        let opf = r#"<package><metadata><dc-metadata>
            <dc:Identifier xmlns:dc="http://purl.org/dc/elements/1.1/"
                >urn:uuid:1234</dc:Identifier>
            </dc-metadata></metadata></package>"#;
        let mi = metadata_from_opf1(opf);
        assert!(mi.identifiers.get("isbn").is_none());
        assert_eq!(
            mi.identifiers.get("unknown").map(String::as_str),
            Some("urn:uuid:1234")
        );
    }

    #[test]
    fn survives_malformed_opf() {
        let mi = metadata_from_opf1("<package><unclosed>");
        assert_eq!(mi.title, "Unknown");
    }
}
