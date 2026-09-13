//! Port of `impose()` (`impose.cpp`, issue #670, split from #578),
//! built against [`crate::podofo`]'s `lopdf`-backed [`PdfDoc`] (issue
//! #74's doc-core).
//!
//! Despite the generic-sounding name, real upstream's own comment
//! explains this implements one specific real-world operation:
//! stamping a header/footer (or any small, page-sized overlay) from a
//! run of "source" pages onto a run of "destination" pages, then
//! deleting the source pages -- used for merging a Chromium-rendered
//! header/footer PDF onto a body-content PDF (both already pages of
//! the *same* document by the time `impose` runs, e.g. after an
//! [`crate::podofo_merge::PdfDoc::append`] call merged them).
//!
//! # Building a Form XObject from a page: a scoped-down `podofo_merge` primitive
//!
//! Real upstream's `CreateXObjectForm` + `FillFromPage` is opaque
//! PoDoFo. Since destination and source pages live in the *same*
//! document here (unlike #669's cross-document `append`/`copy_page`),
//! this doesn't need a full object-graph copy at all -- a Form XObject
//! is just another indirect stream object with a `/BBox`/`/Resources`,
//! and it can point at the *same* already-resolved Resources
//! dictionary content the source page uses (a fresh clone of it, not a
//! shared live reference, so later edits to one don't leak into the
//! other) rather than needing [`crate::podofo_merge`]'s reference-remap
//! machinery.
//!
//! # Disclosed, preserved rather than "fixed"
//!
//! - The invocation snippet is **prepended**, not appended, before the
//!   destination page's existing content. Real upstream's own comment:
//!   this ordering (header/footer drawn *first*, then the original
//!   page content) is a deliberate compatibility workaround for older
//!   Qt WebEngine (pre-6.5) not rendering correctly with the reverse
//!   order -- not something derivable from the PDF spec, preserved as
//!   real observed behavior.
//! - Real upstream collapses the destination page's `/Contents` (which
//!   the PDF spec allows to be an array of several streams) down to a
//!   single combined stream on write. This port does the same --
//!   [`lopdf::Document::get_page_content`] already concatenates and
//!   decodes however many content streams a page has, so writing the
//!   combined (snippet + original) bytes back as one new stream is the
//!   natural equivalent, not a narrowing.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::podofo::{PdfDoc, Result};

/// Real upstream's own literal identifier for the Form XObject resource
/// entry -- not uniquified, matching observed behavior.
const HEADER_FOOTER_NAME: &str = "HeaderFooter";

/// Resolves a page's effective `/Resources` dictionary, following an
/// indirect reference if present and walking `/Parent` for spec
/// inheritance when the page has none of its own. Returns an owned
/// clone (the Form XObject gets its own independent copy, not a shared
/// live reference to the source page's dictionary object).
fn resolve_resources(doc: &Document, mut page_id: ObjectId) -> Option<Dictionary> {
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(page_id) {
            return None;
        }
        let dict = doc.get_dictionary(page_id).ok()?;
        if let Ok(value) = dict.get(b"Resources") {
            return match value {
                Object::Dictionary(d) => Some(d.clone()),
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                _ => None,
            };
        }
        page_id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
}

fn ensure_xobject_entry(res: &mut Dictionary, name: &str, xobj_id: ObjectId) {
    if !res.has(b"XObject") {
        res.set("XObject", Object::Dictionary(Dictionary::new()));
    }
    if let Ok(xobjects) = res.get_mut(b"XObject").and_then(Object::as_dict_mut) {
        xobjects.set(name.as_bytes().to_vec(), Object::Reference(xobj_id));
    }
}

