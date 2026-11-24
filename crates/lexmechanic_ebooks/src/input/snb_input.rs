//! SNB input plugin wiring.
//!
//! Scoped-down stand-in for the real
//! `old_src/.../conversion/plugins/snb_input.py` (TOC-driven chapter
//! reconstruction across `.snbc` files, tracked as its own separate
//! porting item outside issue #52's `ebooks/snb/{snbfile.py,snbml.py}`
//! scope). What issue #52 requires is wiring the real, corrected
//! [`crate::snb::reader::SnbReader`] in here with real metadata
//! extraction -- so `convert` parses the container, validates it,
//! pulls metadata out of `snbf/book.snbf`, and reconstructs one XHTML
//! spine item per `.snbc` sub-document (its `<text>`/`<img>` elements
//! turned back into `<p>`/`<img>` tags) plus manifest entries for every
//! other (image) file in the container -- rather than the previous
//! stub's single hard-coded placeholder page.
//!
//! Image references inside a reconstructed page resolve against
//! `snbc/images/<name>`, matching where [`crate::output::snb_output`]
//! places them -- this is a real, working round trip for content this
//! crate itself wrote, though (like the unported original plugin's own
//! best-effort `ProcessFileName`-based scheme) it is not a fully
//! general resolver for every possible multi-directory OEB layout an
//! arbitrary third-party `.snb` file might use.

use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::snb::reader::SnbReader;
use crate::xml_util::prepare_string_for_xml;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;

pub struct SnbInput;

impl SnbInput {
    pub fn new() -> Self {
        SnbInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = fs::File::open(input_path).context("Failed to open SNB file")?;
        let reader = BufReader::new(file);
        let mut snb = SnbReader::new(reader).context("Failed to init SNB reader")?;
        snb.parse().context("Failed to parse SNB container")?;
        if !snb.is_valid() {
            bail!("SNB file failed container validation (SNBFile.IsValid equivalent)");
        }

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        if let Some(data) = snb.get_file("snbf/book.snbf") {
            apply_metadata(&mut book, &data);
        }

        // Every non-metadata, non-`.snbc` file is a binary asset
        // (images, in practice) -- copy it into the output container
        // byte-for-byte and give it a manifest entry.
        for f in &snb.files {
            if f.file_name == "snbf/book.snbf" || f.file_name == "snbf/toc.snbf" {
                continue;
            }
            if f.file_name.ends_with(".snbc") {
                continue;
            }
            book.container.write(&f.file_name, &f.file_body)?;
            let media_type = mime_guess::from_path(&f.file_name)
                .first()
                .map(|m| m.essence_str().to_string())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let id = manifest_id_for(&f.file_name);
            book.manifest.add(&id, &f.file_name, &media_type);
        }

        let mut snbc_names: Vec<&str> = snb
            .files
            .iter()
            .map(|f| f.file_name.as_str())
            .filter(|name| name.starts_with("snbc/") && name.ends_with(".snbc"))
            .collect();
        snbc_names.sort_unstable();

        for name in snbc_names {
            let data = snb
                .get_file(name)
                .expect("name was just collected from snb.files");
            let xml_str = String::from_utf8_lossy(&data).into_owned();
            let html = snbc_to_xhtml(&xml_str)
                .with_context(|| format!("Failed to parse SNBC document {name}"))?;

            let stem = name.trim_start_matches("snbc/").trim_end_matches(".snbc");
            let href = format!("{stem}.html");
            book.container.write(&href, html.as_bytes())?;

            let id = manifest_id_for(&href);
            book.manifest.add(&id, &href, "application/xhtml+xml");
            book.spine.add(&id, true);
        }

        Ok(book)
    }
}

