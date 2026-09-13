//! Port of `calibre.utils.podofo`'s image deduplication (issue #671,
//! split from #578), built against [`crate::podofo`]'s `lopdf`-backed
//! [`PdfDoc`] (issue #74's doc-core).
//!
//! # A file/content mismatch worth flagging (inherited from research, not this port)
//!
//! Real upstream's C++ file physically named `images.cpp` does **not**
//! contain `image_count()`/`add_image_page()` (those live in `doc.cpp`
//! -- see [`crate::podofo::PdfDoc::image_count`]) -- its real content,
//! and its own header comment even mislabels itself as "impose.cpp",
//! is exactly the `dedup_images()` this module ports.
//!
//! # Structurally identical to `dedup_type3_fonts` (issue #577, CLOSED)
//!
//! Both are "value equality + hash + first-seen-canonical + reference-
//! remap" dedup passes over a class of indirect objects. The real
//! difference is what "equal" means (byte-identical decoded CharProc
//! stream vs. same width/height/SMask-*reference* + byte-identical
//! image stream) and which resource-dictionary keys need fixing up
//! afterward (`CharProcs` vs. any `/Resources/XObject` entry plus any
//! `/SMask` reference).
//!
//! # The real, deliberate 2-pass structure
//!
//! Two images with identical pixel data but referencing two different
//! (but themselves content-identical) SMask objects are **not**
//! considered equal on a first pass, since equality compares the SMask
//! **reference**, not its content. Real upstream runs the whole dedup
//! pass exactly **twice**, sharing nothing but the object table between
//! passes: pass 1 merges images with identical pixel data regardless of
//! differing-but-duplicate SMask refs (impossible on pass 1, since SMask
//! refs haven't been merged yet -- so pass 1 only catches images that
//! already shared the exact same SMask reference, or had none); pass 2
//! then catches images that *now* match because pass 1 already merged
//! their SMask references down to one canonical id. This is a
//! deliberate, hardcoded 2-pass limit, not a fixed point (looping until
//! a pass finds nothing new would be a strict, harmless improvement --
//! not done here, to preserve upstream's own observed behavior).
//!
//! # Disclosed simplification: raw stream bytes, not decoded pixels
//!
//! Real upstream's `Image` value class compares **decoded** stream
//! bytes (`GetCopySafe()` -- PoDoFo's own general stream decoder,
//! independent of the image's specific pixel encoding). This port
//! compares the **raw, as-stored** stream bytes instead: fully decoding
//! every real-world image filter (`DCTDecode`/JPXDecode`/
//! `CCITTFaxDecode`, ...) would need a general image-codec dependency
//! this crate doesn't otherwise need, just to compute a dedup key. Two
//! images with byte-identical raw encoded data are still correctly
//! recognized as duplicates (the common real case -- the same image
//! embedded twice); two images that encode identical pixels via
//! different byte streams (e.g. re-compressed at a different point)
//! are not, which only reduces the dedup *rate*, not correctness (no
//! two distinct images are ever merged).
//!
//! # Two real `lopdf` gotchas found building this (not upstream quirks)
//!
//! - [`lopdf::Document::delete_object`] is not a raw removal: it
//!   *already* walks every object in the document and strips any dict
//!   key/array element that still references the deleted id. Deleting a
//!   duplicate image *before* redirecting the references that point at
//!   it would therefore silently delete those `/Resources/XObject`
//!   entries (and any `/SMask` key) instead of retargeting them at the
//!   canonical image -- the reference-remap sweep in
//!   [`PdfDoc::run_one_dedup_pass`] runs *before* the deletions for
//!   exactly this reason.
//! - An image XObject's own `/SMask` key lives on its **stream**
//!   dictionary (`Object::Stream(Stream { dict, .. })`), which
//!   [`lopdf::Document::get_dictionary_mut`] can't reach -- it only
//!   matches `Object::Dictionary`. The remap sweep uses
//!   [`object_dict_mut`] (matching either shape) instead, or an
//!   image's own `/SMask` reference to another now-merged image would
//!   silently never get updated.

use std::collections::HashMap;

use lopdf::{Dictionary, Object, ObjectId};

use crate::podofo::{is_name, PdfDoc, Result};

