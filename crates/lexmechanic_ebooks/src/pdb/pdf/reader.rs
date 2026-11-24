//! Port of `calibre.ebooks.pdb.pdf.reader`.
//!
//! The PDB payload here isn't a calibre-specific markup format at all --
//! it's a plain PDF file, split across PDB sections purely so it fits
//! the generic PDB container. `extract_content` just concatenates every
//! section back into one PDF and hands off to the real
//! `crate::input::pdf_input::PDFInput`, matching Python's
//! `plugin_for_input_format('pdf').convert(...)` delegation.
//!
//! `header`/`stream` mirror Python's `(header, stream, log, options)`
//! constructor contract (see `crate::pdb::formatreader`'s module docs
//! for why the constructor itself isn't part of the `FormatReader`
//! trait) -- sections are read eagerly in [`Reader::new`], matching the
//! convention `crate::pdb::ereader::reader132::Reader132::new`
//! established.

use crate::input::pdf_input::PDFInput;
use crate::pdb::formatreader::FormatReader;
use crate::pdb::header::PdbHeader;
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use std::path::Path;

pub struct Reader {
    pdf_bytes: Vec<u8>,
}

impl Reader {
    pub fn new<R: Read + Seek>(header: &PdbHeader, stream: &mut R) -> Result<Self> {
        let mut pdf_bytes = Vec::new();
        for i in 0..header.records.len() {
            pdf_bytes.extend_from_slice(&header.section_data(stream, i)?);
        }
        Ok(Reader { pdf_bytes })
    }
}

impl FormatReader for Reader {
    /// Port of `Reader.extract_content`: writes the reassembled PDF to a
    /// temp file, then delegates to [`PDFInput::convert`] for the real
    /// PDF-to-OEB conversion (matching Python's `pdf_plugin.convert`
    /// call). Python also copies any of the PDF input plugin's option
    /// defaults onto `self.options` that the caller didn't already set
    /// -- this crate's `PDFInput::convert` takes no options parameter at
    /// all (no plugin-options system exists yet in this crate), so
    /// there's nothing to copy.
    fn extract_content(&self, output_dir: &Path) -> Result<()> {
        let tmp = tempfile::NamedTempFile::new().context("creating temp PDF file")?;
        std::fs::write(&tmp, &self.pdf_bytes).context("writing reassembled PDF")?;

        let pdf_input = PDFInput::new();
        pdf_input
            .convert(tmp.path(), output_dir)
            .context("Failed to convert embedded PDF")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdb::header::PdbHeaderBuilder;
    use std::io::Cursor;

    fn build_pdb_wrapped_pdf(chunks: &[&[u8]]) -> Vec<u8> {
        let builder = PdbHeaderBuilder::new("PDF ", "wrapped");
        let lengths: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        let mut out = Vec::new();
        builder.build_header(&lengths, &mut out).unwrap();
        for chunk in chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    #[test]
    fn reassembles_sections_into_one_pdf_buffer() {
        let raw = build_pdb_wrapped_pdf(&[b"%PDF-1.4\n", b"1 0 obj\n", b"%%EOF"]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();

        let reader = Reader::new(&header, &mut cursor).unwrap();

        assert_eq!(reader.pdf_bytes, b"%PDF-1.4\n1 0 obj\n%%EOF");
    }

    #[test]
    fn extract_content_reports_a_clear_error_for_non_pdf_bytes() {
        let raw = build_pdb_wrapped_pdf(&[b"not actually a pdf"]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader::new(&header, &mut cursor).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let err = reader.extract_content(tmp.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("Failed to convert embedded PDF"),
            "{err:#}"
        );
    }

    #[test]
    fn handles_a_single_empty_section_without_panicking() {
        let raw = build_pdb_wrapped_pdf(&[b""]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader::new(&header, &mut cursor).unwrap();
        assert!(reader.pdf_bytes.is_empty());
    }
}
