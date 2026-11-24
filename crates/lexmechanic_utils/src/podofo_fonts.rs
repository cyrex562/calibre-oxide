//! Port of `calibre.utils.podofo`'s font management (`fonts.cpp`, issue
//! #577), built against [`crate::podofo`]'s `lopdf`-backed [`PdfDoc`]
//! (issue #74's doc-core).
//!
//! # Real content-stream tokenizing is simpler here than upstream's own
//!
//! `remove_unused_fonts`'s real "is this font used" detector scans every
//! page's (and every top-level Form XObject's) content stream for `Tf`
//! operators inside `BT`/`ET` text blocks. Real upstream hand-rolls a
//! generic PostScript tokenizer and manually maintains its own operand
//! stack (`Tf`'s two operands -- font name, then size -- are pushed
//! individually as bare tokens, so the C++ pops the stack once to
//! discard the size, then peeks the font name). `lopdf::Content`
//! already groups each operator with its own operand list
//! ([`lopdf::content::Operation`]), so a `Tf` operation's font-name
//! operand is simply `operands[0]` -- no manual stack simulation is
//! needed. This is a genuine simplification `lopdf`'s own higher-level
//! API enables, not a narrowing: the observable "which fonts are used"
//! result is identical.
//!
//! # Disclosed scope-narrowing (real upstream's own, not widened)
//!
//! `remove_unused_fonts` only ever considers Type0 and Type3 fonts as
//! removal candidates -- simple fonts (Type1, TrueType, ...) are never
//! removed even if genuinely unused. This is upstream's own real
//! behavior (its candidate loop only inserts Type0/Type3 refs into
//! `all_fonts`), preserved as-is rather than "fixed" to also prune
//! unused simple fonts.
//!
//! # Disclosed simplification
//!
//! `list_fonts`' real upstream distinguishes a `W`/`W2` array entry's
//! exact Python type (`PyFloat` for a real number, `PyLong` for an
//! integer) purely for output fidelity -- this port's [`WArrayValue`]
//! collapses both into a single `Number(f64)` variant, since nothing
//! downstream needs that int/float type distinction preserved, only
//! the numeric value.

use std::collections::{HashMap, HashSet};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Object, ObjectId};

use crate::podofo::{is_name, PdfDoc, PodofoError, Result};

/// One entry of a font's `/W`/`/W2` glyph-width array (port of
/// `convert_w_array`'s recursive number-or-nested-array shape).
#[derive(Debug, Clone, PartialEq)]
pub enum WArrayValue {
    Number(f64),
    Array(Vec<WArrayValue>),
}

/// Port of `list_fonts()`'s per-font dict shape.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontInfo {
    pub base_font: String,
    pub subtype: String,
    pub reference: Option<ObjectId>,
    /// The embedded font program's raw (decompressed) bytes, present
    /// only when `list_fonts(get_font_data: true)` was asked for one.
    pub data: Option<Vec<u8>>,
    /// For a Type0 composite font: its one `/DescendantFonts` entry.
    pub descendant_font: Option<ObjectId>,
    /// The embedded font-file stream's own reference, if any.
    pub stream_ref: Option<ObjectId>,
    pub encoding: Option<String>,
    /// A Type0 font's `/ToUnicode` CMap stream bytes, present only when
    /// `get_font_data` was asked for one.
    pub to_unicode: Option<Vec<u8>>,
    pub w: Option<Vec<WArrayValue>>,
    pub w2: Option<Vec<WArrayValue>>,
    /// A non-`/Identity` `/CIDToGIDMap` stream's raw bytes.
    pub cid_to_gid_map: Option<Vec<u8>>,
}

