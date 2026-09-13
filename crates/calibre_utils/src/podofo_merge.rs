//! Port of `calibre.utils.podofo`'s cross-document page merging
//! (`doc.cpp`'s `copy_page`/`insert_existing_page`/`append`, issue
//! #669, split from #578), built against [`crate::podofo`]'s
//! `lopdf`-backed [`PdfDoc`] (issue #74's doc-core).
//!
//! # The shared primitive
//!
//! All three real operations need the same underlying capability: copy
//! a set of indirect objects from one document's object table into
//! another's, with fresh object numbers, and rewrite every internal
//! reference throughout the copied objects so they point at each
//! other's *new* numbers instead of the stale source ones. This module
//! builds that once ([`copy_objects_into`]/[`remap_objects`]) and has
//! every real operation call it, rather than -- like real upstream's
//! own `append()` -- hand-rolling the reference-fixup walk inline.
//!
//! `copy_page`/`insert_existing_page` and `append` use this primitive
//! with **different object-selection strategies**, matching their real,
//! different upstream semantics:
//! - `copy_page`/`insert_existing_page` copy only the objects
//!   **reachable from the one page being copied** (its own dict,
//!   content stream(s), resources, fonts, images, ... -- whatever that
//!   page's own subgraph touches), *excluding* the page's own `/Parent`
//!   link (which points into the *source* document's page tree and
//!   must not be followed). Real upstream's `InsertDocumentPageAt` is
//!   opaque PoDoFo, but this is the natural, spec-correct scope for
//!   "copy one page" and matches this issue's own suggestion to share
//!   one primitive across all three operations.
//! - `append` copies **every single indirect object in the source
//!   document**, with no reachability pruning at all -- this is real,
//!   observed upstream behavior (see the module doc on
//!   [`PdfDoc::append`]), not a simplification.
//!
//! # Disclosed, not silently reproduced
//!
//! - Real upstream's `append()` has a dead `RemoveKey("Resource")`
//!   line (singular -- the real PDF key is `"Resources"`, so this
//!   never matches anything on a real page dict). Omitted here rather
//!   than reproduced, since reproducing a provably-inert no-op adds
//!   nothing but a mysterious comment.
//! - `PdfReference` bucketing that ignores generation number (real
//!   upstream's `PdfReferenceHasher`) is replicated by using a plain
//!   [`ObjectId`] (object number *and* generation) as the map key
//!   instead -- strictly more precise, and observably identical for
//!   any real PDF that hasn't had its generation numbers deliberately
//!   tampered with (which essentially never happens in practice).

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::podofo::{PdfDoc, PodofoError, Result};

/// The 4 real page attributes that are spec-inheritable from ancestor
/// `/Pages` nodes and that `append`/`copy_page`/`insert_existing_page`
/// flatten directly onto every copied page, so it renders correctly
/// regardless of whether the destination's own page tree happens to
/// provide compatible inherited defaults.
const INHERITABLE_PAGE_ATTRS: &[&[u8]] = &[b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn collect_refs(obj: &Object, out: &mut Vec<ObjectId>) {
    match obj {
        Object::Reference(id) => out.push(*id),
        Object::Array(arr) => arr.iter().for_each(|o| collect_refs(o, out)),
        Object::Dictionary(dict) => dict.iter().for_each(|(_, v)| collect_refs(v, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect_refs(v, out)),
        _ => {}
    }
}

/// Rewrites every [`Object::Reference`] found anywhere inside `obj`
/// (recursing through arrays/dictionaries/stream dictionaries) using
/// `ref_map`, leaving anything not in the map untouched.
fn remap_references(obj: &mut Object, ref_map: &HashMap<ObjectId, ObjectId>) {
    match obj {
        Object::Reference(id) => {
            if let Some(&new_id) = ref_map.get(id) {
                *id = new_id;
            }
        }
        Object::Array(arr) => arr.iter_mut().for_each(|o| remap_references(o, ref_map)),
        Object::Dictionary(dict) => dict.iter_mut().for_each(|(_, v)| remap_references(v, ref_map)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, v)| remap_references(v, ref_map)),
        _ => {}
    }
}

/// Every object id reachable from `root` (inclusive), not following
/// `root`'s own `/Parent` key (see the module doc on why).
fn reachable_from_page(doc: &Document, root: ObjectId) -> Vec<ObjectId> {
    let mut seen = HashSet::new();
    seen.insert(root);
    let mut stack = Vec::new();
    if let Ok(dict) = doc.get_dictionary(root) {
        for (key, value) in dict.iter() {
            if key == b"Parent" {
                continue;
            }
            collect_refs(value, &mut stack);
        }
    }
    let mut order = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        order.push(id);
        if let Ok(obj) = doc.get_object(id) {
            collect_refs(obj, &mut stack);
        }
    }
    order
}

