use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::TagStylizer;
use crate::pdb::writer::PdbWriter;
use crate::pml::pmlml::{PmlMlizer, PmlOptions};
use anyhow::Result;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct PMLOutput;

impl PMLOutput {
    pub fn new() -> Self {
        PMLOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let title = book
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or("Unknown".to_string());

        // `PmlMlizer::extract_content` walks the whole book (cover,
        // spine, image/anchor bookkeeping) in one pass -- see
        // `crate::pml::pmlml` for why that (issue #47's real
        // `PMLMLizer` port) replaced the narrow per-spine-item
        // `html_to_pml` this used to call.
        let mut mlizer = PmlMlizer::new();
        let full_pml = mlizer.extract_content(book, &PmlOptions::default(), &TagStylizer);

        // Write PDB
        let file = File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        let pdb_writer = PdbWriter::new();

        // Encode as cp1252, the legacy Palm/PML text encoding.
        let (cow, _encoding, _malformed) = encoding_rs::WINDOWS_1252.encode(&full_pml);
        let encoded_bytes = cow.to_vec();

        pdb_writer.write(&title, &encoded_bytes, &mut writer)?;

        Ok(())
    }
}

impl Default for PMLOutput {
    fn default() -> Self {
        Self::new()
    }
}
