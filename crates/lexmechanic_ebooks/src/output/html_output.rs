use crate::oeb::book::OEBBook;
use crate::oeb::writer::OEBWriter;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct HTMLOutput;

impl HTMLOutput {
    pub fn new() -> Self {
        HTMLOutput
    }

    pub fn convert(&self, book: &mut OEBBook, output_dir: &Path) -> Result<()> {
        if !output_dir.exists() {
            fs::create_dir_all(output_dir).context("Failed to create output directory")?;
        }

        let writer = OEBWriter::new();
        writer
            .write_book(book, output_dir)
            .context("Failed to write HTML content")?;

        // Optional: If we wanted to ensure index.html matches spine, OEBWriter writes what's in manifest.
        // It's up to OEBBook state.

        Ok(())
    }
}
