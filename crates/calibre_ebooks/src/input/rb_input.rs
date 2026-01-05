use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::rb::reader::RbReader;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::Path;

pub struct RBInput;

impl RBInput {
    pub fn new() -> Self {
        RBInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = fs::File::open(input_path).context("Failed to open RB file")?;
        let reader = BufReader::new(file);
        let mut rb_reader = RbReader::new(reader).context("Failed to parse RB header")?;

        // Prepare Output
        fs::create_dir_all(output_dir)?;

        // Extract Content
        let content = rb_reader
            .read_content()
            .context("Failed to read RB content")?;

        let content_filename = "content.html";
        let content_path = output_dir.join(content_filename);
        fs::write(&content_path, &content)?;

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

        // Metadata extraction could be improved here, currently simplistic.
        // We could use `metadata/rb.rs` logic or extract from `rb_reader.header`.
        // Ideally RbReader exposes metadata too.
        // For now, let's assume basic conversion.
        book.metadata.add("title", "Unknown RB Book");

        Ok(book)
    }
}
