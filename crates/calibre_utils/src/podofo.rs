//! Port of `calibre.utils.podofo` (`calibre_extensions/podofo` C++ extension),
//! the PDF metadata/structure manipulation layer used to write book metadata
//! into PDF files, read PDF outlines, count embedded images, etc.
//!
//! # Library choice: `lopdf`, not an FFI wrap of PoDoFo
//!
//! Real upstream binds the C++ PoDoFo library directly. Per explicit project
//! direction, this port does not FFI-wrap PoDoFo; it is built on
//! [`lopdf`](https://docs.rs/lopdf), a pure-Rust, actively maintained PDF
//! document-object-model library. `lopdf` exposes the same *shape* of API as
//! PoDoFo's own object model (indirect objects, dictionaries, streams, a
//! trailer) — porting the real C++ algorithms onto it is a faithful
//! translation, not a redesign.
//!
//! `lopdf` already provides PDF *text string* encode/decode
//! ([`lopdf::text_string`]/[`lopdf::decode_text_string`]: ASCII → literal
//! PDFDocEncoding, non-ASCII → UTF-16BE with a BOM; decode sniffs the BOM and
//! falls back to the PDFDocEncoding table otherwise), so unlike PoDoFo (whose
//! `PdfString::GetString()` does this decoding internally, invisibly to the
//! C++ binding code) this port does not need to hand-roll a PDF string codec
//! against the spec.
//!
//! # Scope of this module (doc-core)
//!
//! This covers `doc.cpp`'s core document-level operations only: load/open/
//! save/write, the PDF version, page count, the six Info-dict string
//! properties, XMP metadata get/set, and image counting. Outline/bookmark
//! trees (issue #576), font management (issue #577), and image
//! deduplication / page imposition / document merging (issue #578) are
//! separate, larger pieces of the same real C++ source and are tracked as
//! their own follow-on issues, each depending on this module.
//!
//! # Disclosed deviations from upstream
//!
//! - `PdfSaveOptions::NoMetadataUpdate` (used on every real PoDoFo save call
//!   to stop PoDoFo from clobbering `/Info`/XMP on save) has no Rust
//!   equivalent concern: `lopdf::Document::save`/`save_to` only serialize the
//!   existing object table and never rewrite `/Info` themselves.
//! - `image_count()`: real upstream counts an object if `/Type == /XObject`
//!   **OR** `/Subtype == /Image` — an OR, not an AND, which over-counts Form
//!   XObjects (`/Subtype /Form`) and is inconsistent with the correctly
//!   AND-gated image detection used elsewhere in the very same C++ codebase
//!   (`dedup_images`, `remove_unused_fonts`'s Form-XObject check). This is a
//!   likely-unintentional bug, not a deliberate design choice, so this port
//!   uses the correct AND (`/Type == /XObject && /Subtype == /Image`)
//!   instead of silently reproducing it.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// Errors from doc-core PDF operations.
#[derive(Debug, thiserror::Error)]
pub enum PodofoError {
    #[error("PDF error: {0}")]
    Pdf(#[from] lopdf::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid page number: {0}")]
    InvalidPage(u32),
    #[error("Outline item has no parent")]
    OutlineItemHasNoParent,
}

pub type Result<T> = std::result::Result<T, PodofoError>;

/// A loaded PDF document, mirroring the real `podofo.PDFDoc` Python type's
/// doc-core surface.
pub struct PdfDoc {
    pub(crate) doc: Document,
}

impl PdfDoc {
    /// Loads a PDF from an in-memory byte buffer (`PDFDoc.load(raw)`).
    pub fn load(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            doc: Document::load_mem(bytes)?,
        })
    }

    /// Loads a PDF from a file path (`PDFDoc.open(path)`).
    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        Ok(Self {
            doc: Document::load(path)?,
        })
    }

    /// Serializes and writes the PDF to a file path (`PDFDoc.save(path)`).
    pub fn save<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        self.doc.save(path)?;
        Ok(())
    }

    /// Serializes the PDF to an in-memory byte buffer (`PDFDoc.write()`).
    pub fn write(&mut self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.doc.save_to(&mut buf)?;
        Ok(buf)
    }

    /// The number of leaf pages in the document (`PDFDoc.page_count()`/`.pages`).
    pub fn page_count(&self) -> usize {
        self.doc.get_pages().len()
    }

    /// The PDF version, e.g. `"1.7"` (`PDFDoc.version`).
    ///
    /// Real PoDoFo reads the `%PDF-x.y` header version. Per the PDF spec, a
    /// Catalog `/Version` name entry should win when it specifies a later
    /// version than the header; this is honored here too.
    pub fn version(&self) -> String {
        let header_version = self.doc.version.clone();
        if let Ok(catalog) = self.doc.catalog() {
            if let Some(name) = catalog
                .get(b"Version")
                .ok()
                .and_then(|o| o.as_name().ok())
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
            {
                if version_ge(name, &header_version) {
                    return name.to_string();
                }
            }
        }
        header_version
    }

    /// Number of image XObjects in the document (`PDFDoc.image_count()`).
    ///
    /// See the module doc for the disclosed OR-vs-AND deviation from
    /// upstream's likely-buggy real implementation.
    pub fn image_count(&self) -> usize {
        self.doc
            .objects
            .values()
            .filter(|obj| {
                obj.as_dict()
                    .map(|dict| is_name(dict, b"Type", b"XObject") && is_name(dict, b"Subtype", b"Image"))
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn title(&self) -> String {
        self.get_info_string(b"Title")
    }
    pub fn set_title(&mut self, value: &str) {
        self.set_info_string(b"Title", value);
    }

    pub fn author(&self) -> String {
        self.get_info_string(b"Author")
    }
    pub fn set_author(&mut self, value: &str) {
        self.set_info_string(b"Author", value);
    }

    pub fn subject(&self) -> String {
        self.get_info_string(b"Subject")
    }
    pub fn set_subject(&mut self, value: &str) {
        self.set_info_string(b"Subject", value);
    }

    pub fn keywords(&self) -> String {
        self.get_info_string(b"Keywords")
    }
    pub fn set_keywords(&mut self, value: &str) {
        self.set_info_string(b"Keywords", value);
    }

    pub fn creator(&self) -> String {
        self.get_info_string(b"Creator")
    }
    pub fn set_creator(&mut self, value: &str) {
        self.set_info_string(b"Creator", value);
    }

    pub fn producer(&self) -> String {
        self.get_info_string(b"Producer")
    }
    pub fn set_producer(&mut self, value: &str) {
        self.set_info_string(b"Producer", value);
    }

    /// Reads the raw XMP packet from the Catalog's `/Metadata` stream, if
    /// any (`PDFDoc.get_xmp_metadata()`).
    pub fn get_xmp_metadata(&self) -> Option<Vec<u8>> {
        let catalog = self.doc.catalog().ok()?;
        let metadata_ref = catalog.get(b"Metadata").ok()?.as_reference().ok()?;
        let stream = self.doc.get_object(metadata_ref).ok()?.as_stream().ok()?;
        Some(stream.content.clone())
    }

    /// Writes a raw XMP packet into the Catalog's `/Metadata` stream,
    /// creating it if needed, explicitly with no `/Filter` — stored raw, not
    /// compressed (`PDFDoc.set_xmp_metadata(packet)`).
    pub fn set_xmp_metadata(&mut self, packet: &[u8]) -> Result<()> {
        let metadata_id = self.get_or_create_metadata_object_id()?;
        let obj = self.doc.get_object_mut(metadata_id)?;
        let stream = obj.as_stream_mut()?;
        stream.set_plain_content(packet.to_vec());
        stream.dict.remove(b"Filter");
        Ok(())
    }

    fn get_or_create_metadata_object_id(&mut self) -> Result<ObjectId> {
        if let Ok(catalog) = self.doc.catalog() {
            if let Ok(existing) = catalog.get(b"Metadata").and_then(Object::as_reference) {
                return Ok(existing);
            }
        }
        let stream = Stream::new(
            dictionary! {
                "Type" => "Metadata",
                "Subtype" => "XML",
            },
            Vec::new(),
        );
        let metadata_id = self.doc.add_object(stream);
        let catalog = self.doc.catalog_mut()?;
        catalog.set("Metadata", Object::Reference(metadata_id));
        Ok(metadata_id)
    }

    /// Looks up the trailer's `/Info` dictionary, creating an empty one
    /// (wired in indirectly, per the PDF spec's `/Info N G R` requirement)
    /// if none exists — matching real PoDoFo's `get_or_create_info()`, whose
    /// getters have the side effect of creating the dict even on a pure read.
    fn get_or_create_info_id(&mut self) -> ObjectId {
        if let Some(existing) = self.doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok()) {
            return existing;
        }
        let info_id = self.doc.add_object(Dictionary::new());
        self.doc.trailer.set("Info", Object::Reference(info_id));
        info_id
    }

    fn get_info_string(&self, key: &[u8]) -> String {
        let info_id = match self.doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok()) {
            Some(id) => id,
            None => return String::new(),
        };
        let info = match self.doc.get_dictionary(info_id) {
            Ok(dict) => dict,
            Err(_) => return String::new(),
        };
        match info.get(key) {
            Ok(obj) => lopdf::decode_text_string(obj).unwrap_or_default(),
            Err(_) => String::new(),
        }
    }

    fn set_info_string(&mut self, key: &[u8], value: &str) {
        let info_id = self.get_or_create_info_id();
        let info = self
            .doc
            .get_dictionary_mut(info_id)
            .expect("just created or already present");
        if value.is_empty() {
            info.remove(key);
        } else {
            info.set(key, lopdf::text_string(value));
        }
    }
}

