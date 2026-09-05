//! Port of `calibre.ebooks.conversion.plugins.djvu_input.DJVUInput`
//! (issue #129).
//!
//! Extracts the DjVu file's OCR text layer via [`DjvuFile::text`],
//! HTML-izes it the same way `txt_input`/the plain-text pipeline does
//! (`txt::processor::convert_basic`), then feeds the result through
//! the real [`HTMLInput`] to build the OEB -- matching upstream's own
//! four steps exactly (extract text, HTML-ize, run through the HTML
//! input plugin, set metadata).
//!
//! # Disclosed narrowings
//!
//! - Real upstream's own "set metadata from file" step
//!   (`get_file_type_metadata`/`meta_info_to_oeb_metadata`) resolves
//!   to a real no-op for DjVu in real calibre too -- there is no
//!   `calibre.ebooks.metadata.djvu` reader plugin registered for this
//!   format upstream, so `get_file_type_metadata` always returns an
//!   empty `MetaInformation(None, None)` for a `.djvu` file. This port
//!   still sets a real title (derived from the input filename) after
//!   the `HTMLInput` call, purely because `HTMLInput::convert`'s own
//!   title is currently a hardcoded, unrelated placeholder
//!   (`"Converted Log"`) rather than anything HTML-derived -- fixing
//!   *that* placeholder is `html_input.rs`'s own separate, pre-existing
//!   gap, not part of this issue.
use crate::djvu::file::DjvuFile;
use crate::input::html_input::HTMLInput;
use crate::oeb::book::OEBBook;
use crate::txt::processor::convert_basic;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

pub struct DJVUInput;

impl DJVUInput {
    pub fn new() -> Self {
        DJVUInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let djvu = DjvuFile::open(input_path).with_context(|| format!("Failed to parse DjVu file {input_path:?}"))?;
        let raw_text = djvu.text().context("Failed to extract the DjVu text layer")?;
        if raw_text.is_empty() {
            bail!(
                "The DJVU file contains no text, only images, probably page scans. \
                 calibre only supports conversion of DJVU files with actual text in them."
            );
        }

        // Port of `raw_text.replace(b'\n', b' ').replace(b'\037', b'\n\n')`
        // -- DjVu text records are `0x1f`-separated; turning each
        // separator into a blank line is what makes `convert_basic`
        // treat them as separate paragraphs.
        let mut normalized = Vec::with_capacity(raw_text.len());
        for &b in &raw_text {
            match b {
                b'\n' => normalized.push(b' '),
                0x1f => normalized.extend_from_slice(b"\n\n"),
                other => normalized.push(other),
            }
        }
        let text = String::from_utf8_lossy(&normalized);
        let html = convert_basic(&text, "", 0);

        fs::create_dir_all(output_dir).context("Failed to create output directory")?;

        // Write the HTML-ized text somewhere HTMLInput can crawl from,
        // outside `output_dir` so it doesn't get swept up as a stray
        // extra manifest entry alongside whatever HTMLInput copies in.
        let temp_dir = tempfile::tempdir().context("Failed to create a temporary directory")?;
        let temp_html_path = temp_dir.path().join("index.html");
        fs::write(&temp_html_path, &html).context("Failed to write intermediate HTML")?;

        let mut book = HTMLInput::new().convert(&temp_html_path, output_dir)?;

        // See this module's own doc for why a real title is set here
        // despite real upstream's own metadata step being a practical
        // no-op for this format.
        let title = input_path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Untitled".to_string());
        book.metadata.clear("title");
        book.metadata.clear("dc:title");
        book.metadata.add("title", &title);

        Ok(book)
    }
}

impl Default for DJVUInput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same real chunk-building recipe as `djvu::file`'s own test
    // module (its helpers are private to that module, so this is a
    // deliberate, exact duplicate rather than a guess at the format).
    const MAGIC: &[u8; 4] = b"AT&T";

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn text_payload(text: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(text.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(text);
        out
    }

    fn djvu_with_text(pages: &[&[u8]]) -> Vec<u8> {
        let mut form_body = b"DJVU".to_vec();
        for page in pages {
            form_body.extend_from_slice(&chunk(b"TXTa", &text_payload(page)));
        }
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&chunk(b"FORM", &form_body));
        out
    }

    fn djvu_with_no_text() -> Vec<u8> {
        let mut form_body = b"DJVU".to_vec();
        form_body.extend_from_slice(&chunk(b"Sjbz", &[0xde, 0xad, 0xbe, 0xef]));
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&chunk(b"FORM", &form_body));
        out
    }

    #[test]
    fn a_djvu_file_with_no_text_is_a_real_error() {
        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("scan.djvu");
        fs::write(&input_path, djvu_with_no_text()).unwrap();
        let output_dir = temp.path().join("out");

        // `OEBBook` doesn't derive `Debug`, so `unwrap_err()` (which
        // would print the Ok value in its panic message) can't be
        // used here -- match instead.
        let result = DJVUInput::new().convert(&input_path, &output_dir);
        let Err(err) = result else { panic!("expected a real error for a text-less DjVu file") };
        assert!(err.to_string().contains("no text"), "{err}");
    }

    #[test]
    fn real_ocr_text_is_extracted_and_becomes_real_html_paragraphs() {
        let buf = djvu_with_text(&[b"Hello world", b"Second paragraph"]);
        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("book.djvu");
        fs::write(&input_path, &buf).unwrap();
        let output_dir = temp.path().join("out");

        let book = DJVUInput::new().convert(&input_path, &output_dir).expect("conversion failed");
        assert!(!book.manifest.items.is_empty());
        let titles = book.metadata.get("title");
        assert_eq!(titles.len(), 1, "no duplicate/conflicting title entries");
        assert_eq!(titles[0].value, "book");

        // Read back whatever HTMLInput copied into the manifest and
        // confirm the real OCR text survived the whole pipeline.
        let mut found_text = false;
        for item in book.manifest.items.values() {
            let path = output_dir.join(&item.href);
            if let Ok(content) = fs::read_to_string(&path) {
                if content.contains("Hello world") && content.contains("Second paragraph") {
                    found_text = true;
                }
            }
        }
        assert!(found_text, "the DjVu text layer should survive through to the HTML output");
    }
}
