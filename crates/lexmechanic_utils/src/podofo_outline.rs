//! Port of `calibre.utils.podofo`'s outline (bookmark) tree support
//! (`outline.cpp`/`outlines.cpp`, issue #576), built against
//! [`crate::podofo`]'s `lopdf`-backed [`PdfDoc`] (issue #74's doc-core).
//!
//! Real upstream's own C++ files are thin `PyObject`-marshaling wrappers
//! around PoDoFo's own `PdfOutlines`/`PdfOutlineItem` C++ classes --
//! their actual tree-manipulation algorithm (the `/First`/`/Next`/
//! `/Parent`/`/Count` bookkeeping) lives inside the third-party PoDoFo
//! library itself, not in this repository, so it isn't a "translate
//! this file" port the way most of this codebase is. This module
//! implements that algorithm directly against the PDF outline object
//! model (ISO 32000-1 §12.3.3), which every PDF-outline implementation
//! (including PoDoFo's) is required to follow, and is verified against
//! real upstream's own *observable* behavior (`get_outline`/
//! `create_outline`/`PDFOutlineItem.create`/`.erase`'s Python-visible
//! signatures and the shape of dict `get_outline()` returns) rather
//! than against unavailable PoDoFo internals.
//!
//! # Disclosed simplifications
//!
//! - The real PDF outline spec allows a "closed" (collapsed) item, whose
//!   own `/Count` is negative and does not count its children into any
//!   ancestor's total. Nothing in the real Python-facing API this
//!   module ports (`create`/`erase`, no open/close toggle) ever
//!   produces a closed item, so every item here is always "open" and
//!   `/Count` bookkeeping only ever needs the simple positive-total
//!   case.
//! - [`PdfDoc::get_outline`]'s sibling walk is written as an iterative
//!   loop across `/Next` (each iteration recursing into `/First` for
//!   that one item's children) rather than upstream's `convert_outline`
//!   double-recursion (once into `/First`, once into `/Next`) -- the
//!   same tree walk, restructured to avoid one recursive call per
//!   sibling in a long chain; the resulting node list is identical.

use std::collections::{HashMap, HashSet};

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::podofo::{PdfDoc, PodofoError, Result};

/// A resolved `/XYZ` destination (`{page, top, left, zoom}`, matching
/// real `get_outline()`'s per-node `dest` dict shape). `page` is the
/// real upstream's `PdfPage::GetPageNumber()` value: the 1-based index
/// of the destination page within the document's own page tree, or
/// `-1` if the destination doesn't resolve to a real page (matching
/// upstream's own `page ? page->GetPageNumber() : -1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineDestination {
    pub page: i64,
    pub top: f64,
    pub left: f64,
    pub zoom: f64,
}

/// One outline (bookmark) tree node, port of `get_outline()`'s
/// per-item dict shape (`{title, dest?, children}`).
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineNode {
    pub title: String,
    pub dest: Option<OutlineDestination>,
    pub children: Vec<OutlineNode>,
}

/// Opaque handle to a real outline item's underlying PDF indirect
/// object, returned by the `create_*` methods so a caller can target it
/// with a later `create_outline_child`/`create_outline_sibling`/
/// `erase_outline_item` call -- mirrors real `PDFOutlineItem`'s role as
/// a live handle into the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineItemId(pub ObjectId);

impl PdfDoc {
    /// Port of `get_outline()`. Returns `None` if the document has no
    /// `/Outlines` root or that root has no children (matching
    /// upstream's own `if (!root || !root->First()) Py_RETURN_NONE`),
    /// else the top-level list of outline nodes -- already unwrapped
    /// from upstream's synthetic `{children: [...]}` root the way the
    /// real Python-level `get_outline()['children']` wrapper does.
    pub fn get_outline(&self) -> Option<Vec<OutlineNode>> {
        let catalog = self.doc.catalog().ok()?;
        let outlines_ref = catalog.get(b"Outlines").ok()?.as_reference().ok()?;
        let outlines = self.doc.get_dictionary(outlines_ref).ok()?;
        let first_ref = outlines.get(b"First").ok()?.as_reference().ok()?;
        let page_numbers = page_number_map(&self.doc);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.convert_outline_siblings(first_ref, &page_numbers, &mut seen, &mut out);
        Some(out)
    }

