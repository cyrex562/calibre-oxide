use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct TXTOutput;

impl TXTOutput {
    pub fn new() -> Self {
        TXTOutput
    }

    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        let mut file = File::create(output_path).context("Failed to create output TXT file")?;

        // Iterate over spine
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                // Read content from container
                // Assuming it's HTML/XHTML based on typical OEB usage.
                if let Ok(data) = book.container.read(&item.href) {
                    let text = String::from_utf8_lossy(&data);

                    // Convert to text
                    let converted = html2text(&text);

                    file.write_all(converted.as_bytes())?;
                    file.write_all(b"\n\n")?;
                }
            }
        }

        Ok(())
    }
}
