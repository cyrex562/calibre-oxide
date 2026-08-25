use crate::docx::container::Docx;
use crate::docx::to_html::convert_docx_document;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::reader::OEBReader;
use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;

pub struct DOCXInput;

impl DOCXInput {
    pub fn new() -> Self {
        DOCXInput
    }

    /// Port of `docx_input.py`'s `DOCXInput.convert`, which just calls
    /// `Convert(stream, ...)()` -- `Convert.__call__()`'s own return
    /// value (an OPF path, since `__call__` calls `self.write(doc)`
    /// as its own last line) is what calibre's general `input_to_oeb`
    /// pipeline reads to build the finished book. This mirrors that:
    /// [`convert_docx_document`] writes a real OPF/NCX/HTML package
    /// straight into `output_dir` (matching [`crate::input::epub_input::EPUBInput`]'s
    /// own container-rooted-at-`output_dir` pattern, not a nested
    /// subdirectory), and [`OEBReader::read_opf`] reads it back into
    /// an [`OEBBook`] -- the same general OPF-based book loader
    /// `EPUBInput` already uses, not a docx-specific one.
    ///
    /// Replaces the previous placeholder implementation, which called
    /// the (now-removed) provisional `DOCXToHTML::convert` sketch and
    /// delegated to `HTMLInput::convert` -- a path that produced a
    /// nearly-empty `OEBBook` (`HTMLInput`'s own metadata step just
    /// hardcodes `dc:title` to `"Converted Log"`) and never saw the
    /// document's own real title/authors/manifest/spine/guide at all.
    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = File::open(input_path).context("Failed to open DOCX file")?;
        let mut docx = Docx::new(file).map_err(|e| anyhow::anyhow!("DOCX Error: {}", e))?;

        std::fs::create_dir_all(output_dir)?;
        convert_docx_document(&mut docx, output_dir, true, "Notes")
            .map_err(|e| anyhow::anyhow!("Conversion Error: {}", e))?;

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);
        OEBReader::new().read_opf(&mut book, "metadata.opf")?;

        Ok(book)
    }
}
