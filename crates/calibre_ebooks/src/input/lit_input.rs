use crate::lit::reader::LitReader;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;

pub struct LitInput;

impl LitInput {
    pub fn new() -> Self {
        LitInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = fs::File::open(input_path).context("Failed to open LIT file")?;
        let reader = BufReader::new(file);
        let mut lit_reader = LitReader::new(reader).context("Failed to parse LIT header")?;

        // Prepare Output
        fs::create_dir_all(output_dir)?;

        // Extract Content
        // Since extraction is limited, we might just write a placeholder or metadata.
        let content = lit_reader.extract_content()?;

        let content_filename = "content.html";
        let content_path = output_dir.join(content_filename);
        let html_content = format!(
            "<html><body><h1>LIT Conversion</h1><p>{}</p></body></html>",
            content
        );
        fs::write(&content_path, &html_content)?;

        // Build OEBBook
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // Add Content to Manifest & Spine
        let id = "content".to_string();
        let href = content_filename.to_string();

        book.manifest.items.insert(
            id.clone(),
            ManifestItem::new(&id, &href, "application/xhtml+xml"),
        );
        book.manifest.hrefs.insert(href.clone(), id.clone());
        book.spine.add(&id, true);

        book.metadata.add("title", "Converted LIT Book");

        Ok(book)
    }
}
