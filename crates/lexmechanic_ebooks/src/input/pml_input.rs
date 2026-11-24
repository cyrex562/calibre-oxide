use crate::compression::palmdoc::decompress;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::pdb::reader::PdbReader;
use crate::pml::pmlconverter::pml_to_html;
use anyhow::{Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use html_escape::encode_text;
use std::fs;
use std::io::Cursor;
use std::path::Path;

pub struct PMLInput;

impl PMLInput {
    pub fn new() -> Self {
        PMLInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let mut reader = PdbReader::new(input_path).context("Failed to open PDB file")?;

        // 1. Read Payload
        // Rec 0 likely contains Compression header if "TEXt" type.
        // Standard PalmDoc: Rec 0 is header?
        // Or Rec 0 is TEXT?
        // PalmDoc spec: Rec 0 is header. Rec 1..N is text.

        let mut text_content = Vec::new();

        // Check compression
        let mut compression = 1; // 1 = None, 2 = PalmDoc

        if reader.num_records() > 0 {
            let rec0 = reader.read_record(0)?;
            if rec0.len() >= 2 {
                let mut curs = Cursor::new(&rec0);
                compression = curs.read_u16::<BigEndian>()?;
            }
        }

        // Records 1..N-1 usually (Last record might be bookmarks/metadata?)
        // Standard PalmDoc: All records after 0 are text until ...?
        // We'll iterate 1..N.

        for i in 1..reader.num_records() {
            let data = reader.read_record(i)?;

            // Check if it's a valid text record or auxiliary (bookmarks, etc)?
            // Usually text records are roughly 4096 bytes.
            // Let's try to decompress/read.

            let chunk = if compression == 2 {
                decompress(&data)?
            } else {
                data
            };

            text_content.extend_from_slice(&chunk);
        }

        let pml_text = String::from_utf8_lossy(&text_content).to_string();

        // 2. Parse PML
        let html_body = pml_to_html(&pml_text);

        // 3. Create OEBBook
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // Metadata
        let title = reader.header.name.clone();
        book.metadata.add("title", &title);
        book.metadata.add("language", "en"); // Default

        // Content
        let page_filename = "index.html";
        let full_html = format!(
            "<html><head><title>{}</title></head><body>{}</body></html>",
            encode_text(&title),
            html_body
        );

        // Write to output dir
        fs::write(output_dir.join(page_filename), full_html)?;

        book.manifest
            .add("content", page_filename, "application/xhtml+xml");
        book.spine.add("content", true);

        Ok(book)
    }
}

impl Default for PMLInput {
    fn default() -> Self {
        Self::new()
    }
}
