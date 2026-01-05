use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

pub struct ODTInput;

impl ODTInput {
    pub fn new() -> Self {
        ODTInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        fs::create_dir_all(output_dir)?;

        let file = fs::File::open(input_path).context("Failed to open ODT file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ODT zip")?;

        // Extract content.xml
        let mut content_xml = String::new();
        match archive.by_name("content.xml") {
            Ok(mut file) => {
                file.read_to_string(&mut content_xml)?;
            }
            Err(_) => {
                // Try basic extraction if content.xml missing (unlikely for valid ODT)
                // For now, just error or create empty
                content_xml = "<p>No content.xml found</p>".to_string();
            }
        }

        // Basic XML parsing to extract text (naive implementation for now)
        // In a real implementation, we would parse styles.xml, meta.xml etc.
        // For this port, we'll try to extract paragraphs from content.xml

        // Remove namespaces for simpler parsing (generic regex or just treat as string)
        let body_content = if let Some(start) = content_xml.find("<office:body>") {
            if let Some(end) = content_xml.find("</office:body>") {
                &content_xml[start..end + 14]
            } else {
                &content_xml
            }
        } else {
            &content_xml
        };

        // Wrap in HTML
        let html_content = format!(
            "<html><head><title>ODT Content</title></head><body>{}</body></html>",
            body_content
        );

        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);
        fs::write(&content_path, &html_content)?;

        // Build OEBBook
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        let id = "content".to_string();
        let href = content_filename.to_string();

        book.manifest.items.insert(
            id.clone(),
            ManifestItem::new(&id, &href, "application/xhtml+xml"),
        );
        book.manifest.hrefs.insert(href.clone(), id.clone());
        book.spine.add(&id, true);

        // Metadata (Stub)
        book.metadata.add("title", "ODT Document");
        book.metadata.add("creator", "Unknown");

        Ok(book)
    }
}
