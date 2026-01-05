use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::pdb::reader::PdbReader;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct PDBInput;

impl PDBInput {
    pub fn new() -> Self {
        PDBInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        fs::create_dir_all(output_dir)?;

        let mut reader = PdbReader::new(input_path).context("Failed to open PDB")?;

        let header = reader.header.clone();
        let num_records = header.num_records;

        let mut content = String::new();
        content.push_str("<html><body><h1>PDB Records</h1><ul>");

        // Dump records to files
        for i in 0..num_records {
            if let Ok(data) = reader.read_record(i as usize) {
                let filename = format!("record_{}.bin", i);
                let filepath = output_dir.join(&filename);
                fs::write(&filepath, &data)?;

                // Try to treat as text for the index page
                let text_preview = String::from_utf8_lossy(&data);
                let preview_short = if text_preview.len() > 100 {
                    &text_preview[..100]
                } else {
                    &text_preview
                };

                content.push_str(&format!(
                    "<li><b>Record {}:</b> {} bytes - Preview: {}...</li>",
                    i,
                    data.len(),
                    html_escape::encode_text(preview_short)
                ));
            }
        }
        content.push_str("</ul></body></html>");

        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);
        fs::write(&content_path, &content)?;

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

        // Metadata from PDB Header
        let name = header.name.clone();
        book.metadata.add("title", &name);
        book.metadata.add("creator", "Unknown"); // PDB header doesn't have author field in standard part

        Ok(book)
    }
}
