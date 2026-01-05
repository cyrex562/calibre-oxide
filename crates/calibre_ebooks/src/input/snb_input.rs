use crate::oeb::book::OEBBook;
use crate::snb::reader::SnbReader;
use anyhow::{Context, Result};
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

        let container = Box::new(crate::oeb::container::DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // 1. Extract Metadata from snbf/book.snbf
        if let Some(data) = snb.get_file("snbf/book.snbf") {
            let xml_str = String::from_utf8_lossy(&data);
            let doc = roxmltree::Document::parse(&xml_str).context("Failed to parse SNB XML")?;

            // Title
            if let Some(name) = doc.descendants().find(|n| n.has_tag_name("name")) {
                if let Some(text) = name.text() {
                    book.metadata.add("title", text);
                }
            }

            // Author
            if let Some(author) = doc.descendants().find(|n| n.has_tag_name("author")) {
                if let Some(text) = author.text() {
                    book.metadata.add("creator", text);
                }
            }

            // Language
            if let Some(lang) = doc.descendants().find(|n| n.has_tag_name("language")) {
                if let Some(text) = lang.text() {
                    book.metadata.add("language", text);
                }
            }
        }

        // 2. Content Extraction
        // SNB content structure is complex. For this port, we acknowledge the limitation.
        // We will create a placeholder content file.
        fs::create_dir_all(output_dir)?;
        let content_path = output_dir.join("index.html");
        fs::write(&content_path, "<html><body><h1>SNB Content</h1><p>Content extraction not fully implemented.</p></body></html>")?;

        let id = "content".to_string();
        let href = "index.html".to_string();
        book.manifest.add(&id, &href, "application/xhtml+xml");
        book.spine.add(&id, true);

        Ok(book)
    }
}
