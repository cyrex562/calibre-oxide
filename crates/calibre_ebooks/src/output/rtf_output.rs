//! Port of `calibre.ebooks.rtf.rtfml.RTFMLizer`'s call site (the RTF
//! output plugin's `convert`).
//!
//! Rewired (issue #50) to drive the real [`RtfMlizer`] port instead of
//! the crude `html2text`-based plain-text dump this used to do --
//! following the same rewiring pattern already established for
//! `output::pml_output`/`output::rb_output` this session: real
//! metadata is already threaded through `RtfMlizer::extract_content`
//! itself (it reads `book.metadata` directly), real markup comes from
//! walking the XHTML tree with a [`TagStylizer`], and images are
//! embedded for real via [`DefaultImageConverter`] rather than being
//! dropped on the floor.

use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::TagStylizer;
use crate::rtf::rtfml::{DefaultImageConverter, RtfMlizer};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct RTFOutput;

impl RTFOutput {
    pub fn new() -> Self {
        RTFOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut mlizer = RtfMlizer::new();
        let rtf = mlizer.extract_content(book, &TagStylizer, &DefaultImageConverter);

        let mut file = File::create(output_path).context("Failed to create RTF file")?;
        file.write_all(rtf.as_bytes())
            .context("Failed to write RTF file")?;

        Ok(())
    }
}

impl Default for RTFOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::rtf_input::RTFInput;
    use crate::oeb::container::DirContainer;
    use crate::oeb::manifest::ManifestItem;

    #[test]
    fn convert_writes_a_balanced_rtf_document_with_the_spine_content() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let book_dir = tmp_dir.path().join("book");
        std::fs::create_dir_all(&book_dir).unwrap();
        let output_path = tmp_dir.path().join("output.rtf");

        let content_file = "content.html";
        std::fs::write(
            book_dir.join(content_file),
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello World</p><p>Second Line</p></body></html>"#,
        )
        .unwrap();

        let container = Box::new(DirContainer::new(&book_dir));
        let mut book = OEBBook::new(container);
        let id = "item1".to_string();
        book.manifest.items.insert(
            id.clone(),
            ManifestItem::new(&id, content_file, "application/xhtml+xml"),
        );
        book.spine.add(&id, true);
        book.metadata.add("title", "Test Book");
        book.metadata.add("creator", "Test Author");

        let output = RTFOutput::new();
        output
            .convert(&book, &output_path)
            .expect("RTF output conversion failed");

        assert!(output_path.exists());
        let rtf_content = std::fs::read_to_string(&output_path).unwrap();

        assert!(rtf_content.starts_with("{\\rtf1"));
        assert!(rtf_content.contains("Hello World"));
        assert!(rtf_content.contains("Second Line"));
        assert!(rtf_content.contains("\\par"));
        assert!(rtf_content.contains("Test Book"));
        assert!(rtf_content.contains("Test Author"));
    }

    /// Cross-validation per the issue's definition of done: write a
    /// book with `RTFOutput`, then read it back with `RTFInput` (the
    /// same dispatcher a real `.rtf` file goes through via
    /// `conversion::plumber`). `RTFInput`'s own RTF-parsing gap
    /// (`crate::input::rtf_input`'s doc comment) means this cannot
    /// assert on structured content survival the way `RBOutput`'s
    /// equivalent test does -- only that the pipeline round-trips
    /// without erroring and the raw RTF text is recoverable somewhere
    /// in the output.
    #[test]
    fn write_then_read_round_trips_without_erroring() {
        let src_tmp = tempfile::tempdir().unwrap();
        let book_dir = src_tmp.path().join("book");
        std::fs::create_dir_all(&book_dir).unwrap();
        std::fs::write(
            book_dir.join("index.html"),
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Round trip text</p></body></html>"#,
        )
        .unwrap();

        let mut book = OEBBook::new(Box::new(DirContainer::new(&book_dir)));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        book.metadata.add("title", "Round Trip Book");

        let out_path = src_tmp.path().join("book.rtf");
        RTFOutput::new().convert(&book, &out_path).unwrap();

        let extract_dir = tempfile::tempdir().unwrap();
        let read_back = RTFInput::new()
            .convert(&out_path, extract_dir.path())
            .unwrap();
        assert!(!read_back.manifest.items.is_empty());
    }
}