impl PdfDoc {
    /// Port of `list_fonts(get_font_data=False)`.
    pub fn list_fonts(&self, get_font_data: bool) -> Vec<FontInfo> {
        let mut out = Vec::new();
        for (&id, obj) in self.doc.objects.iter() {
            let Ok(dict) = obj.as_dict() else { continue };
            if !is_name(dict, b"Type", b"Font") {
                continue;
            }
            let Some(base_font) = dict.get(b"BaseFont").ok().and_then(|o| o.as_name().ok()) else {
                continue;
            };
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();

            let mut info = FontInfo {
                base_font: String::from_utf8_lossy(base_font).into_owned(),
                subtype,
                reference: Some(id),
                w: dict.get(b"W").ok().and_then(|o| o.as_array().ok()).map(|a| convert_w_array(a)),
                w2: dict.get(b"W2").ok().and_then(|o| o.as_array().ok()).map(|a| convert_w_array(a)),
                encoding: dict
                    .get(b"Encoding")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|b| String::from_utf8_lossy(b).into_owned()),
                ..Default::default()
            };

            info.cid_to_gid_map = match dict.get(b"CIDToGIDMap") {
                Ok(Object::Name(n)) if n == b"Identity" => None,
                Ok(obj) => self
                    .doc
                    .dereference(obj)
                    .ok()
                    .and_then(|(_, o)| o.as_stream().ok())
                    .and_then(|s| s.decompressed_content().ok()),
                Err(_) => None,
            };

            if let Some(desc_id) = dict.get(b"FontDescriptor").ok().and_then(|o| o.as_reference().ok()) {
                if let Ok(desc_dict) = self.doc.get_dictionary(desc_id) {
                    if let Some((_, ff_id)) = get_font_file_key_and_id(desc_dict) {
                        info.stream_ref = Some(ff_id);
                        if get_font_data {
                            info.data = self
                                .doc
                                .get_object(ff_id)
                                .ok()
                                .and_then(|o| o.as_stream().ok())
                                .and_then(|s| s.decompressed_content().ok());
                        }
                    }
                }
            } else if let Ok(Object::Array(dfs)) = dict.get(b"DescendantFonts") {
                if let Some(first) = dfs.first().and_then(|o| o.as_reference().ok()) {
                    info.descendant_font = Some(first);
                    if get_font_data {
                        if let Ok(tu_ref) = dict.get(b"ToUnicode").and_then(Object::as_reference) {
                            info.to_unicode = self
                                .doc
                                .get_object(tu_ref)
                                .ok()
                                .and_then(|o| o.as_stream().ok())
                                .and_then(|s| s.decompressed_content().ok());
                        }
                    }
                }
            }

            out.push(info);
        }
        out
    }

    /// Port of `remove_unused_fonts()`: removes every Type0/Type3 font
    /// not referenced by a `Tf` operator in any page's or Form
    /// XObject's content stream, plus any Type3 `CharProcs` glyph
    /// stream left with zero remaining users. Returns the number of
    /// font objects removed (matching upstream's own `count`, which
    /// counts fonts, not char-proc streams).
    pub fn remove_unused_fonts(&mut self) -> Result<u64> {
        let mut used_fonts: HashSet<ObjectId> = HashSet::new();

        for page_id in self.doc.page_iter().collect::<Vec<_>>() {
            let font_map = page_font_name_to_id(&self.doc, page_id);
            if let Ok(content) = self.doc.get_and_decode_page_content(page_id) {
                collect_used_fonts(&content, &font_map, &mut used_fonts);
            }
        }

        let form_xobject_ids: Vec<ObjectId> = self
            .doc
            .objects
            .iter()
            .filter_map(|(&id, obj)| {
                obj.as_stream()
                    .ok()
                    .filter(|s| is_name(&s.dict, b"Type", b"XObject") && is_name(&s.dict, b"Subtype", b"Form"))
                    .map(|_| id)
            })
            .collect();
        for xobj_id in form_xobject_ids {
            let Ok(obj) = self.doc.get_object(xobj_id) else { continue };
            let Ok(stream) = obj.as_stream() else { continue };
            let font_map = font_name_to_id_map(&self.doc, &stream.dict);
            let Ok(bytes) = stream.decompressed_content() else { continue };
            if let Ok(content) = Content::decode(&bytes) {
                collect_used_fonts(&content, &font_map, &mut used_fonts);
            }
        }

        let mut all_fonts: HashSet<ObjectId> = HashSet::new();
        let mut type3_fonts: HashSet<ObjectId> = HashSet::new();
        let mut charprocs_usage: HashMap<ObjectId, i64> = HashMap::new();
        let font_ids: Vec<ObjectId> = self
            .doc
            .objects
            .iter()
            .filter_map(|(&id, obj)| obj.as_dict().ok().filter(|d| is_name(d, b"Type", b"Font")).map(|_| id))
            .collect();
        for &id in &font_ids {
            let Ok(dict) = self.doc.get_dictionary(id) else { continue };
            match dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) {
                Some(b"Type0") => {
                    all_fonts.insert(id);
                }
                Some(b"Type3") => {
                    all_fonts.insert(id);
                    type3_fonts.insert(id);
                    for r in char_proc_refs(dict) {
                        *charprocs_usage.entry(r).or_insert(0) += 1;
                    }
                }
                _ => {}
            }
        }

        let mut count = 0u64;
        for &font_id in &all_fonts {
            if used_fonts.contains(&font_id) {
                continue;
            }
            count += 1;
            if type3_fonts.contains(&font_id) {
                let refs = self
                    .doc
                    .get_dictionary(font_id)
                    .ok()
                    .map(char_proc_refs)
                    .unwrap_or_default();
                for r in refs {
                    if let Some(c) = charprocs_usage.get_mut(&r) {
                        *c -= 1;
                    }
                }
            } else {
                let descendant_refs: Vec<ObjectId> = self
                    .doc
                    .get_dictionary(font_id)
                    .ok()
                    .and_then(|d| d.get(b"DescendantFonts").ok().and_then(|o| o.as_array().ok()))
                    .map(|arr| arr.iter().filter_map(|o| o.as_reference().ok()).collect())
                    .unwrap_or_default();
                for dfont_id in descendant_refs {
                    self.remove_font_object(dfont_id);
                }
            }
            self.remove_font_object(font_id);
        }

        let zero_usage: Vec<ObjectId> = charprocs_usage.iter().filter(|&(_, &n)| n == 0).map(|(&r, _)| r).collect();
        for r in zero_usage {
            self.doc.delete_object(r);
        }

        Ok(count)
    }

    fn remove_font_object(&mut self, font_id: ObjectId) {
        if let Ok(dict) = self.doc.get_dictionary(font_id) {
            if let Some(desc_id) = dict.get(b"FontDescriptor").ok().and_then(|o| o.as_reference().ok()) {
                if let Some((_, ff_id)) = self.doc.get_dictionary(desc_id).ok().and_then(get_font_file_key_and_id) {
                    self.doc.delete_object(ff_id);
                }
                self.doc.delete_object(desc_id);
            }
        }
        self.doc.delete_object(font_id);
    }

    /// Port of `replace_font_data(data, reference)`: overwrites the
    /// embedded font program bytes for the font at `font_ref`. Like
    /// [`PdfDoc::set_xmp_metadata`], the stream's `/Filter` is dropped
    /// since `data` is stored as-is, not re-compressed.
    pub fn replace_font_data(&mut self, font_ref: ObjectId, data: &[u8]) -> Result<()> {
        let (_, ff_id) = self.font_file_ref(font_ref)?;
        let stream = self.doc.get_object_mut(ff_id)?.as_stream_mut()?;
        stream.set_plain_content(data.to_vec());
        stream.dict.remove(b"Filter");
        Ok(())
    }

    /// Port of `merge_fonts(data, references)`: replaces the first
    /// font's embedded font-file data with `data`, then repoints every
    /// other listed font's descriptor at that same font-file object,
    /// deleting their own now-unused ones.
    pub fn merge_fonts(&mut self, data: &[u8], references: &[ObjectId]) -> Result<()> {
        let mut canonical_ff: Option<ObjectId> = None;
        for (i, &font_ref) in references.iter().enumerate() {
            let (key, ff_id) = self.font_file_ref(font_ref)?;
            if i == 0 {
                canonical_ff = Some(ff_id);
                let stream = self.doc.get_object_mut(ff_id)?.as_stream_mut()?;
                stream.set_plain_content(data.to_vec());
                stream.dict.remove(b"Filter");
            } else {
                let canonical = canonical_ff.expect("set on the first iteration");
                self.doc.delete_object(ff_id);
                let desc_id = self
                    .doc
                    .get_dictionary(font_ref)?
                    .get(b"FontDescriptor")
                    .and_then(Object::as_reference)?;
                self.doc.get_dictionary_mut(desc_id)?.set(key.to_vec(), Object::Reference(canonical));
            }
        }
        Ok(())
    }

    fn font_file_ref(&self, font_ref: ObjectId) -> Result<(&'static [u8], ObjectId)> {
        let desc_id = self
            .doc
            .get_dictionary(font_ref)?
            .get(b"FontDescriptor")
            .and_then(Object::as_reference)
            .map_err(|_| PodofoError::FontHasNoDescriptor)?;
        let desc_dict = self.doc.get_dictionary(desc_id)?;
        get_font_file_key_and_id(desc_dict).ok_or(PodofoError::FontHasNoDescriptor)
    }

    /// Port of `dedup_type3_fonts()`: finds byte-identical `CharProcs`
    /// glyph streams shared across (or within) Type3 fonts, keeps the
    /// first-seen copy of each as canonical, deletes the rest, and
    /// repoints every font's `CharProcs` entries at the canonical copy.
    /// Returns the number of duplicate streams removed.
    pub fn dedup_type3_fonts(&mut self) -> Result<u64> {
        let type3_font_ids: Vec<ObjectId> = self
            .doc
            .objects
            .iter()
            .filter_map(|(&id, obj)| {
                obj.as_dict()
                    .ok()
                    .filter(|d| is_name(d, b"Type", b"Font") && is_name(d, b"Subtype", b"Type3"))
                    .map(|_| id)
            })
            .collect();

        let mut by_content: HashMap<Vec<u8>, ObjectId> = HashMap::new();
        let mut ref_map: HashMap<ObjectId, ObjectId> = HashMap::new();
        let mut to_delete: Vec<ObjectId> = Vec::new();

        for &font_id in &type3_font_ids {
            let Ok(dict) = self.doc.get_dictionary(font_id) else { continue };
            for cp_ref in char_proc_refs(dict) {
                if ref_map.contains_key(&cp_ref) {
                    continue;
                }
                let Ok(obj) = self.doc.get_object(cp_ref) else { continue };
                let Ok(stream) = obj.as_stream() else { continue };
                match by_content.get(&stream.content) {
                    Some(&canonical) if canonical != cp_ref => {
                        ref_map.insert(cp_ref, canonical);
                        to_delete.push(cp_ref);
                    }
                    Some(_) => {}
                    None => {
                        by_content.insert(stream.content.clone(), cp_ref);
                    }
                }
            }
        }

        let count = to_delete.len() as u64;
        for id in to_delete {
            self.doc.delete_object(id);
        }

        if count > 0 {
            for &font_id in &type3_font_ids {
                let entries: Vec<(Vec<u8>, ObjectId)> = match self.doc.get_dictionary(font_id) {
                    Ok(dict) => match dict.get(b"CharProcs").ok().and_then(|o| o.as_dict().ok()) {
                        Some(cp) => cp.iter().filter_map(|(k, v)| v.as_reference().ok().map(|r| (k.clone(), r))).collect(),
                        None => continue,
                    },
                    Err(_) => continue,
                };
                let remapped: Vec<(Vec<u8>, ObjectId)> = entries
                    .into_iter()
                    .filter_map(|(name, r)| ref_map.get(&r).map(|&canonical| (name, canonical)))
                    .collect();
                if remapped.is_empty() {
                    continue;
                }
                if let Ok(font_dict) = self.doc.get_dictionary_mut(font_id) {
                    if let Ok(cp_dict) = font_dict.get_mut(b"CharProcs").and_then(Object::as_dict_mut) {
                        for (name, canonical) in remapped {
                            cp_dict.set(name, Object::Reference(canonical));
                        }
                    }
                }
            }
        }

        Ok(count)
    }
}