/// Copies each of `object_ids` from `src` into `dest` as a brand-new
/// object, then rewrites every reference among the newly-copied objects
/// to point at each other's new ids. Returns the old-id -> new-id map.
fn copy_objects_into(dest: &mut Document, src: &Document, object_ids: &[ObjectId]) -> HashMap<ObjectId, ObjectId> {
    let cloned: Vec<(ObjectId, Object)> = object_ids.iter().filter_map(|&id| src.get_object(id).ok().map(|o| (id, o.clone()))).collect();
    let mut ref_map = HashMap::with_capacity(cloned.len());
    for (old_id, obj) in &cloned {
        let new_id = dest.add_object(obj.clone());
        ref_map.insert(*old_id, new_id);
    }
    remap_objects(dest, ref_map.values().copied(), &ref_map);
    ref_map
}

fn remap_objects(dest: &mut Document, ids: impl Iterator<Item = ObjectId>, ref_map: &HashMap<ObjectId, ObjectId>) {
    for id in ids {
        if let Ok(obj) = dest.get_object_mut(id) {
            remap_references(obj, ref_map);
        }
    }
}

/// Walks `page_id` then its `/Parent` chain looking for `key`, matching
/// real PoDoFo's `FindKeyParent` -- the generic "resolve this page
/// attribute, following spec inheritance" lookup (`podofo_pages`'s own
/// box-specific inheritance walk follows the same shape).
fn find_inherited_value(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = page_id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let dict = doc.get_dictionary(cur).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        cur = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
}

/// Finalizes a page just copied into `dest` (whether via the
/// reachability-based single-page copy or `append`'s whole-document
/// copy): flattens the 4 inheritable attributes directly onto the page
/// dict when not already present there, and guarantees `/Contents`
/// exists (matching upstream's own "prevent segfaults in other code
/// that assumes it" robustness fixup for source pages with no content
/// at all).
fn finalize_copied_page(dest: &mut Document, src: &Document, src_page_id: ObjectId, new_page_id: ObjectId, ref_map: &HashMap<ObjectId, ObjectId>) -> Result<()> {
    for &attr in INHERITABLE_PAGE_ATTRS {
        let already_present = dest.get_dictionary(new_page_id)?.has(attr);
        if already_present {
            continue;
        }
        if let Some(mut value) = find_inherited_value(src, src_page_id, attr) {
            remap_references(&mut value, ref_map);
            dest.get_dictionary_mut(new_page_id)?.set(attr.to_vec(), value);
        }
    }
    if !dest.get_dictionary(new_page_id)?.has(b"Contents") {
        let empty = dest.add_object(Stream::new(Dictionary::new(), Vec::new()));
        dest.get_dictionary_mut(new_page_id)?.set(b"Contents".to_vec(), Object::Reference(empty));
    }
    Ok(())
}