    fn convert_outline_siblings(
        &self,
        mut item_ref: ObjectId,
        page_numbers: &HashMap<ObjectId, i64>,
        seen: &mut HashSet<ObjectId>,
        out: &mut Vec<OutlineNode>,
    ) {
        loop {
            // Guards against a malformed/cyclic outline tree looping
            // forever -- real PoDoFo trusts a well-formed PDF, this
            // port doesn't need to.
            if !seen.insert(item_ref) {
                return;
            }
            let Ok(dict) = self.doc.get_dictionary(item_ref) else { return };
            let title = dict
                .get(b"Title")
                .ok()
                .and_then(|o| lopdf::decode_text_string(o).ok())
                .unwrap_or_default();
            let dest = self.parse_destination(dict, page_numbers);
            let mut node = OutlineNode { title, dest, children: Vec::new() };
            if let Ok(first_child) = dict.get(b"First").and_then(Object::as_reference) {
                self.convert_outline_siblings(first_child, page_numbers, seen, &mut node.children);
            }
            let next = dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());
            out.push(node);
            match next {
                Some(n) => item_ref = n,
                None => return,
            }
        }
    }

    fn parse_destination(&self, dict: &Dictionary, page_numbers: &HashMap<ObjectId, i64>) -> Option<OutlineDestination> {
        let dest_obj = dict.get(b"Dest").ok()?;
        let (_, resolved) = self.doc.dereference(dest_obj).ok()?;
        let arr = resolved.as_array().ok()?;
        let page_ref = arr.first()?.as_reference().ok()?;
        let page = page_numbers.get(&page_ref).copied().unwrap_or(-1);
        let num_at = |i: usize| -> f64 {
            match arr.get(i) {
                Some(Object::Integer(n)) => *n as f64,
                Some(Object::Real(f)) => *f as f64,
                _ => 0.0,
            }
        };
        Some(OutlineDestination {
            page,
            left: num_at(2),
            top: num_at(3),
            zoom: num_at(4),
        })
    }

    /// Port of `PDFDoc.create_outline(title, pagenum, left=0, top=0,
    /// zoom=0)`: creates a new top-level outline item, appended as the
    /// last existing top-level sibling (or as the tree's sole item if
    /// it has none yet).
    pub fn create_outline(&mut self, title: &str, pagenum: u32, left: f64, top: f64, zoom: f64) -> Result<OutlineItemId> {
        let outlines_id = get_or_create_outlines_dict(&mut self.doc)?;
        let page_id = self.page_object_id(pagenum)?;
        let item_id = self.doc.add_object(dictionary! {
            "Title" => lopdf::text_string(title),
            "Parent" => Object::Reference(outlines_id),
            "Dest" => dest_array(page_id, left, top, zoom),
            "Count" => 0,
        });
        self.link_as_last_child(outlines_id, item_id)?;
        increment_count_up_chain(&mut self.doc, outlines_id, 1)?;
        Ok(OutlineItemId(item_id))
    }

    /// Port of `PDFOutlineItem.create(title, pagenum, as_child=True)`.
    pub fn create_outline_child(
        &mut self,
        parent: OutlineItemId,
        title: &str,
        pagenum: u32,
        left: f64,
        top: f64,
        zoom: f64,
    ) -> Result<OutlineItemId> {
        let parent_id = parent.0;
        let page_id = self.page_object_id(pagenum)?;
        let item_id = self.doc.add_object(dictionary! {
            "Title" => lopdf::text_string(title),
            "Parent" => Object::Reference(parent_id),
            "Dest" => dest_array(page_id, left, top, zoom),
            "Count" => 0,
        });
        self.link_as_last_child(parent_id, item_id)?;
        increment_count_up_chain(&mut self.doc, parent_id, 1)?;
        Ok(OutlineItemId(item_id))
    }

    /// Port of `PDFOutlineItem.create(title, pagenum, as_child=False)`:
    /// inserts a new item immediately after `item` as its next sibling
    /// (same parent as `item`).
    pub fn create_outline_sibling(
        &mut self,
        item: OutlineItemId,
        title: &str,
        pagenum: u32,
        left: f64,
        top: f64,
        zoom: f64,
    ) -> Result<OutlineItemId> {
        let item_id = item.0;
        let item_dict = self.doc.get_dictionary(item_id)?;
        let parent_id = item_dict
            .get(b"Parent")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .ok_or(PodofoError::OutlineItemHasNoParent)?;
        let old_next = item_dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());

        let page_id = self.page_object_id(pagenum)?;
        let mut new_dict = dictionary! {
            "Title" => lopdf::text_string(title),
            "Parent" => Object::Reference(parent_id),
            "Dest" => dest_array(page_id, left, top, zoom),
            "Count" => 0,
            "Prev" => Object::Reference(item_id),
        };
        if let Some(next_id) = old_next {
            new_dict.set("Next", Object::Reference(next_id));
        }
        let new_id = self.doc.add_object(new_dict);

        if let Some(next_id) = old_next {
            self.doc.get_dictionary_mut(next_id)?.set("Prev", Object::Reference(new_id));
        } else {
            self.doc.get_dictionary_mut(parent_id)?.set("Last", Object::Reference(new_id));
        }
        self.doc.get_dictionary_mut(item_id)?.set("Next", Object::Reference(new_id));
        increment_count_up_chain(&mut self.doc, parent_id, 1)?;
        Ok(OutlineItemId(new_id))
    }

    /// Port of `PDFOutlineItem.erase()`: removes `item` and its entire
    /// subtree from the outline tree, fixing sibling/parent links and
    /// ancestor `/Count`s, then deletes the underlying PDF objects.
    pub fn erase_outline_item(&mut self, item: OutlineItemId) -> Result<()> {
        let item_id = item.0;
        let dict = self.doc.get_dictionary(item_id)?;
        let parent_id = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
        let prev_id = dict.get(b"Prev").ok().and_then(|o| o.as_reference().ok());
        let next_id = dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());
        let subtree_count = 1 + count_descendants(&self.doc, item_id);

        match (prev_id, next_id) {
            (Some(p), Some(n)) => {
                self.doc.get_dictionary_mut(p)?.set("Next", Object::Reference(n));
                self.doc.get_dictionary_mut(n)?.set("Prev", Object::Reference(p));
            }
            (Some(p), None) => {
                self.doc.get_dictionary_mut(p)?.remove(b"Next");
            }
            (None, Some(n)) => {
                self.doc.get_dictionary_mut(n)?.remove(b"Prev");
            }
            (None, None) => {}
        }

        if let Some(parent_id) = parent_id {
            let parent_dict = self.doc.get_dictionary(parent_id)?;
            let is_first = parent_dict.get(b"First").ok().and_then(|o| o.as_reference().ok()) == Some(item_id);
            let is_last = parent_dict.get(b"Last").ok().and_then(|o| o.as_reference().ok()) == Some(item_id);
            let parent_dict_mut = self.doc.get_dictionary_mut(parent_id)?;
            if is_first {
                match next_id {
                    Some(n) => parent_dict_mut.set("First", Object::Reference(n)),
                    None => {
                        parent_dict_mut.remove(b"First");
                    }
                }
            }
            if is_last {
                match prev_id {
                    Some(p) => parent_dict_mut.set("Last", Object::Reference(p)),
                    None => {
                        parent_dict_mut.remove(b"Last");
                    }
                }
            }
            increment_count_up_chain(&mut self.doc, parent_id, -subtree_count)?;
        }

        for id in collect_subtree_ids(&self.doc, item_id) {
            self.doc.delete_object(id);
        }
        Ok(())
    }

    fn page_object_id(&self, pagenum: u32) -> Result<ObjectId> {
        self.doc
            .get_pages()
            .get(&pagenum)
            .copied()
            .ok_or(PodofoError::InvalidPage(pagenum))
    }

    fn link_as_last_child(&mut self, parent_id: ObjectId, item_id: ObjectId) -> Result<()> {
        let last = self
            .doc
            .get_dictionary(parent_id)?
            .get(b"Last")
            .ok()
            .and_then(|o| o.as_reference().ok());
        if let Some(last_id) = last {
            self.doc.get_dictionary_mut(last_id)?.set("Next", Object::Reference(item_id));
            self.doc.get_dictionary_mut(item_id)?.set("Prev", Object::Reference(last_id));
        } else {
            self.doc.get_dictionary_mut(parent_id)?.set("First", Object::Reference(item_id));
        }
        self.doc.get_dictionary_mut(parent_id)?.set("Last", Object::Reference(item_id));
        Ok(())
    }
}

