use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct CHMInput;

impl CHMInput {
    pub fn new() -> Self {
        CHMInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // CHM (Compiled HTML Help) requires complex binary parsing (LZX/ITSS).
        // This is a placeholder implementation.
        // A full implementation would need a CHM crate (e.g., chm-rs if it existed/was mature)
        // or binding to a C library.

        // For now, we return a "not supported" page, similar to TCR.

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);

        let html_content = format!(
            "<html><body><h1>CHM Content Not Supported Yet</h1><p>The CHM file '{}' cannot be fully converted yet.</p></body></html>",
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

        // Try to get metadata using our metadata module if possible
        // if let Ok(file) = fs::File::open(input_path) {
        //     if let Ok(info) = crate::metadata::chm::get_metadata(file) {
        //         if !info.title.is_empty() {
        //             book.metadata.add("title", &info.title);
        //         }
        //     }
        // }

        if book.metadata.get("title").is_empty() {
            book.metadata.add("title", "Converted CHM");
        }

        Ok(book)
    }
}