/// Attaches `page_id` into `doc`'s page tree so it becomes page number
/// `at` (1-based), incrementing `/Count` up the ancestor chain.
/// Shared by [`PdfDoc::copy_page`], [`PdfDoc::insert_existing_page`],
/// and [`PdfDoc::append`] (which always inserts at the current end).
fn insert_page_at(doc: &mut Document, page_id: ObjectId, at: u32) -> Result<()> {
    let pages = doc.get_pages();
    let (parent_id, insert_idx) = if let Some(&anchor) = pages.get(&at) {
        let parent = doc
            .get_dictionary(anchor)?
            .get(b"Parent")
            .and_then(Object::as_reference)
            .map_err(|_| PodofoError::InvalidPage(at))?;
        let idx = doc
            .get_dictionary(parent)?
            .get(b"Kids")?
            .as_array()?
            .iter()
            .position(|o| o.as_reference().ok() == Some(anchor))
            .unwrap_or(0);
        (parent, idx)
    } else if let Some((_, &last)) = pages.iter().next_back() {
        let parent = last_page_parent(doc, last)?;
        let idx = doc.get_dictionary(parent)?.get(b"Kids")?.as_array()?.len();
        (parent, idx)
    } else {
        let root = doc.catalog()?.get(b"Pages").and_then(Object::as_reference)?;
        (root, 0)
    };

    doc.get_dictionary_mut(page_id)?.set(b"Parent".to_vec(), Object::Reference(parent_id));
    if let Ok(kids) = doc.get_dictionary_mut(parent_id)?.get_mut(b"Kids").and_then(Object::as_array_mut) {
        kids.insert(insert_idx.min(kids.len()), Object::Reference(page_id));
    }

    let mut cur = Some(parent_id);
    let mut seen = HashSet::new();
    while let Some(id) = cur {
        if !seen.insert(id) {
            break;
        }
        cur = match doc.get_dictionary_mut(id) {
            Ok(dict) => {
                if let Ok(Object::Integer(count)) = dict.get_mut(b"Count") {
                    *count += 1;
                }
                dict.get(b"Parent").and_then(Object::as_reference).ok()
            }
            Err(_) => None,
        };
    }
    Ok(())
}

fn last_page_parent(doc: &Document, page_id: ObjectId) -> Result<ObjectId> {
    doc.get_dictionary(page_id)?
        .get(b"Parent")
        .and_then(Object::as_reference)
        .map_err(Into::into)
}

impl PdfDoc {
    /// Copies a single page's whole reachable object graph
    /// (everything `[`reachable_from_page`] finds, i.e. its own dict,
    /// content stream(s), resources, and everything those transitively
    /// reference) from `src` into `self`, and returns the new page's
    /// object id along with the old-id -> new-id map (needed by
    /// [`Self::append`], which finalizes several pages against one
    /// shared map).
    fn copy_page_graph(&mut self, src: &Document, src_page_id: ObjectId) -> Result<(ObjectId, HashMap<ObjectId, ObjectId>)> {
        let ids = reachable_from_page(src, src_page_id);
        let ref_map = copy_objects_into(&mut self.doc, src, &ids);
        let new_page_id = *ref_map.get(&src_page_id).ok_or(PodofoError::InvalidPage(0))?;
        self.doc.get_dictionary_mut(new_page_id)?.remove(b"Parent");
        finalize_copied_page(&mut self.doc, src, src_page_id, new_page_id, &ref_map)?;
        Ok((new_page_id, ref_map))
    }

    /// Port of `copy_page(from, to)`. Same-document deep-copy-and-
    /// insert: **1-based** `from`/`to`.
    pub fn copy_page(&mut self, from: u32, to: u32) -> Result<()> {
        let from_id = self.page_id(from)?;
        // `src` and `dest` are the same document here, which the
        // borrow checker can't express directly against `&mut self`.
        // A full `Document::clone()` just to read from while mutating
        // `self.doc` is real, wasted work for a large PDF -- accepted
        // as a pragmatic simplification for this one same-document
        // case (real upstream's own `InsertDocumentPageAt(dest_idx,
        // *self, src_idx)` doesn't have this problem in C++, since
        // nothing there stops it reading and mutating the same object
        // through two different pointers).
        let src_snapshot = self.doc.clone();
        let (new_page_id, _) = self.copy_page_graph(&src_snapshot, from_id)?;
        insert_page_at(&mut self.doc, new_page_id, to)
    }

