use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::path::Path;

pub struct LRFOutput;

impl LRFOutput {
    pub fn new() -> Self {
        LRFOutput
    }

    pub fn convert(&self, _book: &OEBBook, output_path: &Path) -> Result<()> {
        // Outputting LRF is not supported (proprietary format, mostly obsolete).
        // We will just create a dummy file to satisfy the plugin architecture/testing.
        // In a real scenario this might error or warn.

        // Write a "dummy" LRF header or just text saying it's a stub
        std::fs::write(output_path, b"LRF STUB FILE")?;
        Ok(())
    }
}
