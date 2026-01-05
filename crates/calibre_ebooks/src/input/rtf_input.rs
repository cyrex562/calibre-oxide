use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::Path;

pub struct RTFInput;

impl RTFInput {
    pub fn new() -> Self {
        RTFInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // Read RTF content
        let mut file = fs::File::open(input_path).context("Failed to open RTF file")?;
        let mut content = String::new();
        // RTF is 7-bit ASCII usually, but can contain others. Reading as string is a safe bet for basic ones.
        // If it fails UTF-8, we might need lossy.
        file.read_to_string(&mut content).or_else(|_| {
            let mut file = fs::File::open(input_path)?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            content = String::from_utf8_lossy(&buffer).to_string();
            Ok::<usize, anyhow::Error>(buffer.len())
        })?;

        // 1. Basic Metadata extraction (Reuse existing logic if possible, or simple regex)
        // For now, we will rely on plumber to run metadata extraction separately if needed,
        // or just set basic title.

        fs::create_dir_all(output_dir)?;

        let html_content = self.rtf_to_html(&content);
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

        book.metadata.add("title", "Converted RTF Document");

        Ok(book)
    }

    fn rtf_to_html(&self, rtf: &str) -> String {
        // Very rudimentary RTF to HTML.
        // Real RTF parsing is huge.
        // We will just wrap it in pre tags or do basic cleanup if it's raw text.
        // A proper implementation would need an rtf crate.

        // Check if it looks like RTF
        if !rtf.trim_start().starts_with("{\\rtf") {
            // Treat as text
            return format!(
                "<html><body><pre>{}</pre></body></html>",
                html_escape::encode_text(rtf)
            );
        }

        // Strip RTF tags for a plain text approximation?
        // Or just put it in a block.
        // For this port, we acknowledge we are not writing a full RTF engine.
        format!(
            "<html><body><h1>RTF Content</h1><pre>{}</pre></body></html>",
            html_escape::encode_text(rtf)
        )
    }
}
