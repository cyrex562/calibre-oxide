//! `.pdb` input: dispatches on the container's identity (#926).
//!
//! A Palm database is a **container**, not a format — the same extension
//! carries PalmDoc, eReader/PML, zTXT, Plucker, Haodoo and embedded PDF,
//! distinguished only by the 8-byte type+creator pair in the header. So
//! this reads that pair and routes, the way
//! `calibre.ebooks.pdb.input.PDBInput` does through `get_reader`.
//!
//! It previously did not look at the identity at all: it wrote every
//! record out as `record_N.bin` and built an HTML page listing them with
//! hex previews. That produced no error, so a user converting a Palm book
//! got a page of byte counts and no indication anything had gone wrong —
//! and the real PML converter sat unregistered beside it. Silent wrong
//! output is worse than a refusal, which is why an unsupported identity
//! is now an error that names the format.
//!
//! eReader/PML and embedded PDF are dispatched to real readers. zTXT,
//! Plucker and Haodoo are recognised and refused by name -- no reader for
//! them exists in this tree yet, and saying so beats guessing.

use crate::oeb::book::OEBBook;
use crate::pdb::identity::{format_for, identity_of, PdbFormat};
use crate::pdb::reader::PdbReader;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub struct PDBInput;

impl PDBInput {
    pub fn new() -> Self {
        PDBInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        std::fs::create_dir_all(output_dir)?;

        let identity = {
            let reader = PdbReader::new(input_path).context("Failed to open PDB")?;
            identity_of(&reader.header)
        };

        match format_for(&identity) {
            // eReader's text is PML, which is what `PMLInput` reads. It
            // stays a separate module rather than being inlined, because
            // it is a real format reader in its own right; this is the
            // dispatch upstream performs through `FORMAT_READERS`.
            Some(PdbFormat::EReader) => crate::input::pml_input::PMLInput::new().convert(input_path, output_dir),

            // A `.pdb` can carry a whole PDF. The records concatenate
            // back into one, which the real PDF input then handles.
            // Reassembled here rather than through `pdb::pdf::Reader`
            // because that type's `extract_content` discards the
            // `OEBBook` its own `PDFInput` call produces, and this
            // function has to return one.
            Some(PdbFormat::Pdf) => {
                let mut file = std::fs::File::open(input_path)?;
                let header = crate::pdb::header::PdbHeader::parse(&mut file)?;
                let mut pdf_bytes = Vec::new();
                for i in 0..header.records.len() {
                    pdf_bytes.extend_from_slice(&header.section_data(&mut file, i)?);
                }
                let staging = tempfile::Builder::new().suffix(".pdf").tempfile()?;
                std::fs::write(staging.path(), &pdf_bytes).context("writing the reassembled PDF")?;
                crate::input::pdf_input::PDFInput::new().convert(staging.path(), output_dir).context("converting the PDF carried inside the Palm database")
            }

            // Named individually rather than lumped into one message: a
            // person who knows their book is a zTXT can tell that this
            // recognised the format and has not implemented it, which is
            // a different problem from a corrupt file.
            Some(other) => bail!(
                "this is a {} Palm database ({identity}), which is recognised but not yet supported — eReader/PML and embedded-PDF .pdb files convert today",
                other.name()
            ),

            None => bail!(
                "{} is not a Palm database this can read: its type/creator identity is {identity:?}, which matches no known format",
                input_path.display()
            ),
        }
    }
}

impl Default for PDBInput {
    fn default() -> Self {
        Self::new()
    }
}
