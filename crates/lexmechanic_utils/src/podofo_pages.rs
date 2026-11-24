//! Port of `calibre.utils.podofo`'s standalone page operations
//! (`doc.cpp`'s `delete_pages`/`get_page_box`/`set_page_box`/`set_box`/
//! `extract_first_page`/`extract_anchors`/`alter_links`, issue #668,
//! split from #578), built against [`crate::podofo`]'s `lopdf`-backed
//! [`PdfDoc`] (issue #74's doc-core).
//!
//! # Indexing conventions (real upstream is inconsistent, ported as-is)
//!
//! Real upstream's own C++ mixes 1-based-from-caller (converted to
//! 0-based internally) and plain 0-based arguments across these
//! functions, with no consistent rule. This port preserves each
//! function's own real observed convention rather than silently
//! unifying them, and documents it on every method:
//! - [`PdfDoc::delete_pages`], [`PdfDoc::get_page_box`],
//!   [`PdfDoc::set_page_box`]: **1-based**.
//! - [`PdfDoc::set_box`]: **0-based** (real upstream's own lower-level/
//!   more-permissive variant of `set_page_box`).
//! - [`PdfDoc::alter_links`]'s callback: the page number it returns is
//!   **1-based**, matching upstream's own `get_page(doc, pagenum - 1)`.
//!
//! # Page-box inheritance (real PDF spec rule, not in `lopdf`)
//!
//! Real upstream's PoDoFo page-box getters resolve the PDF spec's
//! page-attribute inheritance: a box not present on the page dict
//! itself is looked up on ancestor `/Pages` nodes, and if still absent
//! anywhere in that chain, `CropBox` defaults to `MediaBox`, while
//! `TrimBox`/`BleedBox`/`ArtBox` default to `CropBox` (which may itself
//! fall back to `MediaBox`). `lopdf` has no such box-resolution helper
//! (its own [`lopdf::Document::get_page_resources`] does this same
//! *shape* of `/Parent`-walk for `/Resources`, which is the template
//! this module's own walk follows) -- implemented by hand here.
//! [`PdfDoc::set_page_box`] always writes directly onto the page's own
//! dictionary, never onto an ancestor, even if the box was previously
//! inherited (matching upstream).
//!
//! # Disclosed real-upstream limitations, preserved rather than widened
//!
//! - [`PdfDoc::extract_anchors`] only reads the Catalog's `/Dests` when
//!   it is a **flat dictionary reached via an indirect reference**.
//!   Real upstream doesn't handle an inline (non-indirect) `/Dests`
//!   dict, nor the modern `/Names/Dests` name-tree structure PDF
//!   producers increasingly use instead of the legacy flat form -- both
//!   are real functional limitations of the code being ported, not
//!   bugs introduced here.
//! - Only `[page /XYZ left top zoom]`-shaped destination arrays are
//!   recognized (matching upstream's own comment that this is enough
//!   for its one real caller); other legal shapes (`/Fit`, `/FitH`,
//!   `/FitR`, ...) are silently skipped, not treated as errors.
//! - `zoom` is read as an integer (`GetNumber()` as a C++ `long long`
//!   in upstream), not a float, even though the PDF spec's `/XYZ` zoom
//!   operand is a real number -- preserved as `i64` here, matching the
//!   observed real behavior (truncates a fractional zoom).

use std::collections::HashSet;

use lopdf::{Object, ObjectId};

use crate::podofo::{is_name, PdfDoc, PodofoError, Result};

/// The 5 real page-box names `get_page_box`/`set_page_box` accept.
pub const BOX_NAMES: &[&str] = &["MediaBox", "CropBox", "TrimBox", "BleedBox", "ArtBox"];

/// One entry from [`PdfDoc::extract_anchors`]: `(page, left, top, zoom)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub page: u32,
    pub left: f64,
    pub top: f64,
    pub zoom: i64,
}

fn make_box_array(left: f64, bottom: f64, width: f64, height: f64) -> Object {
    Object::Array(vec![
        Object::Real(left as f32),
        Object::Real(bottom as f32),
        Object::Real((left + width) as f32),
        Object::Real((bottom + height) as f32),
    ])
}

