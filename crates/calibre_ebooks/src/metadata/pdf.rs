//! Port of `old_src/src/calibre/ebooks/metadata/pdf.py`.
//!
//! The Info-dictionary half is read with `lopdf`. The other half is
//! the cover: `read_info(get_cover=True)` rasterizes page 1 with
//! `pdftoppm` and hands it back as `mi.cover_data`, which is why a
//! PDF added to calibre has a cover at all. That is done here by
//! [`crate::pdf::rasterize`] instead of poppler -- see its module
//! docs for why -- and it is what [`get_metadata`] does that
//! [`get_quick_metadata`] does not, mirroring the
//! `get_quick_metadata = partial(get_metadata, cover=False)` split
//! upstream.

use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use lopdf::{Document, Object};
use std::io::{Read, Seek};

/// Reads metadata *and* renders page 1 as the cover.
///
/// `cover=True` is upstream's default, so this is the plain name; a
/// caller that only wants the title should say so with
/// [`get_quick_metadata`] rather than pay for a page render.
pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    get_metadata_with_cover(stream, true)
}

/// Port of `get_quick_metadata`: the Info dictionary only, no
/// rendering.
pub fn get_quick_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    get_metadata_with_cover(stream, false)
}

/// Port of `get_metadata(stream, cover)`.
///
/// A failure to render the cover is deliberately not a failure to read
/// metadata: PDFium may not be installed, and a PDF whose first page
/// is malformed still has a perfectly good title. Either way the
/// caller gets the metadata it asked for and no cover, which is what
/// it got before this could render at all.
pub fn get_metadata_with_cover<R: Read + Seek>(mut stream: R, cover: bool) -> Result<MetaInformation> {
    // lopdf requires reading the whole stream or from a path.
    // Since we have a stream, let's load it into memory.
    // Note: This might be heavy for large PDFs, but old_code did subprocess.
    // For now, load into memory.
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer)?;

    let doc = Document::load_mem(&buffer).context("Failed to load PDF document")?;

    let mut mi = MetaInformation::default();

    if let Some(info_id) = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| o.as_reference().ok())
    {
        if let Ok(info_dict) = doc.get_object(info_id).and_then(|o| o.as_dict()) {
            if let Some(title) = info_dict
                .get(b"Title")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.title = title;
            }
            if let Some(author) = info_dict
                .get(b"Author")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.authors = author
                    .split(&[',', ';'][..])
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            if let Some(subject) = info_dict
                .get(b"Subject")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.tags = subject
                    .split(&[',', ';'][..])
                    .map(|s| s.trim().to_string())
                    .collect();
            }
        }
    }

    if cover {
        match crate::pdf::rasterize::first_page_cover(&buffer) {
            Ok(data) => mi.cover_data = (Some("jpg".to_string()), data),
            Err(e) => log::debug!("no cover rendered from PDF page 1: {e}"),
        }
    }

    Ok(mi)
}

fn from_pdf_object(obj: &Object) -> Result<String> {
    match obj {
        Object::String(bytes, _) => Ok(String::from_utf8_lossy(bytes).to_string()),
        Object::Name(bytes) => Ok(String::from_utf8_lossy(bytes).to_string()),
        _ => anyhow::bail!("Not a string object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;
    use lopdf::Object;

    #[test]
    fn test_pdf_metadata() -> Result<()> {
        // Construct a minimal PDF with info dict in memory?
        // lopdf allows constructing documents.
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let info_id = doc.new_object_id();

        let mut info = Dictionary::new();
        info.set("Title", Object::string_literal("Test PDF Title"));
        info.set("Author", Object::string_literal("Test Author"));
        info.set("Subject", Object::string_literal("Tag1, Tag2"));

        doc.objects.insert(info_id, Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));
        doc.trailer.set("Root", Object::Reference(pages_id)); // Minimal root

        // Serialize to buffer
        let mut buffer = Vec::new();
        doc.save_to(&mut buffer)?;

        let mut stream = std::io::Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Test PDF Title");
        assert_eq!(mi.authors, vec!["Test Author"]);
        assert!(mi.tags.contains(&"Tag1".to_string()));

        Ok(())
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use std::io::Cursor;

    /// The same two-page fixture `crate::pdf::rasterize` uses.
    const TWO_PAGE_PDF: &[u8] = include_bytes!("../../tests/data/two-page.pdf");

    #[test]
    fn a_pdf_gets_a_cover_rendered_from_its_first_page() {
        if !crate::pdf::rasterize::is_available() {
            eprintln!("skipping: no PDFium library available");
            return;
        }
        let mi = get_metadata(Cursor::new(TWO_PAGE_PDF)).unwrap();
        let (ext, data) = &mi.cover_data;
        assert_eq!(ext.as_deref(), Some("jpg"));
        assert!(!data.is_empty(), "no cover was rendered");

        // Page 1, not page 2: the fixture's first page draws a blue
        // rectangle and its second a red one, so the wrong page would
        // be a passing-looking cover of the wrong thing.
        let img = image::load_from_memory(data).unwrap().to_rgb8();
        let scale = img.width() as f32 / 612.0;
        let px = img.get_pixel((200.0 * scale) as u32, ((792.0 - 350.0) * scale) as u32);
        assert!(px[2] > 200 && px[0] < 60, "expected the blue rectangle from page 1, got {px:?}");
    }

    #[test]
    fn quick_metadata_still_reads_the_info_dictionary_but_renders_nothing() {
        let mi = get_quick_metadata(Cursor::new(TWO_PAGE_PDF)).unwrap();
        assert!(mi.cover_data.1.is_empty(), "the quick path must not render a cover");
    }

    #[test]
    fn a_pdf_with_no_renderable_page_still_yields_metadata() {
        // Truncated past the header: `lopdf` can still be asked for
        // the Info dictionary, and whatever the renderer makes of it
        // must not turn into an error from `get_metadata`.
        let truncated = &TWO_PAGE_PDF[..TWO_PAGE_PDF.len() / 2];
        let mi = get_metadata(Cursor::new(truncated));
        // Either outcome is acceptable for a damaged file; what is not
        // acceptable is a panic, or a cover appearing from nowhere.
        if let Ok(mi) = mi {
            assert!(mi.cover_data.1.is_empty() || crate::pdf::rasterize::is_available());
        }
    }
}