fn get_font_file_key_and_id(desc_dict: &Dictionary) -> Option<(&'static [u8], ObjectId)> {
    for key in [&b"FontFile"[..], &b"FontFile2"[..], &b"FontFile3"[..]] {
        if let Ok(id) = desc_dict.get(key).and_then(Object::as_reference) {
            return Some((key, id));
        }
    }
    None
}

fn char_proc_refs(font_dict: &Dictionary) -> Vec<ObjectId> {
    font_dict
        .get(b"CharProcs")
        .ok()
        .and_then(|o| o.as_dict().ok())
        .map(|cp| cp.iter().filter_map(|(_, v)| v.as_reference().ok()).collect())
        .unwrap_or_default()
}

fn convert_w_array(arr: &[Object]) -> Vec<WArrayValue> {
    arr.iter()
        .filter_map(|o| match o {
            Object::Array(inner) => Some(WArrayValue::Array(convert_w_array(inner))),
            Object::Integer(n) => Some(WArrayValue::Number(*n as f64)),
            Object::Real(f) => Some(WArrayValue::Number(*f as f64)),
            _ => None,
        })
        .collect()
}

fn font_name_to_id_map(doc: &lopdf::Document, resources: &Dictionary) -> HashMap<Vec<u8>, ObjectId> {
    let mut out = HashMap::new();
    let Ok(font_obj) = resources.get(b"Font") else { return out };
    let Ok((_, resolved)) = doc.dereference(font_obj) else { return out };
    let Ok(font_dict) = resolved.as_dict() else { return out };
    for (name, value) in font_dict.iter() {
        if let Ok(id) = value.as_reference() {
            out.insert(name.clone(), id);
        }
    }
    out
}

