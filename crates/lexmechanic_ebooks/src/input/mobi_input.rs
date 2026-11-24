//! Port of `calibre.ebooks.conversion.plugins.mobi_input.MOBIInput`.
//!
//! Reads a `.mobi`/`.prc`/`.azw`/`.azw3` file and produces an [`OEBBook`]:
//! [`crate::mobi::mobi6::MobiReader`] handles the MOBI6 case
//! (`kf8_type().is_none()`) directly; when the file carries a KF8 payload
//! (`standalone` or `joint` with a MOBI6 fallback), extraction is handed
//! off to [`crate::mobi::mobi8::Mobi8Reader`], matching Python's dispatch
//! in `MOBIInput.convert`.

use crate::mobi::mobi6::MobiReader;
use crate::mobi::mobi8::Mobi8Reader;
use crate::mobi::MobiLog;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::reader::OEBReader;
use anyhow::{Context, Result};
use std::path::Path;

pub struct MOBIInput;

impl Default for MOBIInput {
    fn default() -> Self {
        Self::new()
    }
}

impl MOBIInput {
    pub fn new() -> Self {
        MOBIInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        std::fs::create_dir_all(output_dir)?;
        let raw = std::fs::read(input_path)
            .with_context(|| format!("Failed to read {:?}", input_path))?;

        let mut reader = MobiReader::new(&raw).context("Failed to parse MOBI headers")?;
        let is_kf8 = reader.kf8_type.is_some();

        let opf_path = if is_kf8 {
            let log = MobiLog::default();
            let mut mobi8 = Mobi8Reader::new(reader, log, false);
            mobi8
                .run(output_dir)
                .context("Failed to extract KF8 content")?;
            output_dir.join("metadata.opf")
        } else {
            reader
                .extract_content(output_dir)
                .context("Failed to extract MOBI6 content")?;
            reader
                .created_opf_path
                .clone()
                .unwrap_or_else(|| output_dir.join("index.opf"))
        };

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);
        let oeb_reader = OEBReader::new();
        let rel_opf = opf_path.strip_prefix(output_dir).unwrap_or(&opf_path);
        let opf_str = rel_opf.to_str().context("Non-UTF8 OPF path")?;
        oeb_reader.read_opf(&mut book, opf_str)?;

        Ok(book)
    }
}
