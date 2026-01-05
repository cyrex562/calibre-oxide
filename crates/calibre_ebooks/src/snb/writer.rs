use anyhow::{bail, Result};
use std::io::{Seek, Write};

pub struct SnbWriter;

impl SnbWriter {
    pub fn new() -> Self {
        SnbWriter
    }

    pub fn write_dummy<W: Write + Seek>(&self, _writer: &mut W) -> Result<()> {
        // Full SNB writing involves VFAT and custom compression.
        // For now, this is a placeholder.
        bail!("SNB writing not fully implemented");
    }
}
