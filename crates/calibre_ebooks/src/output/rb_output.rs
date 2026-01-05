use crate::oeb::book::OEBBook;
use crate::rb::writer::RbWriter;
use anyhow::Result;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct RBOutput;

impl RBOutput {
    pub fn new() -> Self {
        RBOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut rb_writer = RbWriter::new();

        // Flatten content
        // Similar to PDB, we just concat spine items.
        // RB format expects HTML-like content.
        let mut full_html = String::new();

        // Basic head
        full_html.push_str("<html><head><title>");
        let title = book
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or("Unknown".to_string());
        full_html.push_str(&title);
        full_html.push_str("</title></head><body>");

        for item in &book.spine.items {
            if let Some(manifest_item) = book.manifest.get_by_id(&item.idref) {
                if let Ok(data) = book.container.read(&manifest_item.href) {
                    full_html.push_str(&String::from_utf8_lossy(&data));
                }
            }
        }
        full_html.push_str("</body></html>");

        // Add content entry
        // Flag 0 = Content?
        rb_writer.add_entry("content.html", full_html.into_bytes(), 0);

        // Add Info entry (Flag 2)
        let info = format!("TITLE={}\nAUTHOR={}", title, "Unknown");
        rb_writer.add_entry("info", info.into_bytes(), 2);

        // Write
        let file = File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        rb_writer.write(&mut writer)?;

        Ok(())
    }
}
