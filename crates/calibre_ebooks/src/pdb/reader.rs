use crate::pdb::header::PdbHeader;
use anyhow::{bail, Result};
use std::fs::File;
use std::path::Path;

pub struct PdbReader {
    pub header: PdbHeader,
    file: File,
}

impl PdbReader {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut file = File::open(path)?;
        let header = PdbHeader::parse(&mut file)?;

        Ok(Self { header, file })
    }

    pub fn read_record(&mut self, index: usize) -> Result<Vec<u8>> {
        self.header.section_data(&mut self.file, index)
    }

    pub fn num_records(&self) -> usize {
        self.header.records.len()
    }
}