/// Registers `xobj_id` as `name` in `page_id`'s own `/Resources/
/// XObject` dict, following an indirect `/Resources` reference in
/// place (rather than assuming it's always inline) so a shared
/// Resources object used by other pages isn't silently replaced by a
/// disconnected copy.
fn add_xobject_resource(doc: &mut Document, page_id: ObjectId, name: &str, xobj_id: ObjectId) -> Result<()> {
    let resources_ref = match doc.get_dictionary(page_id)?.get(b"Resources") {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    if let Some(res_id) = resources_ref {
        ensure_xobject_entry(doc.get_dictionary_mut(res_id)?, name, xobj_id);
    } else {
        let page_dict = doc.get_dictionary_mut(page_id)?;
        if !page_dict.has(b"Resources") {
            page_dict.set("Resources", Object::Dictionary(Dictionary::new()));
        }
        ensure_xobject_entry(page_dict.get_mut(b"Resources").and_then(Object::as_dict_mut)?, name, xobj_id);
    }
    Ok(())
}

impl PdfDoc {
    fn impose_page(&mut self, dest_page_num: u32, src_page_num: u32) -> Result<()> {
        let dest_id = self.page_id(dest_page_num)?;
        let src_id = self.page_id(src_page_num)?;

        let (left, bottom, width, height) = self.get_page_box("MediaBox", src_page_num)?;
        let bbox = Object::Array(vec![
            Object::Real(left as f32),
            Object::Real(bottom as f32),
            Object::Real((left + width) as f32),
            Object::Real((bottom + height) as f32),
        ]);
        let content = self.doc.get_page_content(src_id);
        let resources = resolve_resources(&self.doc, src_id);

        let mut form_dict = Dictionary::new();
        form_dict.set("Type", "XObject");
        form_dict.set("Subtype", "Form");
        form_dict.set("BBox", bbox);
        if let Some(res) = resources {
            form_dict.set("Resources", Object::Dictionary(res));
        }
        let form_id = self.doc.add_object(Stream::new(form_dict, content));

        add_xobject_resource(&mut self.doc, dest_id, HEADER_FOOTER_NAME, form_id)?;

        let mut new_content = format!("q\n1 0 0 1 0 0 cm\n/{HEADER_FOOTER_NAME} Do\nQ\n").into_bytes();
        new_content.extend_from_slice(&self.doc.get_page_content(dest_id));
        let new_contents_id = self.doc.add_object(Stream::new(Dictionary::new(), new_content));
        self.doc.get_dictionary_mut(dest_id)?.set("Contents", Object::Reference(new_contents_id));

        Ok(())
    }

    /// Port of `impose(dest_page_num, src_page_num, count)`. **1-based**
    /// `dest_page_num`/`src_page_num`, matching every other 1-based page
    /// op in this cluster. Pairs up `count` consecutive dest/src pages
    /// 1:1, stamps each source page onto its paired destination page,
    /// then removes the `count` source pages (bounds-guarded, matching
    /// upstream, in case `count` exceeds the remaining page count).
    pub fn impose(&mut self, dest_page_num: u32, src_page_num: u32, count: u32) -> Result<()> {
        for i in 0..count {
            self.impose_page(dest_page_num + i, src_page_num + i)?;
        }
        let mut removed = 0;
        while removed < count && src_page_num <= self.page_count() as u32 {
            self.delete_pages(src_page_num, 1)?;
            removed += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn round_trip(doc: Document) -> PdfDoc {
        let mut doc = doc;
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        PdfDoc::load(&buf).unwrap()
    }

    fn page_content(pdf: &PdfDoc, pagenum: u32) -> Vec<u8> {
        let page_id = pdf.doc.get_pages()[&pagenum];
        pdf.doc.get_page_content(page_id)
    }

    /// A doc with 2 plain body pages plus a 3rd "header/footer" page
    /// carrying its own distinct font resource, ready to be imposed
    /// onto page 1.
    fn fixture() -> Document {
        let mut doc = Document::with_version("1.4");
        let font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });

        let body1 = doc.add_object(Stream::new(Dictionary::new(), b"BT (body one) Tj ET".to_vec()));
        let page1 = doc.add_object(dictionary! {
            "Type" => "Page", "Contents" => Object::Reference(body1),
            "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)]),
        });

        let body2 = doc.add_object(Stream::new(Dictionary::new(), b"BT (body two) Tj ET".to_vec()));
        let page2 = doc.add_object(dictionary! {
            "Type" => "Page", "Contents" => Object::Reference(body2),
            "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)]),
        });

        let hf_content = doc.add_object(Stream::new(Dictionary::new(), b"BT (page N of M) Tj ET".to_vec()));
        let hf_page = doc.add_object(dictionary! {
            "Type" => "Page", "Contents" => Object::Reference(hf_content),
            "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(100)]),
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => Object::Reference(font_id) } },
        });

        let pages_id = doc.add_object(dictionary! {
            "Type" => "Pages", "Count" => 3,
            "Kids" => Object::Array(vec![Object::Reference(page1), Object::Reference(page2), Object::Reference(hf_page)]),
        });
        for p in [page1, page2, hf_page] {
            doc.get_dictionary_mut(p).unwrap().set("Parent", Object::Reference(pages_id));
        }
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    #[test]
    fn impose_prepends_the_header_footer_and_preserves_original_content() {
        let mut pdf = round_trip(fixture());
        pdf.impose(1, 3, 1).unwrap();

        let content = String::from_utf8(page_content(&pdf, 1)).unwrap();
        let snippet_pos = content.find("/HeaderFooter Do").expect("invocation snippet present");
        let body_pos = content.find("(body one)").expect("original content preserved");
        assert!(snippet_pos < body_pos, "header/footer must be drawn BEFORE the original content: {content}");
    }

    #[test]
    fn impose_registers_a_form_xobject_with_the_source_pages_bbox_and_resources() {
        let mut pdf = round_trip(fixture());
        pdf.impose(1, 3, 1).unwrap();

        let page1_id = pdf.doc.get_pages()[&1];
        let dict = pdf.doc.get_dictionary(page1_id).unwrap();
        let xobjects = dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap();
        let form_id = xobjects.get(b"HeaderFooter").unwrap().as_reference().unwrap();
        let form = pdf.doc.get_object(form_id).unwrap().as_stream().unwrap();
        assert_eq!(form.dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Form");
        let bbox = form.dict.get(b"BBox").unwrap().as_array().unwrap();
        assert_eq!(bbox[3].as_float().unwrap(), 100.0, "BBox must be the SOURCE page's MediaBox, not the dest's");
        // The source page's own Font resource must have come along.
        assert!(form.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").is_ok());
        assert_eq!(String::from_utf8(form.content.clone()).unwrap().trim_end(), "BT (page N of M) Tj ET");
    }

    #[test]
    fn impose_removes_the_source_pages_and_leaves_the_others_untouched() {
        let mut pdf = round_trip(fixture());
        assert_eq!(pdf.page_count(), 3);
        pdf.impose(1, 3, 1).unwrap();
        assert_eq!(pdf.page_count(), 2);
        // Page 2 (untouched body page) must survive exactly as it was.
        assert_eq!(String::from_utf8(page_content(&pdf, 2)).unwrap().trim_end(), "BT (body two) Tj ET");
    }

    #[test]
    fn impose_pairs_up_multiple_consecutive_pages() {
        let mut doc = Document::with_version("1.4");
        let mk_page = |doc: &mut Document, text: &str| {
            let c = doc.add_object(Stream::new(Dictionary::new(), text.as_bytes().to_vec()));
            doc.add_object(dictionary! {
                "Type" => "Page", "Contents" => Object::Reference(c),
                "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(100)]),
            })
        };
        let dest1 = mk_page(&mut doc, "dest1");
        let dest2 = mk_page(&mut doc, "dest2");
        let hf1 = mk_page(&mut doc, "hf1");
        let hf2 = mk_page(&mut doc, "hf2");
        let pages_id = doc.add_object(dictionary! {
            "Type" => "Pages", "Count" => 4,
            "Kids" => Object::Array([dest1, dest2, hf1, hf2].map(Object::Reference).to_vec()),
        });
        for p in [dest1, dest2, hf1, hf2] {
            doc.get_dictionary_mut(p).unwrap().set("Parent", Object::Reference(pages_id));
        }
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = round_trip(doc);
        pdf.impose(1, 3, 2).unwrap();
        assert_eq!(pdf.page_count(), 2);
        assert!(String::from_utf8(page_content(&pdf, 1)).unwrap().contains("dest1"));
        assert!(String::from_utf8(page_content(&pdf, 2)).unwrap().contains("dest2"));
    }
}
