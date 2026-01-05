use crate::docx::container::DOCX;
use crate::docx::to_html::DOCXToHTML;
use crate::input::html_input::HTMLInput;
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;

pub struct DOCXInput;

impl DOCXInput {
    pub fn new() -> Self {
        DOCXInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = File::open(input_path).context("Failed to open DOCX file")?;

        let mut docx = DOCX::new(file).map_err(|e| anyhow::anyhow!("DOCX Error: {}", e))?;

        // Create source dir for HTML output
        let source_dir = output_dir.join("docx_source");
        std::fs::create_dir_all(&source_dir)?;

        let html_content = DOCXToHTML::convert(&mut docx, &source_dir)
            .map_err(|e| anyhow::anyhow!("Conversion Error: {}", e))?;

        let index_path = source_dir.join("index.html");
        std::fs::write(&index_path, html_content)?;

        // Delegate to HTMLInput
        // HTMLInput copies content to ITS output_dir.
        // So we pass output_dir to HTMLInput.
        let html_input = HTMLInput::new();
        html_input.convert(&index_path, output_dir)
    }
}
