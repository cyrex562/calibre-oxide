use crate::oeb::book::OEBBook;
use crate::snb::writer::SnbWriter;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct SnbOutput;

impl SnbOutput {
    pub fn new() -> Self {
        SnbOutput
    }

    pub fn convert(&self, _book: &OEBBook, output_path: &Path) -> Result<()> {
        let file = File::create(output_path).context("Failed to create SNB file")?;
        let mut writer = BufWriter::new(file);
        let snb_writer = SnbWriter::new();

        // This will currently fail as not fully implemented
        snb_writer
            .write_dummy(&mut writer)
            .context("SNB export not fully implemented")?;

        Ok(())
    }
}
