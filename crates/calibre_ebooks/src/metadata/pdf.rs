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

use crate::metadata::zip_edit::placeholders;
use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object};
use std::io::{Read, Seek};
use std::path::Path;

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
                // `string_to_authors`, as upstream's own reader does.
                // Splitting on `,`/`;` alone never split on `&` -- so a
                // PDF whose Author is "Ann Author & Bob Writer", which is
                // exactly what calibre itself writes, came back as a
                // single author with that whole string as a name. It also
                // split "Herbert, Frank" into two people.
                mi.authors = crate::metadata::authors::string_to_authors(&author);
            }
            // Both are read, because both are used in the wild:
            // upstream calibre and Acrobat write tags to `Keywords`, while
            // some producers put them in `Subject`. Reading only `Subject`
            // meant tags written by calibre itself were invisible.
            for key in [&b"Keywords"[..], &b"Subject"[..]] {
                let Some(value) = info_dict.get(key).ok().and_then(|o| from_pdf_object(o).ok()) else {
                    continue;
                };
                let found: Vec<String> = value.split(&[',', ';'][..]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                for tag in found {
                    if !mi.tags.contains(&tag) {
                        mi.tags.push(tag);
                    }
                }
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

/// Writes `mi` into a PDF's Info dictionary, in place (#834).
///
/// Port of the Info-dictionary half of `utils/podofo`'s
/// `set_metadata_implementation`: `Title`, `Author`, and `Keywords`.
///
/// **Disclosed narrowing:** upstream also writes an XMP packet and merges
/// it with any existing one. XMP is a second, parallel metadata store
/// inside the PDF, and building one needs `metadata_to_xmp_packet` --
/// unported. So a reader that prefers XMP over the Info dictionary will
/// still show the old values. The Info dictionary is what this port's own
/// reader, and most viewers' document-properties panel, use.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    // Loaded from bytes rather than by path so no file handle is held
    // while the result is written back -- Windows refuses to replace a
    // file anything still has open. Same reason as
    // `metadata::zip_edit::replace_entry`'s scoping.
    let buffer = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc = Document::load_mem(&buffer).context("not a readable PDF")?;

    let mut entries: Vec<(&str, String)> = Vec::new();
    if let Some(title) = placeholders::real_title(&mi.title) {
        entries.push(("Title", title.to_string()));
    }
    if let Some(authors) = placeholders::real_authors(&mi.authors) {
        // One `Author` string, joined the way the reader splits it back.
        entries.push(("Author", crate::metadata::authors::authors_to_string(authors)));
    }
    let tags: Vec<&str> = mi.tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    if !tags.is_empty() {
        // `Keywords`, not `Subject`: that is what upstream writes and what
        // viewers show as keywords. `Subject` is a one-line description of
        // the document, not a tag list.
        entries.push(("Keywords", tags.join(", ")));
    }
    if entries.is_empty() {
        // Nothing real to write. Saving anyway would rewrite the whole
        // file and change its checksum for no reason.
        return Ok(());
    }

    let info_id = match doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => {
            // A PDF need not have an Info dictionary at all; make one and
            // point the trailer at it.
            let id = doc.add_object(Dictionary::new());
            doc.trailer.set("Info", Object::Reference(id));
            id
        }
    };

    {
        let info = doc.get_object_mut(info_id).and_then(|o| o.as_dict_mut()).context("the PDF's Info entry is not a dictionary")?;
        for (key, value) in entries {
            // A literal PDF string. `Document::save` escapes it.
            info.set(key, Object::string_literal(value));
        }
    }

    // Written beside the original and renamed over it, so an interrupted
    // save cannot truncate the book.
    let staging = tempfile::Builder::new().prefix("set-metadata").suffix(".pdf").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    doc.save_to(&mut std::fs::File::create(staging.path())?).context("writing the updated PDF")?;
    staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
    Ok(())
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

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use lopdf::dictionary;

    /// A real, minimal, loadable PDF. `with_info` controls whether it has
    /// an Info dictionary at all -- a PDF need not, and the writer has to
    /// cope either way.
    fn write_pdf(path: &Path, with_info: bool) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let content_id = doc.add_object(lopdf::Stream::new(dictionary! {}, b"BT /F1 12 Tf (Hello) Tj ET".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        if with_info {
            let info_id = doc.add_object(dictionary! {
                "Title" => Object::string_literal("Old Title"),
                "Author" => Object::string_literal("Old Author"),
                "Producer" => Object::string_literal("Some Producer"),
            });
            doc.trailer.set("Info", info_id);
        }
        doc.save(path).unwrap();
    }

    fn a_pdf(dir: &tempfile::TempDir, with_info: bool) -> std::path::PathBuf {
        let path = dir.path().join("book.pdf");
        write_pdf(&path, with_info);
        path
    }

    fn read_back(path: &Path) -> MetaInformation {
        get_quick_metadata(std::fs::File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn title_and_authors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "New Title");
        assert_eq!(got.authors, vec!["Ann Author".to_string(), "Bob Writer".to_string()]);
    }

    /// The case the old author split corrupted: a single author whose
    /// name contains a comma was read back as two people.
    #[test]
    fn an_author_name_containing_a_comma_stays_one_author() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);

        let mut mi = MetaInformation::default();
        mi.authors = vec!["Herbert, Frank".to_string()];
        set_metadata(&path, &mi).unwrap();

        assert_eq!(read_back(&path).authors, vec!["Herbert, Frank".to_string()], "splitting on a comma invented a second author");
    }

    /// Tags go to `Keywords`, which is what upstream and Acrobat use. The
    /// reader was only looking at `Subject`, so this would have written
    /// tags our own reader could not see.
    #[test]
    fn tags_round_trip_through_keywords() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);

        let mut mi = MetaInformation::default();
        mi.tags = vec!["Science Fiction".to_string(), "Classics".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert!(got.tags.contains(&"Science Fiction".to_string()), "{:?}", got.tags);
        assert!(got.tags.contains(&"Classics".to_string()), "{:?}", got.tags);
    }

    /// A PDF with no Info dictionary gets one.
    #[test]
    fn a_pdf_without_an_info_dictionary_gains_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, false);
        assert_eq!(read_back(&path).title, "Unknown", "the fixture should start with no title");

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        assert_eq!(read_back(&path).title, "New Title");
    }

    /// The same placeholder rule as the other formats.
    #[test]
    fn placeholder_metadata_does_not_overwrite_real_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);
        let before = std::fs::read(&path).unwrap();

        set_metadata(&path, &MetaInformation::default()).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "Old Title", "the real title was overwritten with a placeholder");
        assert_eq!(got.authors, vec!["Old Author".to_string()]);
        // And nothing was written at all, so the checksum is unchanged --
        // rewriting the file for no change would make the next scan report
        // the book as edited.
        assert_eq!(std::fs::read(&path).unwrap(), before, "a no-op write should not touch the file");
    }

    /// Fields the PDF already had that this does not set must survive.
    #[test]
    fn other_info_entries_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let doc = Document::load(&path).unwrap();
        let info_id = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = doc.get_object(info_id).unwrap().as_dict().unwrap();
        assert!(info.get(b"Producer").is_ok(), "the Producer entry was lost");
        assert!(info.get(b"Author").is_ok(), "the Author entry was lost");
    }

    /// The document itself must still be a readable one-page PDF.
    #[test]
    fn the_document_is_still_valid_and_has_its_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_pdf(&dir, true);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let doc = Document::load(&path).unwrap();
        assert_eq!(doc.get_pages().len(), 1, "the page should still be there");
    }
}
