//! Port of `calibre.ebooks.rb.writer.RBWriter`'s call site (the RB
//! output plugin's `convert`).

use crate::metadata::MetaInformation;
use crate::oeb::book::OEBBook;
use crate::rb::rbml::RbOptions;
use crate::rb::writer::RbWriter;
use anyhow::Result;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct RBOutput;

impl RBOutput {
    pub fn new() -> Self {
        RBOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let title = book
            .metadata
            .first("title")
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        let authors: Vec<String> = book
            .metadata
            .get("creator")
            .iter()
            .map(|i| i.value.clone())
            .collect();
        let mi = MetaInformation::new(&title, authors);

        let file = File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        RbWriter::new().write_content(book, &mut writer, Some(&mi), &RbOptions::default())?;

        Ok(())
    }
}

impl Default for RBOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::rb_input::RBInput;
    use crate::oeb::container::DirContainer;

    /// Cross-validation per the issue's definition of done: write a
    /// book with `RBOutput`, then read it back with `RBInput` (the
    /// same dispatcher a real `.rb` file goes through via
    /// `conversion::plumber`), and check the original text, an
    /// embedded image, and metadata all survive the round trip.
    #[test]
    fn write_then_read_round_trips_text_images_and_metadata() {
        let src_tmp = tempfile::tempdir().unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(src_tmp.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.manifest.add("cover", "cover.png", "image/png");
        book.spine.add("item1", true);
        let _ = book.container.write(
            "index.html",
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Round trip <b>RB</b> content</p></body></html>",
        );
        // A minimal valid 1x1 grayscale PNG.
        let png = one_pixel_png();
        let _ = book.container.write("cover.png", &png);

        book.metadata.add("title", "Round Trip Book");
        book.metadata.add("creator", "Round Trip Author");

        let out_path = src_tmp.path().join("book.rb");
        RBOutput::new().convert(&book, &out_path).unwrap();

        let extract_dir = tempfile::tempdir().unwrap();
        let read_back = RBInput::new()
            .convert(&out_path, extract_dir.path())
            .unwrap();

        assert_eq!(
            read_back.metadata.first("title").map(|i| i.value.clone()),
            Some("Round Trip Book".to_string())
        );
        assert_eq!(
            read_back.metadata.get("creator")[0].value,
            "Round Trip Author".to_string()
        );

        assert_eq!(read_back.spine.items.len(), 1);
        let page = read_back
            .manifest
            .get_by_id(&read_back.spine.items[0].idref)
            .unwrap();
        let html = read_back.container.read(&page.href).unwrap();
        let html = String::from_utf8_lossy(&html);
        assert!(html.contains("Round trip"), "{html}");
        assert!(html.contains("<B>RB</B>"), "{html}");

        // The cover image manifest-only (not spine), content preserved
        // byte-for-byte (images are stored raw, only re-encoded once by
        // `RbWriter::images`, then read back verbatim).
        let image_items: Vec<_> = read_back
            .manifest
            .iter()
            .filter(|i| i.media_type.starts_with("image/"))
            .collect();
        assert_eq!(image_items.len(), 1);
        let image_bytes = read_back.container.read(&image_items[0].href).unwrap();
        assert!(!image_bytes.is_empty());
        assert!(image_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    fn one_pixel_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([200, 100, 50]));
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut buf = std::io::Cursor::new(Vec::new());
        dynamic.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }
}