fn page_number_map(doc: &Document) -> HashMap<ObjectId, i64> {
    doc.get_pages().into_iter().map(|(num, id)| (id, num as i64)).collect()
}

fn get_or_create_outlines_dict(doc: &mut Document) -> Result<ObjectId> {
    if let Ok(existing) = doc.catalog()?.get(b"Outlines").and_then(Object::as_reference) {
        return Ok(existing);
    }
    let outlines_id = doc.add_object(dictionary! {
        "Type" => "Outlines",
        "Count" => 0,
    });
    doc.catalog_mut()?.set("Outlines", Object::Reference(outlines_id));
    Ok(outlines_id)
}

fn dest_array(page_id: ObjectId, left: f64, top: f64, zoom: f64) -> Object {
    Object::Array(vec![
        Object::Reference(page_id),
        Object::Name(b"XYZ".to_vec()),
        Object::Real(left as f32),
        Object::Real(top as f32),
        Object::Real(zoom as f32),
    ])
}

/// Adds `delta` to `/Count` on `start_id` and every ancestor reachable
/// via `/Parent` (inclusive of `start_id`), stopping at the outline
/// root (which has no `/Parent`). See the module docs for why this
/// never needs the PDF spec's "closed item" negative-count case.
fn increment_count_up_chain(doc: &mut Document, start_id: ObjectId, delta: i64) -> Result<()> {
    let mut current = Some(start_id);
    while let Some(id) = current {
        let existing = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Count").ok())
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        doc.get_dictionary_mut(id)?.set("Count", Object::Integer(existing + delta));
        current = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Parent").ok())
            .and_then(|o| o.as_reference().ok());
    }
    Ok(())
}