    /// Port of `insert_existing_page(src_doc, src_page, at)`. Cross-
    /// document version of [`Self::copy_page`]. **0-based** `src_page`/
    /// `at`, matching upstream's own real (and, compared to most other
    /// page ops in this file, unusually low-level) convention here.
    pub fn insert_existing_page(&mut self, src: &PdfDoc, src_page: u32, at: u32) -> Result<()> {
        let src_page_id = src
            .doc
            .get_pages()
            .get(&(src_page + 1))
            .copied()
            .ok_or(PodofoError::InvalidPage(src_page))?;
        let (new_page_id, _) = self.copy_page_graph(&src.doc, src_page_id)?;
        insert_page_at(&mut self.doc, new_page_id, at + 1)
    }

    /// Port of `append(*other_docs)`: appends every page of each
    /// `other` document, in order, onto the end of `self`. Real
    /// upstream copies **every** indirect object in each source
    /// document's object table into dest -- not just objects reachable
    /// from its pages -- so any object the source document doesn't
    /// itself reference from a page comes along too (no reachability
    /// pruning; matches upstream, see the module doc).
    pub fn append(&mut self, others: &[&PdfDoc]) -> Result<()> {
        for other in others {
            self.append_one(&other.doc)?;
        }
        Ok(())
    }