/// Resolves a page's effective `/Resources/Font` name-to-reference map,
/// honoring page-tree inheritance the same way [`lopdf::Document::get_page_resources`]
/// does: the page's own inline `Resources` dict if present, else the
/// first non-empty `Font` dict found walking up `/Parent`.
fn page_font_name_to_id(doc: &lopdf::Document, page_id: ObjectId) -> HashMap<Vec<u8>, ObjectId> {
    let Ok((primary, extra_ids)) = doc.get_page_resources(page_id) else {
        return HashMap::new();
    };
    if let Some(dict) = primary {
        let m = font_name_to_id_map(doc, dict);
        if !m.is_empty() {
            return m;
        }
    }
    for id in extra_ids {
        if let Ok(dict) = doc.get_dictionary(id) {
            let m = font_name_to_id_map(doc, dict);
            if !m.is_empty() {
                return m;
            }
        }
    }
    HashMap::new()
}

fn collect_used_fonts(content: &Content<Vec<Operation>>, font_map: &HashMap<Vec<u8>, ObjectId>, used: &mut HashSet<ObjectId>) {
    let mut in_text_block = false;
    for op in &content.operations {
        match op.operator.as_str() {
            "BT" => in_text_block = true,
            "ET" => in_text_block = false,
            "Tf" if in_text_block => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    if let Some(&id) = font_map.get(name) {
                        used.insert(id);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Stream};

    fn font_dict(base_font: &str, subtype: &str) -> Dictionary {
        dictionary! {
            "Type" => "Font",
            "Subtype" => subtype,
            "BaseFont" => base_font,
        }
    }

    fn round_trip(doc: Document) -> PdfDoc {
        let mut doc = doc;
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        PdfDoc::load(&buf).unwrap()
    }

    #[test]
    fn list_fonts_reads_simple_font_fields() {
        let mut doc = Document::with_version("1.4");
        let ff_id = doc.add_object(Stream::new(dictionary! {}, b"glyf-data".to_vec()));
        let desc_id = doc.add_object(dictionary! {
            "Type" => "FontDescriptor",
            "FontFile2" => Object::Reference(ff_id),
        });
        let mut f = font_dict("Arial", "TrueType");
        f.set("FontDescriptor", Object::Reference(desc_id));
        f.set("Encoding", "WinAnsiEncoding");
        f.set("W", Object::Array(vec![Object::Integer(1), Object::Real(2.5)]));
        doc.add_object(f);
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let pdf = round_trip(doc);
        let fonts = pdf.list_fonts(true);
        assert_eq!(fonts.len(), 1);
        let f = &fonts[0];
        assert_eq!(f.base_font, "Arial");
        assert_eq!(f.subtype, "TrueType");
        assert_eq!(f.encoding.as_deref(), Some("WinAnsiEncoding"));
        assert_eq!(f.data.as_deref(), Some(b"glyf-data".as_slice()));
        assert_eq!(f.w, Some(vec![WArrayValue::Number(1.0), WArrayValue::Number(2.5)]));
    }

    #[test]
    fn list_fonts_reads_type0_descendant_and_to_unicode() {
        let mut doc = Document::with_version("1.4");
        let cid_font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType2" });
        let tu_id = doc.add_object(Stream::new(dictionary! {}, b"cmap-data".to_vec()));
        let mut f = font_dict("NotoSans", "Type0");
        f.set("DescendantFonts", Object::Array(vec![Object::Reference(cid_font_id)]));
        f.set("ToUnicode", Object::Reference(tu_id));
        doc.add_object(f);
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let pdf = round_trip(doc);
        let fonts: Vec<_> = pdf.list_fonts(true).into_iter().filter(|f| f.subtype == "Type0").collect();
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].descendant_font, Some(cid_font_id));
        assert_eq!(fonts[0].to_unicode.as_deref(), Some(b"cmap-data".as_slice()));
    }

    #[test]
    fn remove_unused_fonts_keeps_used_drops_unused_type0() {
        let mut doc = Document::with_version("1.4");
        let used_ff = doc.add_object(Stream::new(dictionary! {}, b"used".to_vec()));
        let used_desc = doc.add_object(dictionary! { "FontFile2" => Object::Reference(used_ff) });
        let used_font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "FontDescriptor" => Object::Reference(used_desc),
        });
        let unused_ff = doc.add_object(Stream::new(dictionary! {}, b"unused".to_vec()));
        let unused_desc = doc.add_object(dictionary! { "FontFile2" => Object::Reference(unused_ff) });
        let unused_font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "FontDescriptor" => Object::Reference(unused_desc),
        });

        let (mut doc, _page) = doc_with_page_using_fonts_into(doc, &[("F1", used_font)], &[("F1", used_font), ("F2", unused_font)]);
        let mut pdf = PdfDoc { doc: std::mem::take(&mut doc) };
        let removed = pdf.remove_unused_fonts().unwrap();
        assert_eq!(removed, 1);
        assert!(pdf.doc.get_object(used_font).is_ok());
        assert!(pdf.doc.get_object(unused_font).is_err());
        assert!(pdf.doc.get_object(unused_desc).is_err());
        assert!(pdf.doc.get_object(unused_ff).is_err());
    }

    /// Like `doc_with_page_using_fonts` but takes an already-populated
    /// `Document` (fonts already added as objects) instead of building
    /// a fresh one, since the two-step "create fonts, then wire a page
    /// that references them" tests need the objects created first.
    fn doc_with_page_using_fonts_into(
        mut doc: Document,
        font_names: &[(&str, ObjectId)],
        all_fonts: &[(&str, ObjectId)],
    ) -> (Document, ObjectId) {
        let mut font_res = Dictionary::new();
        for (name, id) in all_fonts {
            font_res.set(*name, Object::Reference(*id));
        }
        let resources = dictionary! { "Font" => Object::Dictionary(font_res) };

        let mut content = String::new();
        content.push_str("BT\n");
        for (name, _) in font_names {
            content.push_str(&format!("/{name} 12 Tf\n"));
        }
        content.push_str("ET\n");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "Resources" => resources,
            "Contents" => Object::Reference(content_id),
        });
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Count" => 1,
                "Kids" => Object::Array(vec![Object::Reference(page_id)]),
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        (doc, page_id)
    }

    #[test]
    fn remove_unused_fonts_never_removes_used_simple_font_but_also_never_removes_unused_simple_font() {
        let mut doc = Document::with_version("1.4");
        let unused_simple = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1" });
        let (doc, _page) = doc_with_page_using_fonts_into(doc, &[], &[("F1", unused_simple)]);
        let mut pdf = PdfDoc { doc };
        let removed = pdf.remove_unused_fonts().unwrap();
        assert_eq!(removed, 0);
        assert!(pdf.doc.get_object(unused_simple).is_ok());
    }

    #[test]
    fn remove_unused_fonts_shared_char_proc_survives_if_still_used() {
        let mut doc = Document::with_version("1.4");
        let cp_id = doc.add_object(Stream::new(dictionary! {}, b"glyph".to_vec()));
        let used_font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_id) },
        });
        let unused_font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_id) },
        });

        let (doc, _page) = doc_with_page_using_fonts_into(doc, &[("F1", used_font)], &[("F1", used_font), ("F2", unused_font)]);
        let mut pdf = PdfDoc { doc };
        let removed = pdf.remove_unused_fonts().unwrap();
        assert_eq!(removed, 1);
        assert!(pdf.doc.get_object(unused_font).is_err());
        // The shared glyph stream is still used by `used_font` -> must survive.
        assert!(pdf.doc.get_object(cp_id).is_ok());
    }

    #[test]
    fn remove_unused_fonts_charproc_only_used_by_removed_font_is_deleted() {
        let mut doc = Document::with_version("1.4");
        let cp_id = doc.add_object(Stream::new(dictionary! {}, b"glyph".to_vec()));
        let unused_font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_id) },
        });
        let (doc, _page) = doc_with_page_using_fonts_into(doc, &[], &[("F1", unused_font)]);
        let mut pdf = PdfDoc { doc };
        let removed = pdf.remove_unused_fonts().unwrap();
        assert_eq!(removed, 1);
        assert!(pdf.doc.get_object(cp_id).is_err());
    }

    #[test]
    fn replace_font_data_overwrites_font_file_stream() {
        let mut doc = Document::with_version("1.4");
        let ff_id = doc.add_object(Stream::new(dictionary! { "Filter" => "FlateDecode" }, b"old".to_vec()));
        let desc_id = doc.add_object(dictionary! { "FontFile2" => Object::Reference(ff_id) });
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "TrueType", "FontDescriptor" => Object::Reference(desc_id),
        });
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = PdfDoc { doc };
        pdf.replace_font_data(font_id, b"new-font-bytes").unwrap();
        let stream = pdf.doc.get_object(ff_id).unwrap().as_stream().unwrap();
        assert_eq!(stream.content, b"new-font-bytes");
        assert!(!stream.dict.has(b"Filter"));
    }

    #[test]
    fn merge_fonts_consolidates_into_one_shared_font_file() {
        let mut doc = Document::with_version("1.4");
        let ff1 = doc.add_object(Stream::new(dictionary! {}, b"a".to_vec()));
        let desc1 = doc.add_object(dictionary! { "FontFile2" => Object::Reference(ff1) });
        let font1 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "TrueType", "FontDescriptor" => Object::Reference(desc1),
        });
        let ff2 = doc.add_object(Stream::new(dictionary! {}, b"b".to_vec()));
        let desc2 = doc.add_object(dictionary! { "FontFile2" => Object::Reference(ff2) });
        let font2 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "TrueType", "FontDescriptor" => Object::Reference(desc2),
        });
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = PdfDoc { doc };
        pdf.merge_fonts(b"shared", &[font1, font2]).unwrap();

        assert!(pdf.doc.get_object(ff2).is_err());
        let desc2_dict = pdf.doc.get_dictionary(desc2).unwrap();
        let ff2_ref = desc2_dict.get(b"FontFile2").unwrap().as_reference().unwrap();
        assert_eq!(ff2_ref, ff1);
        let stream = pdf.doc.get_object(ff1).unwrap().as_stream().unwrap();
        assert_eq!(stream.content, b"shared");
    }

    #[test]
    fn dedup_type3_fonts_merges_identical_glyph_streams() {
        let mut doc = Document::with_version("1.4");
        let cp_a = doc.add_object(Stream::new(dictionary! {}, b"same-glyph".to_vec()));
        let cp_b = doc.add_object(Stream::new(dictionary! {}, b"same-glyph".to_vec()));
        let cp_c = doc.add_object(Stream::new(dictionary! {}, b"different".to_vec()));
        let font1 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_a), "g2" => Object::Reference(cp_c) },
        });
        let font2 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_b) },
        });
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = PdfDoc { doc };
        let removed = pdf.dedup_type3_fonts().unwrap();
        assert_eq!(removed, 1);
        assert!(pdf.doc.get_object(cp_b).is_err());
        assert!(pdf.doc.get_object(cp_a).is_ok());
        assert!(pdf.doc.get_object(cp_c).is_ok());

        let font2_dict = pdf.doc.get_dictionary(font2).unwrap();
        let cp_dict = font2_dict.get(b"CharProcs").unwrap().as_dict().unwrap();
        let g1_ref = cp_dict.get(b"g1").unwrap().as_reference().unwrap();
        assert_eq!(g1_ref, cp_a);

        let font1_dict = pdf.doc.get_dictionary(font1).unwrap();
        let cp_dict1 = font1_dict.get(b"CharProcs").unwrap().as_dict().unwrap();
        assert_eq!(cp_dict1.get(b"g2").unwrap().as_reference().unwrap(), cp_c);
    }

    #[test]
    fn dedup_type3_fonts_is_a_no_op_with_no_duplicates() {
        let mut doc = Document::with_version("1.4");
        let cp_a = doc.add_object(Stream::new(dictionary! {}, b"one".to_vec()));
        let cp_b = doc.add_object(Stream::new(dictionary! {}, b"two".to_vec()));
        doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3",
            "CharProcs" => dictionary! { "g1" => Object::Reference(cp_a), "g2" => Object::Reference(cp_b) },
        });
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog" });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut pdf = PdfDoc { doc };
        assert_eq!(pdf.dedup_type3_fonts().unwrap(), 0);
    }
}
