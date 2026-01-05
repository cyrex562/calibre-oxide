use crate::input::pdf_input::PDFInput;
use crate::oeb::book::OEBBook;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{Read, Seek};
use std::path::Path;
use tempfile::NamedTempFile;

pub struct AZW4Input;

impl AZW4Input {
    pub fn new() -> Self {
        AZW4Input
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // AZW4 is PDF wrapped in a PDB container.
        let mut file = fs::File::open(input_path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let pdf_start_sig = b"%PDF";
        let pdf_end_sig = b"%%EOF";

        let start = buffer
            .windows(pdf_start_sig.len())
            .position(|window| window == pdf_start_sig);

        if let Some(start_idx) = start {
            let end = buffer
                .windows(pdf_end_sig.len())
                .rposition(|window| window == pdf_end_sig);

            if let Some(mut end_idx) = end {
                end_idx += pdf_end_sig.len();
                if end_idx > start_idx {
                    let pdf_data = &buffer[start_idx..end_idx];

                    // Create a temporary PDF file to pass to PDFInput
                    // (PDFInput takes a Path, so we need a file)
                    let temp_pdf = NamedTempFile::new()?;
                    fs::write(&temp_pdf, pdf_data)?;

                    let pdf_input = PDFInput::new();
                    // We can reuse the output dir
                    let mut book = pdf_input
                        .convert(temp_pdf.path(), output_dir)
                        .context("Failed to convert embedded PDF")?;

                    // Specific AZW4 Update title if needed?
                    // PDFInput likely did a good job if metadata was inside the PDF.
                    // If AZW4 wrapper had metadata, we might want to read it from PDB headers,
                    // but extraction via PDF is the robust fallback.

                    return Ok(book);
                }
            }
        }

        bail!("No embedded PDF found in AZW4 container")
    }
}
