use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct RTFOutput;

impl RTFOutput {
    pub fn new() -> Self {
        RTFOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut file = File::create(output_path).context("Failed to create RTF file")?;

        // Write RTF Header
        writeln!(file, "{{\\rtf1\\ansi\\deff0")?;
        writeln!(file, "{{\\fonttbl{{\\f0 Arial;}}}}")?;
        writeln!(file, "\\f0\\fs24")?; // Font 0, Size 12pt (24 half-points)

        // Iterate content
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let text_content = String::from_utf8_lossy(&data);
                    // convert html to text first
                    let plain_text = html2text(&text_content);

                    // Escape for RTF
                    let rtf_text = self.escape_rtf(&plain_text);
                    writeln!(file, "{} \\par", rtf_text)?;
                }
            }
        }

        writeln!(file, "}}")?; // Close RTF

        Ok(())
    }

    fn escape_rtf(&self, text: &str) -> String {
        text.replace('\\', "\\\\")
            .replace('{', "\\{")
            .replace('}', "\\}")
            .replace('\n', "\\par\n")
    }
}
