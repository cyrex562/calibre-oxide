use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use std::fs;
use std::path::Path;

pub struct TCROutput;

impl TCROutput {
    pub fn new() -> Self {
        TCROutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut combined_text = String::new();
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let html = String::from_utf8_lossy(&data);
                    let text = html2text(&html);
                    combined_text.push_str(&text);
                    combined_text.push_str("\n");
                }
            }
        }

        // Write as plain text for now, as proper TCR compression is low priority and complex
        fs::write(output_path, combined_text).context("Failed to write TCR file")?;
        Ok(())
    }
}
