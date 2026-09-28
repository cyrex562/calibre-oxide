//! TCR input.
//!
//! This used to be a placeholder writing a page reading "TCR Content Not
//! Supported Yet" and returning it as the book -- while registered, so
//! `ebook-convert book.tcr out.epub` reported success and handed the user
//! that page.
//!
//! Its stated reason was that "full decompression is not implemented in
//! this phase". Decompression turned out to be the easy half of TCR: the
//! format is a dictionary substitution, and upstream's own `decompress` is
//! eighteen lines. It is now ported as
//! [`crate::compression::tcr::decompress`] and used here.
//!
//! Found while auditing the registered input plugins for placeholders
//! (#926, #940, #942 are the same pattern).

use crate::compression::tcr;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct TCRInput;

impl TCRInput {
    pub fn new() -> Self {
        TCRInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let compressed = fs::read(input_path).with_context(|| format!("reading {}", input_path.display()))?;
        let text = tcr::decompress(&compressed).with_context(|| format!("decompressing {}", input_path.display()))?;

        fs::create_dir_all(output_dir)?;

        // TCR holds plain text, so the same treatment `txt_input` gives a
        // `.txt`: the bytes are Latin-1 (the format predates Unicode and
        // has no encoding declaration), and paragraphs are blank-line
        // separated.
        let decoded: String = text.iter().map(|&b| b as char).collect();
        let title = input_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string());

        let mut html = String::from("<html><head><title>");
        html.push_str(&html_escape::encode_text(&title));
        html.push_str("</title></head><body>\n");
        for paragraph in decoded.split("\n\n") {
            let paragraph = paragraph.trim();
            if paragraph.is_empty() {
                continue;
            }
            html.push_str("<p>");
            html.push_str(&html_escape::encode_text(paragraph));
            html.push_str("</p>\n");
        }
        html.push_str("</body></html>");

        let content_filename = "index.html";
        fs::write(output_dir.join(content_filename), html.as_bytes())?;

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);
        book.metadata.add("title", &title);

        let id = "content".to_string();
        let href = content_filename.to_string();
        book.manifest.items.insert(id.clone(), ManifestItem::new(&id, &href, "application/xhtml+xml"));
        book.manifest.hrefs.insert(href, id.clone());
        book.spine.add(&id, true);

        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a real TCR file whose dictionary encodes `text` one byte per
    /// character, which is a valid if unoptimised TCR.
    fn write_tcr(path: &Path, text: &str) {
        let mut out = tcr::MAGIC.to_vec();
        // Entry i is the single byte i, so the body is the text itself.
        for i in 0..256u16 {
            out.push(1);
            out.push(i as u8);
        }
        out.extend_from_slice(text.as_bytes());
        fs::write(path, out).unwrap();
    }

    #[test]
    fn a_real_tcr_converts_to_html_with_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let book_path = dir.path().join("My Book.tcr");
        write_tcr(&book_path, "First paragraph.\n\nSecond paragraph.");
        let out = dir.path().join("out");

        let book = TCRInput::new().convert(&book_path, &out).unwrap();

        let html = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(html.contains("<p>First paragraph.</p>"), "{html}");
        assert!(html.contains("<p>Second paragraph.</p>"), "{html}");
        assert!(!html.contains("Not Supported"), "the placeholder is still being written:\n{html}");
        assert_eq!(book.spine.items.len(), 1);
    }

    /// The title comes from the filename -- TCR carries no metadata.
    #[test]
    fn the_title_comes_from_the_filename() {
        let dir = tempfile::tempdir().unwrap();
        let book_path = dir.path().join("Treasure Island.tcr");
        write_tcr(&book_path, "Text.");
        let out = dir.path().join("out");

        let book = TCRInput::new().convert(&book_path, &out).unwrap();
        assert_eq!(book.metadata.get("title").first().map(|i| i.value.as_str()), Some("Treasure Island"));
    }

    /// The change that matters: a file this cannot read is an error, not a
    /// success returning a page that says so.
    #[test]
    fn a_file_that_is_not_a_tcr_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("book.tcr");
        fs::write(&fake, b"not a TCR file").unwrap();
        let out = dir.path().join("out");

        assert!(TCRInput::new().convert(&fake, &out).is_err(), "a non-TCR must not convert to a placeholder page");
        assert!(!out.join("index.html").exists(), "nothing should have been written");
    }

    /// Markup characters in the text must not become markup.
    #[test]
    fn text_is_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let book_path = dir.path().join("book.tcr");
        write_tcr(&book_path, "5 < 6 & <script>alert(1)</script>");
        let out = dir.path().join("out");

        TCRInput::new().convert(&book_path, &out).unwrap();
        let html = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!html.contains("<script>"), "markup was not escaped:\n{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }
}