    fn append_one(&mut self, src: &Document) -> Result<()> {
        let all_src_ids: Vec<ObjectId> = src.objects.keys().copied().collect();
        let src_pages = src.get_pages();
        let ref_map = copy_objects_into(&mut self.doc, src, &all_src_ids);

        for &src_page_id in src_pages.values() {
            let Some(&new_page_id) = ref_map.get(&src_page_id) else { continue };
            self.doc.get_dictionary_mut(new_page_id)?.remove(b"Parent");
            finalize_copied_page(&mut self.doc, src, src_page_id, new_page_id, &ref_map)?;
            let at = self.page_count() as u32 + 1;
            insert_page_at(&mut self.doc, new_page_id, at)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn doc_with_pages(n: usize) -> Document {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for i in 0..n {
            let content_id = doc.add_object(Stream::new(Dictionary::new(), format!("BT ({i}) Tj ET").into_bytes()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => Object::Reference(pages_id),
                "Contents" => Object::Reference(content_id),
            });
            kids.push(Object::Reference(page_id));
        }
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Count" => n as i64,
                "Kids" => Object::Array(kids),
                "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)]),
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

    fn page_text(pdf: &PdfDoc, pagenum: u32) -> String {
        let page_id = pdf.doc.get_pages()[&pagenum];
        let content_id = pdf.doc.get_dictionary(page_id).unwrap().get(b"Contents").unwrap().as_reference().unwrap();
        let bytes = pdf.doc.get_object(content_id).unwrap().as_stream().unwrap().content.clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn copy_page_duplicates_a_page_within_the_same_document() {
        let mut pdf = round_trip(doc_with_pages(2));
        pdf.copy_page(1, 3).unwrap();
        assert_eq!(pdf.page_count(), 3);
        // The copy at position 3 has the SAME content as page 1...
        assert_eq!(page_text(&pdf, 1), page_text(&pdf, 3));
        // ...but is a genuinely independent object, not a shared reference.
        let p1 = pdf.doc.get_pages()[&1];
        let p3 = pdf.doc.get_pages()[&3];
        assert_ne!(p1, p3);
    }

    #[test]
    fn copy_page_inherits_media_box_from_the_source_pages_tree() {
        let mut pdf = round_trip(doc_with_pages(1));
        pdf.copy_page(1, 2).unwrap();
        let (l, b, w, h) = pdf.get_page_box("MediaBox", 2).unwrap();
        assert_eq!((l, b, w, h), (0.0, 0.0, 612.0, 792.0));
    }

    #[test]
    fn insert_existing_page_copies_across_documents_zero_based() {
        let src = round_trip(doc_with_pages(3));
        let mut dest = round_trip(doc_with_pages(1));
        // 0-based: src page index 1 ("page 2" in 1-based terms) goes to dest index 1.
        dest.insert_existing_page(&src, 1, 1).unwrap();
        assert_eq!(dest.page_count(), 2);
        assert_eq!(page_text(&dest, 2), page_text(&src, 2));
    }

    #[test]
    fn append_adds_every_page_in_order_at_the_end() {
        let mut dest = round_trip(doc_with_pages(1));
        let src1 = round_trip(doc_with_pages(2));
        let src2 = round_trip(doc_with_pages(1));
        dest.append(&[&src1, &src2]).unwrap();
        assert_eq!(dest.page_count(), 4);
        assert_eq!(page_text(&dest, 1), page_text(&round_trip(doc_with_pages(1)), 1));
        assert_eq!(page_text(&dest, 2), page_text(&src1, 1));
        assert_eq!(page_text(&dest, 3), page_text(&src1, 2));
        assert_eq!(page_text(&dest, 4), page_text(&src2, 1));
    }

    #[test]
    fn append_flattens_inherited_attributes_onto_every_appended_page() {
        let mut dest = round_trip(doc_with_pages(1));
        let src = round_trip(doc_with_pages(2));
        dest.append(&[&src]).unwrap();
        for pagenum in [2, 3] {
            let page_id = dest.doc.get_pages()[&pagenum];
            assert!(dest.doc.get_dictionary(page_id).unwrap().has(b"MediaBox"), "page {pagenum} should have a flattened MediaBox");
        }
    }

    #[test]
    fn append_gives_appended_pages_a_working_dest_side_parent() {
        let mut dest = round_trip(doc_with_pages(1));
        let src = round_trip(doc_with_pages(1));
        dest.append(&[&src]).unwrap();
        let dest_pages_root = dest.doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
        let appended_page = dest.doc.get_pages()[&2];
        let parent = dest.doc.get_dictionary(appended_page).unwrap().get(b"Parent").unwrap().as_reference().unwrap();
        assert_eq!(parent, dest_pages_root);
        let count = dest.doc.get_dictionary(dest_pages_root).unwrap().get(b"Count").unwrap().as_i64().unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn append_ensures_contents_exists_even_for_a_contentless_source_page() {
        let mut src_doc = Document::with_version("1.4");
        let page_id = src_doc.add_object(dictionary! { "Type" => "Page" });
        let pages_id = src_doc.add_object(dictionary! {
            "Type" => "Pages", "Count" => 1, "Kids" => Object::Array(vec![Object::Reference(page_id)]),
        });
        src_doc.get_dictionary_mut(page_id).unwrap().set("Parent", Object::Reference(pages_id));
        let catalog_id = src_doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
        src_doc.trailer.set("Root", Object::Reference(catalog_id));
        let src = round_trip(src_doc);

        let mut dest = round_trip(doc_with_pages(0));
        dest.append(&[&src]).unwrap();
        let new_page = dest.doc.get_pages()[&1];
        assert!(dest.doc.get_dictionary(new_page).unwrap().has(b"Contents"));
    }

    #[test]
    fn copy_page_does_not_pull_in_the_whole_source_document() {
        // A doc with 2 unrelated pages: copying just page 1 into dest
        // must not also drag page 2's own content stream along -- only
        // 2 new objects (the page + its one content stream) should be
        // added, not a copy of the whole 2-page source document.
        let src = round_trip(doc_with_pages(2));
        let mut dest = round_trip(doc_with_pages(0));
        let before = dest.doc.objects.len();
        dest.insert_existing_page(&src, 0, 0).unwrap();
        assert_eq!(dest.doc.objects.len(), before + 2, "unexpected objects: {:?}", dest.doc.objects.keys().collect::<Vec<_>>());
    }
}
