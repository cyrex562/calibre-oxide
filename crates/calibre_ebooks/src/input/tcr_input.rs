use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct TCRInput;

impl TCRInput {
    pub fn new() -> Self {
        TCRInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // TCR is the text compression format for Psion.
        // Full decompression is not implemented in this phase.
        // We will create a placeholder.

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);

        let html_content = "<html><body><h1>TCR Content Not Supported Yet</h1><p>The TCR format (Psion) is not yet supported for full text extraction.</p></body></html>";
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

        book.metadata.add("title", "Converted TCR");
        book.metadata.add("creator", "Unknown");

        Ok(book)
    }
}
