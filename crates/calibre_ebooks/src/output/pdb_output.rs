use crate::oeb::book::OEBBook;
use crate::pdb::writer::PdbWriter;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use std::fs;
use std::path::Path;

pub struct PDBOutput;

impl PDBOutput {
    pub fn new() -> Self {
        PDBOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let writer = PdbWriter::new();

        // Accumulate all content
        let mut all_content = String::new();
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let html = String::from_utf8_lossy(&data);
                    let text = html2text(&html);
                    all_content.push_str(&text);
                    all_content.push('\n');
                }
            }
        }

        let mut file = fs::File::create(output_path)?;
        writer.write("GenericPDB", all_content.as_bytes(), &mut file)?;
        Ok(())
    }
}
