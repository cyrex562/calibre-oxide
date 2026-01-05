use crate::oeb::book::OEBBook;
use crate::oeb::writer::OEBWriter;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct OEBOutput;

impl OEBOutput {
    pub fn new() -> Self {
        OEBOutput
    }

    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        // If output_path is intended as a directory, ensure it exists.
        // If it exists and is a file, error? Or overwrite/delete?
        // Plumber usually handles output path existence logic or passes a target.
        // OEBWriter expects a directory path.

        if output_path.exists() && output_path.is_file() {
            // Error explicitly as we need a directory
            anyhow::bail!(
                "Output path for OEB must be a directory, found file: {:?}",
                output_path
            );
        }

        if !output_path.exists() {
            fs::create_dir_all(output_path).context("Failed to create OEB output directory")?;
        }

        // Delegate to OEBWriter
        let writer = OEBWriter::new();
        writer.write_book(book, output_path)?;

        Ok(())
    }
}
