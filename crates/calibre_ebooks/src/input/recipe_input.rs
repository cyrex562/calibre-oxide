use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct RecipeInput;

impl RecipeInput {
    pub fn new() -> Self {
        RecipeInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // Recipes are Python scripts. We cannot execute them safely in this Rust port without an embedded Python interpreter.
        // Returning a placeholder.

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);

        let html_content = format!(
            "<html><body><h1>Recipe Execution Not Supported</h1><p>The recipe '{}' relies on Python script execution which is not supported.</p></body></html>",
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
        book.metadata.add("title", "Recipe Result");

        Ok(book)
    }
}