/// An image XObject is a *stream* object (`Object::Stream`), not a
/// plain dictionary -- [`Object::as_dict`] only matches
/// [`Object::Dictionary`], so a `/Type`/`/Subtype` check needs this
/// instead to look at either shape's dictionary.
fn object_dict(obj: &Object) -> Option<&Dictionary> {
    match obj {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// Mutable counterpart of [`object_dict`] -- needed because an image
/// XObject's own `/SMask` key lives on its *stream* dict, which
/// [`lopdf::Document::get_dictionary_mut`] can't reach (it only matches
/// [`Object::Dictionary`], not [`Object::Stream`]).
fn object_dict_mut(obj: &mut Object) -> Option<&mut Dictionary> {
    match obj {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&mut stream.dict),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ImageKey {
    width: i64,
    height: i64,
    smask: Option<ObjectId>,
    content: Vec<u8>,
}

impl PdfDoc {
    fn image_key(&self, id: ObjectId) -> Option<ImageKey> {
        let stream = self.doc.get_object(id).ok()?.as_stream().ok()?;
        let width = stream.dict.get(b"Width").and_then(Object::as_i64).ok()?;
        let height = stream.dict.get(b"Height").and_then(Object::as_i64).ok()?;
        let smask = stream.dict.get(b"SMask").and_then(Object::as_reference).ok();
        Some(ImageKey { width, height, smask, content: stream.content.clone() })
    }

    fn run_one_dedup_pass(&mut self) -> u64 {
        let image_ids: Vec<ObjectId> = self
            .doc
            .objects
            .iter()
            .filter_map(|(&id, obj)| {
                object_dict(obj)
                    .filter(|d| is_name(d, b"Type", b"XObject") && is_name(d, b"Subtype", b"Image"))
                    .map(|_| id)
            })
            .collect();

        let mut canonical_by_key: HashMap<ImageKey, ObjectId> = HashMap::new();
        let mut ref_map: HashMap<ObjectId, ObjectId> = HashMap::new();
        let mut to_delete: Vec<ObjectId> = Vec::new();

        for &id in &image_ids {
            let Some(key) = self.image_key(id) else { continue };
            match canonical_by_key.get(&key) {
                Some(&canonical) if canonical != id => {
                    ref_map.insert(id, canonical);
                    to_delete.push(id);
                }
                Some(_) => {}
                None => {
                    canonical_by_key.insert(key, id);
                }
            }
        }

        let count = to_delete.len() as u64;

        if count > 0 {
            // The reference-remap sweep MUST run before deleting the
            // duplicates: `lopdf::Document::delete_object` itself
            // already walks every object and strips any dict key/array
            // element that still references the deleted id (its own
            // dangling-reference cleanup) -- if a duplicate is deleted
            // first, that cleanup would silently *remove* every
            // `/Resources/XObject` entry pointing at it instead of
            // *redirecting* it to the canonical image, which is not
            // what a dedup pass should do to a page's own resources.
            let all_ids: Vec<ObjectId> = self.doc.objects.keys().copied().collect();
            for id in all_ids {
                let Some(dict) = self.doc.get_object_mut(id).ok().and_then(object_dict_mut) else { continue };

                if let Ok(&Object::Reference(smask_ref)) = dict.get(b"SMask") {
                    if let Some(&canonical) = ref_map.get(&smask_ref) {
                        dict.set(b"SMask".to_vec(), Object::Reference(canonical));
                    }
                }

                if let Ok(xobjects) = dict.get_mut(b"Resources").and_then(Object::as_dict_mut).and_then(|res| res.get_mut(b"XObject")).and_then(Object::as_dict_mut) {
                    let updates: Vec<(Vec<u8>, ObjectId)> = xobjects
                        .iter()
                        .filter_map(|(name, value)| value.as_reference().ok().and_then(|r| ref_map.get(&r).map(|&canonical| (name.clone(), canonical))))
                        .collect();
                    for (name, canonical) in updates {
                        xobjects.set(name, Object::Reference(canonical));
                    }
                }
            }
        }

        for id in &to_delete {
            self.doc.delete_object(*id);
        }

        count
    }

    /// Port of `dedup_images()`. See the module doc for why this runs
    /// exactly 2 passes rather than looping to convergence.
    pub fn dedup_images(&mut self) -> Result<u64> {
        let mut total = 0u64;
        for _ in 0..2 {
            total += self.run_one_dedup_pass();
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Dictionary, Document, Stream};

    fn image_stream(dict: Dictionary, width: i64, height: i64, content: &[u8]) -> Stream {
        let mut dict = dict;
        dict.set("Type", "XObject");
        dict.set("Subtype", "Image");
        dict.set("Width", width);
        dict.set("Height", height);
        Stream::new(dict, content.to_vec())
    }

    fn round_trip(doc: Document) -> PdfDoc {
        let mut doc = doc;
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        PdfDoc::load(&buf).unwrap()
    }

    #[test]
    fn dedup_images_merges_byte_identical_images_and_fixes_up_resources() {
        let mut doc = Document::with_version("1.4");
        let img_a = doc.add_object(image_stream(dictionary! {}, 10, 10, b"pixels"));
        let img_b = doc.add_object(image_stream(dictionary! {}, 10, 10, b"pixels"));
        let img_c = doc.add_object(image_stream(dictionary! {}, 10, 10, b"different"));

        let page1 = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => Object::Reference(img_a) } },
        });
        let page2 = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => Object::Reference(img_b), "Im1" => Object::Reference(img_c) } },
        });
        let pages_id = doc.add_object(dictionary! {
            "Type" => "Pages", "Count" => 2, "Kids" => Object::Array(vec![Object::Reference(page1), Object::Reference(page2)]),
        });
        doc.get_dictionary_mut(page1).unwrap().set("Parent", Object::Reference(pages_id));
        doc.get_dictionary_mut(page2).unwrap().set("Parent", Object::Reference(pages_id));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = round_trip(doc);
        let removed = pdf.dedup_images().unwrap();
        assert_eq!(removed, 1, "exactly one of the two byte-identical images should be removed");
        assert!(pdf.doc.get_object(img_b).is_err(), "the non-canonical duplicate should be gone");
        assert!(pdf.doc.get_object(img_a).is_ok());
        assert!(pdf.doc.get_object(img_c).is_ok(), "a genuinely different image must survive");

        // page2's Resources/XObject/Im0 must now point at the canonical img_a.
        let page2_dict = pdf.doc.get_dictionary(page2).unwrap();
        let xobjects = page2_dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap();
        assert_eq!(xobjects.get(b"Im0").unwrap().as_reference().unwrap(), img_a);
        assert_eq!(xobjects.get(b"Im1").unwrap().as_reference().unwrap(), img_c);
    }

    #[test]
    fn distinct_dimensions_are_never_merged_even_with_identical_bytes() {
        let mut doc = Document::with_version("1.4");
        let img_a = doc.add_object(image_stream(dictionary! {}, 10, 10, b"pixels"));
        let img_b = doc.add_object(image_stream(dictionary! {}, 20, 20, b"pixels"));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        let mut pdf = round_trip(doc);
        assert_eq!(pdf.dedup_images().unwrap(), 0);
        assert!(pdf.doc.get_object(img_a).is_ok());
        assert!(pdf.doc.get_object(img_b).is_ok());
    }

    #[test]
    fn second_pass_merges_images_whose_smasks_only_became_identical_after_pass_one() {
        let mut doc = Document::with_version("1.4");
        // Two byte-identical SMasks -- pass 1 merges these first.
        let smask_a = doc.add_object(image_stream(dictionary! {}, 5, 5, b"mask"));
        let smask_b = doc.add_object(image_stream(dictionary! {}, 5, 5, b"mask"));
        // Two byte-identical color images, but each currently points at
        // a DIFFERENT (still-distinct-at-this-point) SMask -- pass 1
        // alone must NOT merge these (different SMask reference).
        let mut dict_a = dictionary! {};
        dict_a.set("SMask", Object::Reference(smask_a));
        let img_a = doc.add_object(image_stream(dict_a, 10, 10, b"color"));
        let mut dict_b = dictionary! {};
        dict_b.set("SMask", Object::Reference(smask_b));
        let img_b = doc.add_object(image_stream(dict_b, 10, 10, b"color"));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = round_trip(doc);
        let removed = pdf.dedup_images().unwrap();
        // 1 SMask removed (pass 1) + 1 color image removed (pass 2, only
        // possible after the SMask refs converged) = 2 total.
        assert_eq!(removed, 2);
        assert!(pdf.doc.get_object(img_b).is_err());
        let surviving = pdf.doc.get_object(img_a).unwrap().as_stream().unwrap();
        let surviving_smask = surviving.dict.get(b"SMask").unwrap().as_reference().unwrap();
        assert_eq!(surviving_smask, smask_a);
    }
}
