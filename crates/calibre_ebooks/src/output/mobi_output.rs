use crate::mobi::writer::MobiWriter;
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct MOBIOutput;

impl MOBIOutput {
    pub fn new() -> Self {
        MOBIOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let file = File::create(output_path).context("Failed to create output MOBI file")?;
        let mut writer = BufWriter::new(file);

        let mobi_writer = MobiWriter::new();
        mobi_writer.write(book, &mut writer)?;

        Ok(())
    }
}
