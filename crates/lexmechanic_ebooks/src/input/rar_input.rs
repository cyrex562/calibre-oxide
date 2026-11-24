use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct RARInput;

impl RARInput {
    pub fn new() -> Self {
        RARInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // Generic RAR support is stubbed due to lack of pure Rust unrar.
        // Similar to CHM and TCR.

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);

        let html_content = format!(
            "<html><body><h1>RAR Content Not Supported Yet</h1><p>The RAR file '{}' cannot be fully converted yet.</p></body></html>",
            input_path.file_name().unwrap_or_default().to_string_lossy()
        );
        fs::write(&content_path, html_content)?;

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
        book.metadata.add("title", "Converted RAR");

        Ok(book)
    }
}