fn parse_box_array(arr: &[Object]) -> Option<(f64, f64, f64, f64)> {
    if arr.len() < 4 {
        return None;
    }
    let mut v = [0f64; 4];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = arr[i].as_float().ok()? as f64;
    }
    let (llx, lly, urx, ury) = (v[0], v[1], v[2], v[3]);
    Some((llx, lly, urx - llx, ury - lly))
}

impl PdfDoc {
    pub(crate) fn page_id(&self, pagenum: u32) -> Result<ObjectId> {
        self.doc
            .get_pages()
            .get(&pagenum)
            .copied()
            .ok_or(PodofoError::InvalidPage(pagenum))
    }

    fn page_number_of(&self, page_id: ObjectId) -> Option<u32> {
        self.doc
            .get_pages()
            .iter()
            .find(|&(_, &id)| id == page_id)
            .map(|(&n, _)| n)
    }

    /// Walks `page_id` then its `/Parent` chain looking for `key` as a
    /// box array (`[llx lly urx ury]`), converting to `(left, bottom,
    /// width, height)`. Does not apply the `CropBox`/`MediaBox`
    /// fallback chain -- see [`Self::page_box_with_fallback`] for that.
    fn find_inherited_box(&self, page_id: ObjectId, key: &[u8]) -> Option<(f64, f64, f64, f64)> {
        let mut cur = page_id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(cur) {
                return None;
            }
            let dict = self.doc.get_dictionary(cur).ok()?;
            if let Ok(arr) = dict.get_deref(key, &self.doc).and_then(Object::as_array) {
                if let Some(b) = parse_box_array(arr) {
                    return Some(b);
                }
            }
            cur = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
        }
    }

    /// Port of the real page-box-inheritance fallback rule: `CropBox`
    /// defaults to `MediaBox`; `TrimBox`/`BleedBox`/`ArtBox` default to
    /// `CropBox` (itself falling back to `MediaBox` if also absent).
    fn page_box_with_fallback(&self, page_id: ObjectId, which: &str) -> Option<(f64, f64, f64, f64)> {
        if let Some(b) = self.find_inherited_box(page_id, which.as_bytes()) {
            return Some(b);
        }
        match which {
            "MediaBox" => None,
            "CropBox" => self.page_box_with_fallback(page_id, "MediaBox"),
            _ => self.page_box_with_fallback(page_id, "CropBox"),
        }
    }

    /// Port of `get_page_box(which, pagenum)`. **1-based** `pagenum`.
    pub fn get_page_box(&self, which: &str, pagenum: u32) -> Result<(f64, f64, f64, f64)> {
        if !BOX_NAMES.contains(&which) {
            return Err(PodofoError::UnknownBoxName(which.to_string()));
        }
        let page_id = self.page_id(pagenum)?;
        self.page_box_with_fallback(page_id, which)
            .ok_or(PodofoError::InvalidPage(pagenum))
    }

    /// Port of `set_page_box(which, pagenum, left, bottom, width,
    /// height)`. **1-based** `pagenum`. Always writes directly onto the
    /// page's own dictionary, never an ancestor, even if the box was
    /// previously resolved via inheritance.
    pub fn set_page_box(&mut self, which: &str, pagenum: u32, left: f64, bottom: f64, width: f64, height: f64) -> Result<()> {
        if !BOX_NAMES.contains(&which) {
            return Err(PodofoError::UnknownBoxName(which.to_string()));
        }
        let page_id = self.page_id(pagenum)?;
        let arr = make_box_array(left, bottom, width, height);
        self.doc.get_dictionary_mut(page_id)?.set(which.as_bytes().to_vec(), arr);
        Ok(())
    }

    /// Port of `set_box(page_num, box_name, left, bottom, width,
    /// height)`: the same box-array construction as
    /// [`Self::set_page_box`], but **0-based** `page_num` and an
    /// unrestricted `box_name` (any string is accepted and set as a
    /// literal dictionary key, matching upstream's more permissive/
    /// low-level real behavior).
    pub fn set_box(&mut self, page_num: u32, box_name: &str, left: f64, bottom: f64, width: f64, height: f64) -> Result<()> {
        let page_id = self.page_id(page_num + 1)?;
        let arr = make_box_array(left, bottom, width, height);
        self.doc.get_dictionary_mut(page_id)?.set(box_name.as_bytes().to_vec(), arr);
        Ok(())
    }

    /// Removes the page at 1-based `pagenum`, unlinking it from its
    /// parent's `/Kids` array, decrementing `/Count` up the ancestor
    /// chain, and deleting the page object itself. Shared by
    /// [`Self::delete_pages`] and [`Self::extract_first_page`].
    fn remove_page_at(&mut self, pagenum: u32) -> Result<()> {
        let page_id = self.page_id(pagenum)?;
        let parent_id = self
            .doc
            .get_dictionary(page_id)
            .ok()
            .and_then(|d| d.get(b"Parent").and_then(Object::as_reference).ok());

        if let Some(parent_id) = parent_id {
            if let Ok(dict) = self.doc.get_dictionary_mut(parent_id) {
                if let Ok(kids) = dict.get_mut(b"Kids").and_then(Object::as_array_mut) {
                    kids.retain(|o| o.as_reference().ok() != Some(page_id));
                }
            }
            let mut cur = Some(parent_id);
            let mut seen = HashSet::new();
            while let Some(id) = cur {
                if !seen.insert(id) {
                    break;
                }
                cur = match self.doc.get_dictionary_mut(id) {
                    Ok(dict) => {
                        if let Ok(Object::Integer(count)) = dict.get_mut(b"Count") {
                            *count -= 1;
                        }
                        dict.get(b"Parent").and_then(Object::as_reference).ok()
                    }
                    Err(_) => None,
                };
            }
        }
        self.doc.objects.remove(&page_id);
        Ok(())
    }

    /// Port of `delete_pages(page, count)`. **1-based** `page`. Removes
    /// `count` consecutive pages starting at `page` by repeatedly
    /// removing at the same index -- correct because every removal
    /// shifts subsequent pages down by one, matching upstream's own
    /// `RemovePageAt` loop.
    pub fn delete_pages(&mut self, page: u32, count: u32) -> Result<()> {
        for _ in 0..count {
            self.remove_page_at(page)?;
        }
        Ok(())
    }

    /// Port of `extract_first_page()`: truncates the document down to
    /// just its first page. Leaves any indirect objects only the
    /// removed pages referenced (fonts, images, ...) as orphans in the
    /// object table -- matching upstream, which does no reachability
    /// garbage collection here (that's `remove_unused_fonts`/
    /// `dedup_images`'s job, run separately).
    pub fn extract_first_page(&mut self) -> Result<()> {
        while self.page_count() > 1 {
            self.remove_page_at(2)?;
        }
        Ok(())
    }

    /// Port of `extract_anchors()`. See the module docs for the two
    /// real, disclosed limitations (flat-dict-only, `/XYZ`-only).
    pub fn extract_anchors(&self) -> Result<std::collections::HashMap<String, Anchor>> {
        let mut out = std::collections::HashMap::new();
        let catalog = self.doc.catalog()?;
        let Ok(Object::Reference(dests_ref)) = catalog.get(b"Dests") else {
            return Ok(out);
        };
        let Ok(dests_dict) = self.doc.get_dictionary(*dests_ref) else {
            return Ok(out);
        };
        for (name, value) in dests_dict.iter() {
            let Ok(arr) = value.as_array() else { continue };
            if arr.len() <= 4 || arr[1].as_name().ok() != Some(b"XYZ") {
                continue;
            }
            let Ok(page_ref) = arr[0].as_reference() else { continue };
            let Some(page) = self.page_number_of(page_ref) else { continue };
            let left = arr[2].as_float().unwrap_or(0.0) as f64;
            let top = arr[3].as_float().unwrap_or(0.0) as f64;
            let zoom = arr[4]
                .as_i64()
                .unwrap_or_else(|_| arr[4].as_float().unwrap_or(0.0) as i64);
            out.insert(String::from_utf8_lossy(name).into_owned(), Anchor { page, left, top, zoom });
        }
        Ok(out)
    }

    fn link_uri(&self, link_id: ObjectId) -> Option<String> {
        let dict = self.doc.get_dictionary(link_id).ok()?;
        let action = dict.get_deref(b"A", &self.doc).ok()?.as_dict().ok()?;
        let uri = action.get(b"URI").ok()?.as_str().ok()?;
        Some(String::from_utf8_lossy(uri).into_owned())
    }

    /// Port of `alter_links(callback, mark_links)`. For every URI-action
    /// link annotation: optionally marks it with a fixed border/color
    /// (`mark_links`), then calls `callback(uri)`; if it returns
    /// `Some((pagenum, left, top, zoom))` (**1-based** `pagenum`,
    /// matching upstream), the link's `/A` URI action is replaced with
    /// a direct `/XYZ`-style `/Dest`.
    pub fn alter_links(&mut self, mark_links: bool, mut callback: impl FnMut(&str) -> Option<(u32, f64, f64, f64)>) -> Result<()> {
        let link_ids: Vec<ObjectId> = self
            .doc
            .objects
            .iter()
            .filter_map(|(&id, obj)| {
                let dict = obj.as_dict().ok()?;
                if !is_name(dict, b"Type", b"Annot") || !is_name(dict, b"Subtype", b"Link") {
                    return None;
                }
                let action = dict.get_deref(b"A", &self.doc).ok()?.as_dict().ok()?;
                if !is_name(action, b"Type", b"Action") || !is_name(action, b"S", b"URI") {
                    return None;
                }
                action.get(b"URI").and_then(Object::as_str).ok()?;
                Some(id)
            })
            .collect();

        for link_id in link_ids {
            if mark_links {
                if let Ok(dict) = self.doc.get_dictionary_mut(link_id) {
                    dict.set(b"Border".to_vec(), Object::Array(vec![Object::Integer(16), Object::Integer(16), Object::Integer(1)]));
                    dict.set(
                        b"C".to_vec(),
                        Object::Array(vec![Object::Real(1.0), Object::Real(0.0), Object::Real(0.0)]),
                    );
                }
            }
            let Some(uri) = self.link_uri(link_id) else { continue };
            if let Some((pagenum, left, top, zoom)) = callback(&uri) {
                let page_id = self.page_id(pagenum)?;
                let dest = Object::Array(vec![
                    Object::Reference(page_id),
                    Object::Name(b"XYZ".to_vec()),
                    Object::Real(left as f32),
                    Object::Real(top as f32),
                    Object::Real(zoom as f32),
                ]);
                let dict = self.doc.get_dictionary_mut(link_id)?;
                dict.remove(b"A");
                dict.set(b"Dest".to_vec(), dest);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document};

    fn doc_with_pages(n: usize) -> Document {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..n {
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => Object::Reference(pages_id),
            });
            kids.push(Object::Reference(page_id));
        }
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Count" => n as i64,
                "Kids" => Object::Array(kids),
                "MediaBox" => make_box_array(0.0, 0.0, 612.0, 792.0),
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    fn round_trip(doc: Document) -> PdfDoc {
        let mut doc = doc;
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        PdfDoc::load(&buf).unwrap()
    }

    #[test]
    fn get_page_box_inherits_media_box_from_the_pages_tree() {
        let pdf = round_trip(doc_with_pages(2));
        let (l, b, w, h) = pdf.get_page_box("MediaBox", 1).unwrap();
        assert_eq!((l, b, w, h), (0.0, 0.0, 612.0, 792.0));
        // CropBox is absent everywhere -> falls back to MediaBox.
        assert_eq!(pdf.get_page_box("CropBox", 1).unwrap(), (0.0, 0.0, 612.0, 792.0));
        // TrimBox falls back to CropBox, which itself falls back to MediaBox.
        assert_eq!(pdf.get_page_box("TrimBox", 2).unwrap(), (0.0, 0.0, 612.0, 792.0));
    }

    #[test]
    fn set_page_box_writes_a_page_local_override_not_the_inherited_ancestor() {
        let mut pdf = round_trip(doc_with_pages(2));
        pdf.set_page_box("CropBox", 1, 10.0, 20.0, 100.0, 200.0).unwrap();
        assert_eq!(pdf.get_page_box("CropBox", 1).unwrap(), (10.0, 20.0, 100.0, 200.0));
        // The other page is untouched -- still falls back to the
        // inherited MediaBox, proving the set was page-local.
        assert_eq!(pdf.get_page_box("CropBox", 2).unwrap(), (0.0, 0.0, 612.0, 792.0));
    }

    #[test]
    fn set_box_is_zero_based_and_accepts_an_arbitrary_name() {
        let mut pdf = round_trip(doc_with_pages(2));
        pdf.set_box(0, "MyCustomBox", 1.0, 2.0, 3.0, 4.0).unwrap();
        // page_num=0 is the FIRST page -- verify via the 1-based getter.
        let page_id = pdf.doc.get_pages()[&1];
        let arr = pdf.doc.get_dictionary(page_id).unwrap().get(b"MyCustomBox").unwrap().as_array().unwrap();
        assert_eq!(parse_box_array(arr).unwrap(), (1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn unknown_box_name_errors() {
        let pdf = round_trip(doc_with_pages(1));
        assert!(matches!(pdf.get_page_box("BogusBox", 1), Err(PodofoError::UnknownBoxName(_))));
    }

    #[test]
    fn delete_pages_removes_consecutive_pages_and_updates_count() {
        let mut pdf = round_trip(doc_with_pages(5));
        pdf.delete_pages(2, 2).unwrap();
        assert_eq!(pdf.page_count(), 3);
        let pages_id = pdf.doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
        let count = pdf.doc.get_dictionary(pages_id).unwrap().get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn extract_first_page_truncates_to_one_page() {
        let mut pdf = round_trip(doc_with_pages(4));
        pdf.extract_first_page().unwrap();
        assert_eq!(pdf.page_count(), 1);
    }

    #[test]
    fn extract_anchors_reads_xyz_destinations_from_the_flat_dests_dict() {
        let mut doc = doc_with_pages(2);
        let page2_id = doc.get_pages()[&2];
        let dests_id = doc.add_object(dictionary! {
            "chapter2" => Object::Array(vec![
                Object::Reference(page2_id),
                Object::Name(b"XYZ".to_vec()),
                Object::Real(10.0),
                Object::Real(750.0),
                Object::Integer(2),
            ]),
            // A non-/XYZ shape must be silently skipped, not error.
            "ignored" => Object::Array(vec![
                Object::Reference(page2_id),
                Object::Name(b"Fit".to_vec()),
            ]),
        });
        doc.catalog_mut().unwrap().set("Dests", Object::Reference(dests_id));
        let pdf = round_trip(doc);

        let anchors = pdf.extract_anchors().unwrap();
        assert_eq!(anchors.len(), 1);
        let a = anchors.get("chapter2").unwrap();
        assert_eq!(*a, Anchor { page: 2, left: 10.0, top: 750.0, zoom: 2 });
    }

    #[test]
    fn alter_links_marks_and_rewrites_a_uri_link_to_an_internal_destination() {
        let mut doc = doc_with_pages(2);
        let page2_id = doc.get_pages()[&2];
        let link_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "A" => dictionary! {
                "Type" => "Action",
                "S" => "URI",
                "URI" => Object::string_literal("https://example.com/chapter2"),
            },
        });
        let mut pdf = PdfDoc { doc };

        let mut seen_uri = None;
        pdf.alter_links(true, |uri| {
            seen_uri = Some(uri.to_string());
            Some((2, 5.0, 6.0, 1.0))
        })
        .unwrap();
        assert_eq!(seen_uri.as_deref(), Some("https://example.com/chapter2"));

        let dict = pdf.doc.get_dictionary(link_id).unwrap();
        assert!(!dict.has(b"A"));
        assert_eq!(dict.get(b"Border").unwrap().as_array().unwrap().len(), 3);
        let dest = dict.get(b"Dest").unwrap().as_array().unwrap();
        assert_eq!(dest[0].as_reference().unwrap(), page2_id);
        assert_eq!(dest[1].as_name().unwrap(), b"XYZ");
    }

    #[test]
    fn alter_links_leaves_the_link_untouched_when_callback_declines() {
        let mut doc = doc_with_pages(1);
        let link_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "A" => dictionary! {
                "Type" => "Action",
                "S" => "URI",
                "URI" => Object::string_literal("https://example.com/"),
            },
        });
        let mut pdf = PdfDoc { doc };
        pdf.alter_links(false, |_uri| None).unwrap();
        let dict = pdf.doc.get_dictionary(link_id).unwrap();
        assert!(dict.has(b"A"));
        assert!(!dict.has(b"Dest"));
        assert!(!dict.has(b"Border"));
    }
}
