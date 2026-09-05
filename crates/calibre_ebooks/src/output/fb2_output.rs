//! Port of `calibre.ebooks.conversion.plugins.fb2_output.FB2Output`'s
//! call site (issue #457).
//!
//! Rewired to drive the real [`Fb2Mlizer`] port instead of its own
//! crude regex-based HTML-to-FB2 substitution (`<img>`->`<image>`,
//! `<br>`->`<empty-line/>`, `<div>`->`<p>` over raw strings) -- the
//! same rewiring pattern already established for `output::rtf_output`
//! (issue #50): real markup comes from walking the XHTML tree with a
//! [`TagStylizer`], images are converted/embedded for real via
//! [`DefaultImageConverter`], and metadata is read directly from
//! `book.metadata` inside `Fb2Mlizer::extract_content` itself.
//!
//! `date`/`uuid_fallback` are synthesized here (real upstream reads
//! the system clock and generates a UUID at the same call site) --
//! `Fb2Mlizer::extract_content` takes them as parameters instead of
//! reading the clock itself specifically so its own tests can pass
//! fixed values for reproducibility; this is the one real call site
//! that needs a genuine, non-deterministic value.
//!
//! Disclosed narrowing, matching every other output plugin in this
//! crate (e.g. `rtf_output`): upstream's own `FB2Output.convert` also
//! runs `SVGRasterizer`/`linearize_jacket` transforms on the OEB book
//! before calling `FB2MLizer.extract_content` -- neither is wired at
//! the output-plugin level anywhere in this crate yet, a pre-existing
//! gap shared by every other output plugin, not new to this fix.

use crate::fb2::fb2ml::{DefaultImageConverter, Fb2Mlizer, Fb2Options};
use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::TagStylizer;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct FB2Output;

impl FB2Output {
    pub fn new() -> Self {
        FB2Output
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let uuid_fallback = uuid::Uuid::new_v4().to_string();

        let mut mlizer = Fb2Mlizer::new();
        let fb2 = mlizer.extract_content(book, &Fb2Options::default(), &TagStylizer, &DefaultImageConverter, &date, &uuid_fallback);

        let mut file = File::create(output_path).context("Failed to create output FB2 file")?;
        file.write_all(fb2.as_bytes()).context("Failed to write FB2 file")?;
        Ok(())
    }
}

impl Default for FB2Output {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        image::DynamicImage::new_rgb8(w, h).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageOutputFormat::Png).unwrap();
        buf
    }

    #[test]
    fn convert_drives_the_real_fb2mlizer_with_real_metadata_and_markup() {
        let tmp_source = tempfile::tempdir().unwrap();
        let source_path = tmp_source.path();
        std::fs::write(source_path.join("ch1.html"), r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1><p>Text</p></body></html>"#).unwrap();

        let container = Box::new(DirContainer::new(source_path));
        let mut book = OEBBook::new(container);
        book.manifest.add("ch1", "ch1.html", "application/xhtml+xml");
        book.spine.add("ch1", true);
        book.metadata.add("title", "FB2 Test");
        book.metadata.add("creator", "Author Name");

        let tmp_out = tempfile::tempdir().unwrap();
        let output_path = tmp_out.path().join("book.fb2");

        FB2Output::new().convert(&book, &output_path).expect("conversion failed");

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("<book-title>FB2 Test</book-title>"), "{content}");
        // Real Fb2Mlizer splits a "First Last" creator into separate
        // first-name/last-name elements, unlike the old ad-hoc
        // converter's single <first-name>Author Name</first-name>.
        assert!(content.contains("<first-name>Author</first-name><last-name>Name</last-name>"), "{content}");
        assert!(content.contains("<section"), "{content}");
        assert!(content.contains("<p>Text</p>"), "{content}");
        // Real Fb2Mlizer structure the old ad-hoc converter never
        // produced: a real FictionBook wrapper and document info.
        assert!(content.starts_with("<?xml"), "{content}");
        assert!(content.contains("<FictionBook"), "{content}");
        assert!(content.contains("<document-info>"), "{content}");
    }

    /// A real (not fake-bytes) image round-trips through the actual
    /// `DefaultImageConverter` -- the old ad-hoc converter never
    /// validated or transcoded image data at all.
    #[test]
    fn convert_embeds_a_real_image_via_the_real_image_converter() {
        let tmp_source = tempfile::tempdir().unwrap();
        let source_path = tmp_source.path();
        std::fs::write(source_path.join("ch1.html"), r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="image.png"/></p></body></html>"#).unwrap();
        std::fs::write(source_path.join("image.png"), png_bytes(2, 2)).unwrap();

        let container = Box::new(DirContainer::new(source_path));
        let mut book = OEBBook::new(container);
        book.manifest.add("ch1", "ch1.html", "application/xhtml+xml");
        book.manifest.add("img1", "image.png", "image/png");
        book.spine.add("ch1", true);
        book.metadata.add("title", "FB2 Image Test");

        let tmp_out = tempfile::tempdir().unwrap();
        let output_path = tmp_out.path().join("book.fb2");
        FB2Output::new().convert(&book, &output_path).expect("conversion failed");

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("<image l:href=\"#img_0\"/>"), "{content}");
        // A native PNG passes through `DefaultImageConverter` unchanged
        // (only non-native formats are actually transcoded to JPEG).
        assert!(content.contains("<binary id=\"img_0\" content-type=\"image/png\">"), "{content}");
    }
}