fn count_descendants(doc: &Document, id: ObjectId) -> i64 {
    let Ok(dict) = doc.get_dictionary(id) else { return 0 };
    match dict.get(b"First").ok().and_then(|o| o.as_reference().ok()) {
        Some(first) => count_subtree_siblings(doc, first),
        None => 0,
    }
}

fn count_subtree_siblings(doc: &Document, mut id: ObjectId) -> i64 {
    let mut total = 0;
    loop {
        total += 1 + count_descendants(doc, id);
        let next = doc.get_dictionary(id).ok().and_then(|d| d.get(b"Next").ok()).and_then(|o| o.as_reference().ok());
        match next {
            Some(n) => id = n,
            None => break,
        }
    }
    total
}

fn collect_subtree_ids(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    let mut out = vec![id];
    if let Ok(dict) = doc.get_dictionary(id) {
        if let Ok(first) = dict.get(b"First").and_then(Object::as_reference) {
            let mut next = Some(first);
            while let Some(n) = next {
                out.extend(collect_subtree_ids(doc, n));
                next = doc.get_dictionary(n).ok().and_then(|d| d.get(b"Next").ok()).and_then(|o| o.as_reference().ok());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn get_outline_is_none_with_no_outlines_dict() {
        let doc = round_trip(doc_with_pages(1));
        assert!(doc.get_outline().is_none());
    }

    #[test]
    fn create_outline_then_read_it_back() {
        let mut pdf = PdfDoc { doc: doc_with_pages(3) };
        pdf.create_outline("Chapter One", 1, 0.0, 792.0, 0.0).unwrap();
        pdf.create_outline("Chapter Two", 2, 0.0, 792.0, 0.0).unwrap();

        let pdf = round_trip(pdf.doc);
        let outline = pdf.get_outline().unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Chapter One");
        assert_eq!(outline[0].dest.unwrap().page, 1);
        assert_eq!(outline[1].title, "Chapter Two");
        assert_eq!(outline[1].dest.unwrap().page, 2);
        assert!(outline[0].children.is_empty());
    }

    #[test]
    fn create_outline_child_and_sibling_build_a_real_tree() {
        let mut pdf = PdfDoc { doc: doc_with_pages(3) };
        let root = pdf.create_outline("Part One", 1, 0.0, 0.0, 0.0).unwrap();
        let child1 = pdf.create_outline_child(root, "Section 1.1", 1, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_sibling(child1, "Section 1.2", 2, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline("Part Two", 3, 0.0, 0.0, 0.0).unwrap();

        let pdf = round_trip(pdf.doc);
        let outline = pdf.get_outline().unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Part One");
        assert_eq!(outline[0].children.len(), 2);
        assert_eq!(outline[0].children[0].title, "Section 1.1");
        assert_eq!(outline[0].children[1].title, "Section 1.2");
        assert_eq!(outline[1].title, "Part Two");
    }

    #[test]
    fn create_outline_count_bookkeeping() {
        let mut pdf = PdfDoc { doc: doc_with_pages(2) };
        let root = pdf.create_outline("Part One", 1, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_child(root, "Section 1.1", 1, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_child(root, "Section 1.2", 2, 0.0, 0.0, 0.0).unwrap();

        let outlines_id = pdf.doc.catalog().unwrap().get(b"Outlines").unwrap().as_reference().unwrap();
        let root_count = pdf.doc.get_dictionary(outlines_id).unwrap().get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(root_count, 3); // root item + its 2 children

        let root_dict = pdf.doc.get_dictionary(root.0).unwrap();
        let root_item_count = root_dict.get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(root_item_count, 2); // root item's own 2 children
    }

    #[test]
    fn erase_leaf_item_fixes_sibling_links_and_counts() {
        let mut pdf = PdfDoc { doc: doc_with_pages(3) };
        let root = pdf.create_outline("Part One", 1, 0.0, 0.0, 0.0).unwrap();
        let child1 = pdf.create_outline_child(root, "Section 1.1", 1, 0.0, 0.0, 0.0).unwrap();
        let child2 = pdf.create_outline_sibling(child1, "Section 1.2", 2, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_sibling(child2, "Section 1.3", 3, 0.0, 0.0, 0.0).unwrap();

        pdf.erase_outline_item(child2).unwrap();

        let outlines_id = pdf.doc.catalog().unwrap().get(b"Outlines").unwrap().as_reference().unwrap();
        let root_count = pdf.doc.get_dictionary(outlines_id).unwrap().get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(root_count, 3); // root item + 2 remaining children

        let pdf = round_trip(pdf.doc);
        let outline = pdf.get_outline().unwrap();
        assert_eq!(outline[0].children.len(), 2);
        assert_eq!(outline[0].children[0].title, "Section 1.1");
        assert_eq!(outline[0].children[1].title, "Section 1.3");
    }

    #[test]
    fn erase_first_child_updates_parent_first_pointer() {
        let mut pdf = PdfDoc { doc: doc_with_pages(2) };
        let root = pdf.create_outline("Part One", 1, 0.0, 0.0, 0.0).unwrap();
        let child1 = pdf.create_outline_child(root, "Section 1.1", 1, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_sibling(child1, "Section 1.2", 2, 0.0, 0.0, 0.0).unwrap();

        pdf.erase_outline_item(child1).unwrap();

        let pdf = round_trip(pdf.doc);
        let outline = pdf.get_outline().unwrap();
        assert_eq!(outline[0].children.len(), 1);
        assert_eq!(outline[0].children[0].title, "Section 1.2");
    }

    #[test]
    fn erase_subtree_removes_all_descendants() {
        let mut pdf = PdfDoc { doc: doc_with_pages(2) };
        let root = pdf.create_outline("Part One", 1, 0.0, 0.0, 0.0).unwrap();
        let child = pdf.create_outline_child(root, "Section 1.1", 1, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline_child(child, "Sub 1.1.1", 2, 0.0, 0.0, 0.0).unwrap();
        pdf.create_outline("Part Two", 2, 0.0, 0.0, 0.0).unwrap();

        pdf.erase_outline_item(root).unwrap();

        let outlines_id = pdf.doc.catalog().unwrap().get(b"Outlines").unwrap().as_reference().unwrap();
        let root_count = pdf.doc.get_dictionary(outlines_id).unwrap().get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(root_count, 1); // only "Part Two" remains

        let pdf = round_trip(pdf.doc);
        let outline = pdf.get_outline().unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].title, "Part Two");
    }

    #[test]
    fn create_outline_rejects_invalid_page_number() {
        let mut pdf = PdfDoc { doc: doc_with_pages(1) };
        let err = pdf.create_outline("X", 99, 0.0, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, PodofoError::InvalidPage(99)));
    }
}
