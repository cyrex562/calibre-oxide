use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct LRFInput;

impl LRFInput {
    pub fn new() -> Self {
        LRFInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // Just extract metadata and create a placeholder book.
        // Full LRF content extraction is out of scope for this batch.

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);

        let html_content = "<html><body><h1>LRF Content Not Supported Yet</h1><p>The LRF format is a proprietary binary format. Content extraction is not yet implemented.</p></body></html>";
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

        // Metadata
        if let Ok(info) = crate::metadata::lrf::get_metadata(fs::File::open(input_path)?) {
            book.metadata.add("title", &info.title);
            if !info.authors.is_empty() {
                book.metadata.add("creator", &info.authors[0]);
            }
        }

        if book.metadata.get("title").is_empty() {
            book.metadata.add("title", "Converted LRF");
        }

        Ok(book)
    }
}
