use crate::mobi::writer2::main::{MobiWriter, MobiWriterOpts};
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct MOBIOutput;

impl Default for MOBIOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl MOBIOutput {
    pub fn new() -> Self {
        MOBIOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut writer = MobiWriter::new(MobiWriterOpts::default());
        let bytes = writer
            .write(book)
            .context("Failed to encode MOBI content")?;

        let mut file = File::create(output_path).context("Failed to create output MOBI file")?;
        file.write_all(&bytes)
            .context("Failed to write MOBI content")?;

        Ok(())
    }
}