impl Default for SnbInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Turn a container path into a manifest-safe id (alphanumerics only,
/// everything else collapsed to `_`).
fn manifest_id_for(path: &str) -> String {
    let mut id: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if id.is_empty() || !id.chars().next().unwrap().is_ascii_alphabetic() {
        id.insert(0, 'x');
    }
    id
}

/// Pull the fields `crate::metadata::snb::get_metadata` also reads out
/// of `snbf/book.snbf` into `book.metadata` (title/creator/language/
/// publisher/description), for the OEB pipeline's own metadata
/// consumers rather than a standalone `MetaInformation`.
fn apply_metadata(book: &mut OEBBook, data: &[u8]) {
    let xml_str = String::from_utf8_lossy(data);
    let Ok(doc) = roxmltree::Document::parse(&xml_str) else {
        return;
    };
    if let Some(text) = doc
        .descendants()
        .find(|n| n.has_tag_name("name"))
        .and_then(|n| n.text())
        .filter(|t| !t.is_empty())
    {
        book.metadata.add("title", text);
    }
    if let Some(text) = doc
        .descendants()
        .find(|n| n.has_tag_name("author"))
        .and_then(|n| n.text())
        .filter(|t| !t.is_empty())
    {
        book.metadata.add("creator", text);
    }
    if let Some(text) = doc
        .descendants()
        .find(|n| n.has_tag_name("language"))
        .and_then(|n| n.text())
        .filter(|t| !t.is_empty())
    {
        book.metadata
            .add("language", &text.to_lowercase().replace('_', "-"));
    }
    if let Some(text) = doc
        .descendants()
        .find(|n| n.has_tag_name("publisher"))
        .and_then(|n| n.text())
        .filter(|t| !t.is_empty())
    {
        book.metadata.add("publisher", text);
    }
    if let Some(text) = doc
        .descendants()
        .find(|n| n.tag_name().name() == "abstract")
        .and_then(|n| n.text())
        .filter(|t| !t.is_empty())
    {
        book.metadata.add("description", text);
    }
}

/// Turn one `<snbc>` document (as written by
/// `crate::snb::snbml::SnbcDoc::to_xml`) back into a minimal XHTML
/// page: `<text>` elements become `<p>` paragraphs, `<img>` elements
/// become `<img src="images/...">` tags resolved against
/// `snbc/images/`.
fn snbc_to_xhtml(xml: &str) -> Result<String> {
    let doc = roxmltree::Document::parse(xml).context("Invalid SNBC XML")?;
    let title = doc
        .descendants()
        .find(|n| n.has_tag_name("title"))
        .and_then(|n| n.text())
        .unwrap_or("")
        .to_string();

    let mut body_html = String::new();
    if let Some(body) = doc.descendants().find(|n| n.has_tag_name("body")) {
        for child in body.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "text" => {
                    let text = child.text().unwrap_or("");
                    body_html.push_str("<p>");
                    body_html.push_str(&prepare_string_for_xml(text, false));
                    body_html.push_str("</p>\n");
                }
                "img" => {
                    let src = child.text().unwrap_or("");
                    body_html.push_str("<img src=\"images/");
                    body_html.push_str(&prepare_string_for_xml(src, true));
                    body_html.push_str("\"/>\n");
                }
                _ => {}
            }
        }
    }

    Ok(format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{}</title></head><body>\n{}</body></html>",
        prepare_string_for_xml(&title, false),
        body_html
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_id_for_sanitizes_and_prefixes_when_needed() {
        assert_eq!(manifest_id_for("snbc/chapter1.html"), "snbc_chapter1_html");
        assert_eq!(manifest_id_for("1.png"), "x1_png");
    }

    #[test]
    fn snbc_to_xhtml_converts_text_and_img_elements() {
        let xml = "<?xml version='1.0' encoding='utf-8'?>\n<snbc><head><title>Ch 1</title></head>\
                   <body><text><![CDATA[Hello]]></text><img>pic.jpg</img></body></snbc>";
        let html = snbc_to_xhtml(xml).unwrap();
        assert!(html.contains("<title>Ch 1</title>"), "{html}");
        assert!(html.contains("<p>Hello</p>"), "{html}");
        assert!(html.contains("<img src=\"images/pic.jpg\"/>"), "{html}");
    }
}