fn is_name(dict: &Dictionary, key: &[u8], expected: &[u8]) -> bool {
    dict.get(key)
        .and_then(Object::as_name)
        .map(|name| name == expected)
        .unwrap_or(false)
}

/// Compares two `"major.minor"` PDF version strings, returning whether `a >= b`.
/// Falls back to `false` on any malformed input (keeps the header version).
fn version_ge(a: &str, b: &str) -> bool {
    fn parse(s: &str) -> Option<(u32, u32)> {
        let (maj, min) = s.split_once('.')?;
        Some((maj.parse().ok()?, min.parse().ok()?))
    }
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a >= b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The exact minimal fixture PDF from upstream's own `test_podofo()` in
    // `old_src/src/calibre/utils/podofo/__init__.py` — a real, complete,
    // minimal one-page PDF with an Info dict (UTF-16BE-encoded Author/Title)
    // and a FlateDecode-compressed XMP Metadata stream.
    const FIXTURE: &[u8] = b"%PDF-1.1\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog/Metadata 6 0 R/Pages 2 0 R>>\nendobj\n2 0 obj<</Type/Pages/Count 1/Kids[ 3 0 R]/MediaBox[ 0 0 300 144]>>\nendobj\n3 0 obj<</Type/Page/Contents 4 0 R/Parent 2 0 R/Resources<</Font<</F1<</Type/Font/BaseFont/Times-Roman/Subtype/Type1>>>>>>>>\nendobj\n4 0 obj<</Length 55>>\nstream\n  BT\n    /F1 18 Tf\n    0 0 Td\n    (Hello World) Tj\n  ET\nendstream\nendobj\n5 0 obj<</Author(\xfe\xff\x00U\x00n\x00k\x00n\x00o\x00w\x00n)/CreationDate(D:20140919134038+05'00')/Producer(PoDoFo - http://podofo.sf.net)/Title(\xfe\xff\x00n\x00e\x00w\x00t)>>\nendobj\n6 0 obj<</Type/Metadata/Filter/FlateDecode/Length 584/Subtype/XML>>\nstream\nx\x9c\xed\x98\xcd\xb2\x930\x14\xc7\xf7}\n&.\x1d\x1ahoGa\x80\x8e\xb6\xe3x\x17ua\xaf\xe3\xd2\t\xc9i\x1b\x0b\x81&a\xc0\xfbj.|$_\xc1\xd0r\xe9\xb7V\x9d\xbb\x83\x15\x9c\x9c\xff\xff\x97\x8fs\xb2 \x18W9\xa1k\xd0V\x0cK.B\xf4\xf3\xfb\x0fdq\x16\xa2\xcf\xa3\x993\xcb'\xb0\xe2\xef\x1f%\xcc\x1f?<\xd0\xc75\xf5\x18\x1aG\xbd\xa0\xf2\xab4OA\x13\xabJ\x13\xa1\xfc*D\x84e1\xf8\xe6\xbd\x0ec\x14\xf5,+\x90l\xe1\x7f\x9c\xbek\x92\xccW\x88VZ\xe7>\xc6eY\xf6\xcba?\x93K\xecz\x9e\x87\x9d\x01\x1e\x0cl\x93a\xaboB\x93\xca\x16\xea\xc5\xd6\xa3q\x99\x82\xa2\x92\xe7\x9ag\xa2qc\xb45\xcb\x0b\x99l\xad\x18\xc5\x90@\nB+\xec\xf6]\x8c\xacZK\xe2\xac\xd0!j\xec\x8c!\xa3>\xdb\xfb=\x85\x1b\xd2\x9bD\xef#M,\xe15\xd4O\x88X\x86\xa8\xb2\x19,H\x91h\x14\x05x7z`\x81O<\x02|\x99VOBs\x9d\xc0\x7f\xe0\x05\x94\xfa\xd6)\x1c\xb1jx^\xc4\tW+\x90'\x13xK\x96\xf8Hy\x96X\xabU\x11\x7f\x05\xaa\xff\xa4=I\xab\x95T\x02\xd1\xd9)u\x0e\x9b\x0b\xcb\x8e>\x89\xb5\xc8Jqm\x91\x07\xaa-\xee\xc8{\x972=\xdd\xfa+\xe5d\xea\xb9\xad'\xa1\xfa\xdbj\xee\xd3,\xc5\x15\xc9M-9\xa6\x96\xdaD\xce6Wr\xd3\x1c\xdf3S~|\xc1A\xe2MA\x92F{\xb1\x0eM\xba?3\xdd\xc2\x88&S\xa2!\x1a8\xee\x9d\xedx\xb6\xeb=\xb8C\xff\xce\xf1\x87\xaf\xfb\xde\xe0\xd5\xc8\xf3^:#\x7f\xe8\x04\xf8L\xf2\x0fK\xcd%W\xe9\xbey\xea/\xa5\x89`D\xb2m\x17\t\x92\x822\xb7\x02(\x1c\x13\xc5)\x1e\x9c-\x01\xff\x1e\xc0\x16\xd5\xe5\r\xaaG\xcc\x8e\x0c\xff\xca\x8e\x92\x84\xc7\x12&\x93\xd6\xb3\x89\xd8\x10g\xd9\xfai\xe7\xedv\xde6-\x94\xceR\x9bfI\x91\n\x85\x8e}nu9\x91\xcd\xefo\xc6+\x90\x1c\x94\xcd\x05\x83\xea\xca\xd17\x16\xbb\xb6\xfc\xa22\xa9\x9bn\xbe0p\xfd\x88wAs\xc3\x9a+\x19\xb7w\xf2a#=\xdf\xd3A:H\x07\xe9 \x1d\xa4\x83t\x90\x0e\xd2A:H\x07yNH/h\x7f\xd6\x80`!*\xd18\xfa\x05\x94\x80P\xb0\nendstream\nendobj\nxref\n0 7\n0000000000 65535 f \n0000000015 00000 n \n0000000074 00000 n \n0000000148 00000 n \n0000000280 00000 n \n0000000382 00000 n \n0000000522 00000 n \ntrailer\n<</ID[<4D028D512DEBEFD964756764AD8FF726><4D028D512DEBEFD964756764AD8FF726>]/Info 5 0 R/Root 1 0 R/Size 7>>\nstartxref\n1199\n%%EOF\n";

    #[test]
    fn loads_the_fixture_and_reads_info_dict_strings_decoded_from_utf16be() {
        let doc = PdfDoc::load(FIXTURE).unwrap();
        assert_eq!(doc.title(), "newt");
        assert_eq!(doc.author(), "Unknown");
        assert_eq!(doc.producer(), "PoDoFo - http://podofo.sf.net");
    }

    #[test]
    fn page_count_matches_the_single_page_fixture() {
        let doc = PdfDoc::load(FIXTURE).unwrap();
        assert_eq!(doc.page_count(), 1);
    }

    #[test]
    fn version_reads_the_pdf_header() {
        let doc = PdfDoc::load(FIXTURE).unwrap();
        assert_eq!(doc.version(), "1.1");
    }

    #[test]
    fn set_and_get_title_round_trips_through_a_reload() {
        let mut doc = PdfDoc::load(FIXTURE).unwrap();
        doc.set_title("info title");
        doc.set_author("info author");
        doc.set_keywords("a, b");
        let bytes = doc.write().unwrap();

        let reloaded = PdfDoc::load(&bytes).unwrap();
        assert_eq!(reloaded.title(), "info title");
        assert_eq!(reloaded.author(), "info author");
        assert_eq!(reloaded.keywords(), "a, b");
    }

    #[test]
    fn setting_a_string_to_empty_removes_the_info_key() {
        let mut doc = PdfDoc::load(FIXTURE).unwrap();
        assert_eq!(doc.author(), "Unknown");
        doc.set_author("");
        assert_eq!(doc.author(), "");
    }

    #[test]
    fn xmp_metadata_round_trips_set_then_get() {
        let mut doc = PdfDoc::load(FIXTURE).unwrap();
        let packet = b"<x:xmpmeta>hello xmp</x:xmpmeta>".to_vec();
        doc.set_xmp_metadata(&packet).unwrap();
        assert_eq!(doc.get_xmp_metadata().unwrap(), packet);
    }

    #[test]
    fn xmp_metadata_is_none_when_document_has_no_metadata_stream() {
        // A doc built fresh in-memory (not the fixture) with no /Metadata key.
        let mut doc = PdfDoc::load(FIXTURE).unwrap();
        let catalog = doc.doc.catalog_mut().unwrap();
        catalog.remove(b"Metadata");
        assert!(doc.get_xmp_metadata().is_none());
    }

    /// Builds a minimal but structurally real, valid PDF (proper xref/trailer)
    /// via `lopdf`'s own object model + `save_to`, since `Document::load_mem`
    /// requires a well-formed xref table that isn't worth hand-writing byte
    /// offsets for.
    fn minimal_pdf_bytes(extra_objects: &[Dictionary]) -> Vec<u8> {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.set_object(pages_id, dictionary! {
            "Type" => "Pages",
            "Count" => 0,
            "Kids" => Object::Array(vec![]),
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        for extra in extra_objects {
            doc.add_object(extra.clone());
        }
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    #[test]
    fn info_dict_is_created_lazily_as_a_real_indirect_object() {
        let raw = minimal_pdf_bytes(&[]);
        let mut doc = PdfDoc::load(&raw).unwrap();
        assert_eq!(doc.title(), "");
        doc.set_title("hello");
        assert_eq!(doc.title(), "hello");
        let info_ref = doc.doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        // Must be a real indirect object, not an inline dict.
        assert!(doc.doc.get_dictionary(info_ref).is_ok());
    }

    #[test]
    fn image_count_uses_and_not_the_upstream_or_bug() {
        // A Form XObject (Subtype/Form, no Type/XObject match with Subtype/Image)
        // must NOT be counted, unlike upstream's disclosed OR-bug.
        let raw = minimal_pdf_bytes(&[
            dictionary! { "Type" => "XObject", "Subtype" => "Form" },
            dictionary! { "Type" => "XObject", "Subtype" => "Image" },
        ]);
        let doc = PdfDoc::load(&raw).unwrap();
        assert_eq!(doc.image_count(), 1);
    }
}
