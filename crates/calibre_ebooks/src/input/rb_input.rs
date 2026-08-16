//! Port of `calibre.ebooks.rb.reader.Reader`'s call site (the RB input
//! plugin's `convert`): read a RocketBook file into an [`OEBBook`],
//! following this crate's established input-plugin convention (see
//! e.g. `pml_input.rs`) of building the manifest/spine directly rather
//! than writing a literal `metadata.opf` file the way Python's
//! `OPFCreator`-based `Reader.create_opf` does.

use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::rb::reader::RbReader;
use anyhow::{Context, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;

pub struct RBInput;

impl RBInput {
    pub fn new() -> Self {
        RBInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = fs::File::open(input_path).context("Failed to open RB file")?;
        let mut rb_reader =
            RbReader::new(BufReader::new(file)).context("Failed to parse RB file")?;

        fs::create_dir_all(output_dir)?;

        // Port of `Reader.extract_content` + `Reader.create_opf`: for
        // each TOC entry whose name ends in "html", collect it as a
        // page; for each ending in "png", collect it as an image. Note
        // the real quirk this crate's own `RBWriter` produces in every
        // file: an `index.hidx` entry that matches neither suffix, so
        // it never surfaces here either -- see `rb::reader`'s docs.
        let (pages, images) = rb_reader
            .extract_content()
            .context("Failed to extract RB content")?;

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        book.metadata.add("title", &rb_reader.mi.title);
        for author in &rb_reader.mi.authors {
            book.metadata.add("creator", author);
        }
        for lang in &rb_reader.mi.languages {
            book.metadata.add("language", lang);
        }

        // Pages become both manifest entries and the spine, in TOC
        // order -- port of `create_opf`'s `create_manifest(pages+
        // images)` + `create_spine(pages)`.
        for (index, (name, content)) in pages.iter().enumerate() {
            book.container.write(name, content.as_bytes())?;
            let id = format!("page{index}");
            book.manifest.add(&id, name, "application/xhtml+xml");
            book.spine.add(&id, true);
        }

        // Images are manifest-only (never in the spine).
        for (index, (name, data)) in images.iter().enumerate() {
            book.container.write(name, data)?;
            let id = format!("image{index}");
            let media_type = mime_guess::from_path(name)
                .first()
                .map(|m| m.essence_str().to_string())
                .unwrap_or_else(|| "image/png".to_string());
            book.manifest.add(&id, name, &media_type);
        }

        Ok(book)
    }
}

impl Default for RBInput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rb::header::MAGIC;
    use crate::rb::rbml::RbOptions;
    use crate::rb::writer::RbWriter;
    use byteorder::{LittleEndian, WriteBytesExt};
    use std::io::Cursor;

    #[test]
    fn convert_reads_rb_metadata_pages_and_images() {
        use crate::metadata::MetaInformation;
        use crate::oeb::container::DirContainer as SrcDirContainer;

        // Build a small book with an image, write it out via `RbWriter`
        // to get a real `.rb` file (round-trip through the real writer
        // exercises far more than a hand-built fixture would).
        let src_tmp = tempfile::tempdir().unwrap();
        let mut book = OEBBook::new(Box::new(SrcDirContainer::new(src_tmp.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        let _ = book.container.write(
            "index.html",
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Input plugin text</p></body></html>",
        );

        let mi = MetaInformation::new("Input Plugin Title", vec!["Plugin Author".to_string()]);
        let mut out = Cursor::new(Vec::new());
        RbWriter::new()
            .write_content(&book, &mut out, Some(&mi), &RbOptions::default())
            .unwrap();

        let rb_path = src_tmp.path().join("book.rb");
        fs::write(&rb_path, out.into_inner()).unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let oeb = RBInput::new().convert(&rb_path, out_dir.path()).unwrap();

        assert_eq!(
            oeb.metadata.first("title").map(|i| i.value.clone()),
            Some("Input Plugin Title".to_string())
        );
        assert_eq!(
            oeb.metadata.get("creator")[0].value,
            "Plugin Author".to_string()
        );

        assert_eq!(oeb.spine.items.len(), 1);
        let page_item = oeb.manifest.get_by_id(&oeb.spine.items[0].idref).unwrap();
        let html = oeb.container.read(&page_item.href).unwrap();
        assert!(
            String::from_utf8_lossy(&html).contains("Input plugin text"),
            "{}",
            String::from_utf8_lossy(&html)
        );
    }

    /// Confirms `RbHeader::parse`'s new size-verification rejects a
    /// file whose header lies about its own length, end to end through
    /// the input plugin (not just the header unit tests).
    #[test]
    fn convert_rejects_a_size_mismatched_file() {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC); // 14 bytes -> pos 14
        buf.extend_from_slice(&[0u8; 14]); // pos 28 (fills the u32+u16+u32 gap)
        buf.write_u32::<LittleEndian>(999999).unwrap(); // byte 28: a size that doesn't match the real (32-byte) file
        let tmp = tempfile::tempdir().unwrap();
        let rb_path = tmp.path().join("bad.rb");
        fs::write(&rb_path, &buf).unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        // `OEBBook` doesn't implement `Debug`, so `Result::unwrap_err`
        // (which requires the `Ok` type to) can't be used here.
        let err = match RBInput::new().convert(&rb_path, out_dir.path()) {
            Ok(_) => panic!("expected a size-mismatch error"),
            Err(err) => err,
        };
        let found_corrupt = err
            .chain()
            .any(|cause| cause.to_string().to_lowercase().contains("corrupt"));
        assert!(found_corrupt, "{err:?}");
    }
}
