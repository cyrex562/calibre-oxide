//! Port of `calibre.utils.fonts.sfnt.cff.writer` (issue #565, split
//! from #554/#65): the mechanical, self-contained CFF write-side
//! primitives -- the variable-offset-width INDEX writer
//! ([`IndexBuilder`], the reverse of #564's `table::parse_index`), the
//! SID-interning string table ([`StringsWriter`], the reverse of
//! #564's `Strings`), the glyph-name-array charset writer
//! ([`CharsetsWriter`], the reverse of #564's format-0 `Charset`), and
//! [`Subset`] -- the real subsetting orchestrator that ties all of the
//! above together into a new, smaller CFF table.
//!
//! # The interior-mutability decision: `RefCell<StringsWriter>`
//!
//! Real Python's `strings` parameter to `Dict.compile` is a *stateful*
//! closure over a mutable `Strings` object (interning as it goes), but
//! [`crate::fonts::sfnt::cff::dict_data::Dict::compile`]'s
//! `resolve_sid` parameter is `&dyn Fn(&str) -> u16` (no interior
//! mutation), and [`StringsWriter::intern`] needs `&mut self`.
//! [`Subset::new`] resolves this the standard way: a `RefCell<StringsWriter>`
//! captured by a small closure (`|s| strings.borrow_mut().intern(s)`),
//! which is a real `Fn` since the mutation happens through a shared
//! reference. No caller outside this function ever sees the `RefCell`.
//!
//! # `Subset::new`'s compile order matches real upstream's exactly
//!
//! Real `Subset.__init__` compiles the Top DICT once *before* the
//! private dict (interning any `Sid`-typed Top DICT fields first), even
//! though that first compile's output is discarded (only its side
//! effect -- interning -- matters, since the real offsets aren't known
//! yet). This port keeps that same call, unused return value and all,
//! rather than only calling `compile` where its result is later used --
//! diverging on ordering would intern new (non-standard) strings in a
//! different order and change every SID assigned from that point on,
//! producing a different (though still internally self-consistent)
//! byte layout than real upstream's for the same input font.
//!
//! # A faithfully-replicated real upstream quirk: `PrivateDict.compile`
//!
//! Real `PrivateDict.compile` computes the local `Subrs` INDEX's offset
//! as `len(raw)` from a compile pass that ran *before* the `Subrs`
//! field itself was added to the dict -- so the stored offset doesn't
//! account for the few extra bytes the `Subrs` field's own entry adds
//! to the second, final compile. This looks like a real upstream bug
//! (per this project's convention: replicate observed upstream bugs
//! bug-for-bug rather than silently "fixing" them) -- see
//! [`compile_private_dict`]. It only matters for fonts with local
//! (non-global) Subrs, which is uncommon; most embedded ebook fonts
//! that use CFF outlines don't define any.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::fonts::sfnt::cff::constants::CFF_STANDARD_STRINGS;
use crate::fonts::sfnt::cff::dict_data::{Dict, Operand};
use crate::fonts::sfnt::cff::table::{Cff, Charset};

/// Port of `writer.py`'s own `Index` class (an accumulate-then-compile
/// byte-string list) -- the write-side mirror of #564's
/// `table::parse_index`. Named `IndexBuilder` to avoid colliding with
/// that module's own (unnamed, function-based) reader.
#[derive(Debug, Clone, Default)]
pub struct IndexBuilder {
    items: Vec<Vec<u8>>,
}

impl IndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, item: Vec<u8>) {
        self.items.push(item);
    }

    pub fn extend(&mut self, items: impl IntoIterator<Item = Vec<u8>>) {
        self.items.extend(items);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Port of `Index.calcsize`.
    fn calcsize(largest_offset: u32) -> u8 {
        if largest_offset < 0x100 {
            1
        } else if largest_offset < 0x1_0000 {
            2
        } else if largest_offset < 0x100_0000 {
            3
        } else {
            4
        }
    }

    /// Port of `Index.compile`.
    pub fn compile(&self) -> Vec<u8> {
        if self.items.is_empty() {
            return 0u16.to_be_bytes().to_vec();
        }
        let mut offsets = vec![1u32];
        for item in &self.items {
            offsets.push(offsets.last().unwrap() + item.len() as u32);
        }
        let offsize = Self::calcsize(*offsets.last().unwrap());

        let mut out = Vec::new();
        out.extend((self.items.len() as u16).to_be_bytes());
        out.push(offsize);
        for &o in &offsets {
            match offsize {
                1 => out.push(o as u8),
                2 => out.extend((o as u16).to_be_bytes()),
                3 => out.extend(&o.to_be_bytes()[1..]),
                _ => out.extend(o.to_be_bytes()),
            }
        }
        for item in &self.items {
            out.extend_from_slice(item);
        }
        out
    }
}

/// Port of `writer.py`'s `Strings`: interns strings, reusing a
/// standard string's real SID when one matches, and assigning fresh
/// SIDs (`391 + n`) to new ones in first-seen order -- the reverse of
/// #564's `Strings` reader. Real upstream's `__call__` is this port's
/// [`Self::intern`].
#[derive(Debug, Clone)]
pub struct StringsWriter {
    added: HashMap<String, u16>,
    items: Vec<String>,
}

impl Default for StringsWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl StringsWriter {
    pub fn new() -> Self {
        let added = CFF_STANDARD_STRINGS.iter().enumerate().map(|(i, s)| (s.to_string(), i as u16)).collect();
        StringsWriter { added, items: Vec::new() }
    }

    /// Port of `Strings.__call__`.
    pub fn intern(&mut self, s: &str) -> u16 {
        if let Some(&sid) = self.added.get(s) {
            return sid;
        }
        let sid = (self.items.len() + CFF_STANDARD_STRINGS.len()) as u16;
        self.added.insert(s.to_string(), sid);
        self.items.push(s.to_string());
        sid
    }

    /// Port of `Strings.compile` (via the base `Index.compile`):
    /// serializes only the *newly interned* (non-standard) strings, in
    /// first-seen order.
    pub fn compile(&self) -> Vec<u8> {
        let mut idx = IndexBuilder::new();
        idx.extend(self.items.iter().map(|s| s.as_bytes().to_vec()));
        idx.compile()
    }
}

/// Port of `writer.py`'s `Charsets`: a format-0 charset table (a flat
/// SID array, one per non-`.notdef` glyph in glyph-id order) -- the
/// reverse of #564's format-0 `Charset::parse` branch.
#[derive(Debug, Clone)]
pub struct CharsetsWriter {
    /// Glyph names for glyph ids `1..`, matching real upstream's own
    /// `charsets.extend(cff.charset[1:])` (`.notdef` at glyph 0 is
    /// never included -- it's implicit).
    names: Vec<String>,
}

impl CharsetsWriter {
    pub fn new(names: Vec<String>) -> Self {
        CharsetsWriter { names }
    }

    /// Port of `Charsets.compile`.
    pub fn compile(&self, strings: &mut StringsWriter) -> Vec<u8> {
        let mut out = vec![0u8]; // format 0
        for name in &self.names {
            out.extend(strings.intern(name).to_be_bytes());
        }
        out
    }
}

/// The compiled form of a real font's Private DICT, plus its local
/// Subrs INDEX bytes if it has any -- the reverse of #564's own
/// `Cff::private_dict`/`Cff::private_subrs`. Port of `writer.py`'s
/// `PrivateDict` class.
struct CompiledPrivateDict {
    raw: Vec<u8>,
    subrs_raw: Option<Vec<u8>>,
}

/// Port of `PrivateDict.__init__` + `PrivateDict.compile`. See the
/// module doc for the faithfully-replicated real upstream `Subrs`
/// offset quirk this reproduces.
fn compile_private_dict(mut src: Dict, subrs: Option<&[Vec<u8>]>, resolve_sid: &dyn Fn(&str) -> u16) -> CompiledPrivateDict {
    let subrs_raw = subrs.map(|items| {
        let mut idx = IndexBuilder::new();
        idx.extend(items.iter().cloned());
        idx.compile()
    });

    let mut raw = src.compile(resolve_sid);
    if subrs_raw.is_some() {
        src.set("Subrs", vec![Operand::Int(raw.len() as i64)]);
        raw = src.compile(resolve_sid);
    }
    CompiledPrivateDict { raw, subrs_raw }
}

/// Port of `writer.py`'s `Subset` class: the real CFF subsetting
/// orchestrator. Rebuilds the Name/String/CharStrings/GlobalSubrs
/// INDEX structures and the charset table from scratch, keeping only
/// glyphs in `keep_charnames` -- dropped glyphs' charstrings become a
/// single real `endchar` operator byte (`14`) rather than being
/// removed, so every surviving glyph keeps its original glyph id.
pub struct Subset {
    /// The complete, real, re-parseable CFF table bytes.
    pub raw: Vec<u8>,
    /// Port of `Subset.charname_map`: glyph name -> (original/kept)
    /// glyph id, in first-seen order (an `IndexMap` port of real
    /// upstream's `OrderedDict`, though nothing here currently reads
    /// the ordering -- kept for parity with upstream's own type).
    pub charname_map: IndexMap<String, usize>,
}

impl Subset {
    /// Port of `Subset.__init__`.
    pub fn new(cff: &Cff, keep_charnames: HashSet<String>) -> Self {
        let mut keep_charnames = keep_charnames;
        keep_charnames.insert(".notdef".to_string());

        let header = vec![1u8, 0, 4, cff.offset_size];

        // Font names Index
        let mut font_names = IndexBuilder::new();
        font_names.extend(cff.font_names.iter().cloned());

        // Strings Index -- see the module doc for the RefCell decision.
        let strings = RefCell::new(StringsWriter::new());
        let resolve_sid = |s: &str| strings.borrow_mut().intern(s);

        // CharStrings Index and charsets
        let mut char_strings = IndexBuilder::new();
        let mut charname_map: IndexMap<String, usize> = IndexMap::new();
        // `cff.charset[1:]` -- `.notdef` is not included. A *standard*
        // charset's real upstream list is never populated at all (see
        // `Charset.__init__`'s own early return), so this is
        // faithfully empty in that case too.
        let names_after_notdef: Vec<String> = match &cff.charset {
            Charset::Standard(_) => Vec::new(),
            Charset::Custom(names) => names.iter().skip(1).cloned().collect(),
        };

        let endchar_operator = vec![14u8];
        for i in 0..cff.num_glyphs {
            let cname = cff.charset.safe_lookup(i);
            let ok = cname.as_deref().is_some_and(|n| keep_charnames.contains(n));
            let cs = if ok { cff.char_strings[i].clone() } else { endchar_operator.clone() };
            char_strings.push(cs);
            if ok {
                charname_map.insert(cname.unwrap(), i);
            }
        }

        // Add the strings
        let char_strings_raw = char_strings.compile();
        let charsets_raw = CharsetsWriter::new(names_after_notdef).compile(&mut strings.borrow_mut());

        // Global subroutines
        let mut global_subrs = IndexBuilder::new();
        global_subrs.extend(cff.global_subrs.iter().cloned());
        let global_subrs_raw = global_subrs.compile();

        // TOP DICT -- real upstream's own `Dict` (writer.py) subclasses
        // `Index`: the compiled DICT byte-code is wrapped as the sole
        // entry of a one-item INDEX (the real "Top DICT INDEX"), not
        // emitted bare. `#563`'s `dict_data::Dict::compile` only
        // produces the byte-code itself, so that wrapping happens here.
        fn compile_top_dict_index(dict: &Dict, resolve_sid: &dyn Fn(&str) -> u16) -> Vec<u8> {
            let mut idx = IndexBuilder::new();
            idx.push(dict.compile(resolve_sid));
            idx.compile()
        }

        let mut top_dict = cff.top_dict.clone();
        compile_top_dict_index(&top_dict, &resolve_sid); // Add strings (result unused -- see module doc)

        let private = cff.private_dict.as_ref().map(|pd| compile_private_dict(pd.clone(), cff.private_subrs.as_deref(), &resolve_sid));

        let fixed_prefix: Vec<u8> = header.into_iter().chain(font_names.compile()).collect();

        let t = &mut top_dict;
        // Put in dummy offsets
        t.set("charset", vec![Operand::Int(1)]);
        t.set("CharStrings", vec![Operand::Int(1)]);
        if let Some(p) = &private {
            t.set("Private", vec![Operand::Int(p.raw.len() as i64), Operand::Int(1)]);
        }
        let mut top_dict_raw = compile_top_dict_index(t, &resolve_sid);

        let strings_raw = strings.borrow().compile();

        // Calculate real offsets
        let mut pos = fixed_prefix.len();
        pos += top_dict_raw.len();
        pos += strings_raw.len();
        pos += global_subrs_raw.len();
        t.set("charset", vec![Operand::Int(pos as i64)]);
        pos += charsets_raw.len();
        t.set("CharStrings", vec![Operand::Int(pos as i64)]);
        pos += char_strings_raw.len();
        if let Some(p) = &private {
            t.set("Private", vec![Operand::Int(p.raw.len() as i64), Operand::Int(pos as i64)]);
        }
        top_dict_raw = compile_top_dict_index(t, &resolve_sid);

        let mut raw = fixed_prefix;
        raw.extend(top_dict_raw);
        raw.extend(strings_raw);
        raw.extend(global_subrs_raw);
        raw.extend(charsets_raw);
        raw.extend(char_strings_raw);
        if let Some(p) = private {
            raw.extend(p.raw);
            if let Some(subrs_raw) = p.subrs_raw {
                raw.extend(subrs_raw);
            }
        }

        Subset { raw, charname_map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::sfnt::cff::table::parse_index;

    /// A real, fixed-width-offset (see `table::tests::minimal_cff`'s own
    /// doc for why) 4-glyph CFF table: `.notdef` + 3 glyphs, a custom
    /// format-0 charset resolving to real CFF Standard Strings
    /// ("space"/"exclam"/"quotedbl", SIDs 1-3) so no font-embedded
    /// Strings INDEX entries are needed -- standing in for arbitrary
    /// glyph names since nothing here interprets real charstring
    /// semantics (each glyph's "charstring" is just a distinguishable
    /// 1-byte payload).
    fn four_glyph_cff() -> Vec<u8> {
        fn write_offset_op(v: u32) -> Vec<u8> {
            let mut out = vec![29u8];
            out.extend(v.to_be_bytes());
            out
        }
        fn index(entries: &[&[u8]]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend((entries.len() as u16).to_be_bytes());
            if entries.is_empty() {
                return out;
            }
            out.push(1u8); // offset_size = 1
            let mut off = 1u32;
            out.push(off as u8);
            for e in entries {
                off += e.len() as u32;
                out.push(off as u8);
            }
            for e in entries {
                out.extend_from_slice(e);
            }
            out
        }

        let header = vec![1u8, 0, 4, 4]; // major, minor, header_size=4, offset_size=4
        let names = index(&[b"Font"]);

        let build = |charstrings_off: u32, charset_off: u32| -> Vec<u8> {
            let mut dict = Vec::new();
            dict.extend(write_offset_op(charset_off));
            dict.push(15); // charset
            dict.extend(write_offset_op(charstrings_off));
            dict.push(17); // CharStrings
            index(&[&dict])
        };

        let strings = index(&[]);
        let global_subrs = index(&[]);
        let char_strings = index(&[b"\x0e", b"\xAA", b"\xBB", b"\xCC"]);

        let top_dict_index_len = build(0, 0).len();
        let after_top_dict = header.len() + names.len() + top_dict_index_len + strings.len() + global_subrs.len();
        let charstrings_offset = after_top_dict as u32;
        let charset_offset = charstrings_offset + char_strings.len() as u32;

        let charset_table = {
            let mut c = vec![0u8]; // format 0
            c.extend(1u16.to_be_bytes()); // glyph 1 -> SID 1 "space"
            c.extend(2u16.to_be_bytes()); // glyph 2 -> SID 2 "exclam"
            c.extend(3u16.to_be_bytes()); // glyph 3 -> SID 3 "quotedbl"
            c
        };

        let top_dict_index = build(charstrings_offset, charset_offset);
        assert_eq!(top_dict_index.len(), top_dict_index_len, "fixed-width offset encoding must not change size between passes");

        let mut out = header;
        out.extend(names);
        out.extend(top_dict_index);
        out.extend(strings);
        out.extend(global_subrs);
        out.extend(char_strings);
        out.extend(charset_table);
        out
    }

    #[test]
    fn subset_keeps_requested_glyphs_and_endchars_the_rest() {
        let raw = four_glyph_cff();
        let cff = Cff::parse(&raw).unwrap();
        assert_eq!(cff.num_glyphs, 4);

        let keep: HashSet<String> = ["exclam".to_string()].into_iter().collect();
        let subset = Subset::new(&cff, keep);

        // Round-trip the subset output through the real reader -- not a
        // second hand-check of the byte layout.
        let reparsed = Cff::parse(&subset.raw).unwrap();
        assert_eq!(reparsed.num_glyphs, 4, "dropped glyphs are stubbed, not removed -- glyph ids are preserved");
        assert_eq!(reparsed.char_strings[0], vec![0x0e]);
        assert_eq!(reparsed.char_strings[1], vec![0x0e], "unrequested glyph 1 (space) becomes a bare endchar");
        assert_eq!(reparsed.char_strings[2], vec![0xBB], "requested glyph 2 (exclam) keeps its real charstring");
        assert_eq!(reparsed.char_strings[3], vec![0x0e], "unrequested glyph 3 (quotedbl) becomes a bare endchar");
        assert_eq!(reparsed.charset.lookup(2).as_deref(), Some("exclam"));

        assert_eq!(subset.charname_map.get("exclam"), Some(&2));
        assert!(!subset.charname_map.contains_key("space"));
        assert!(!subset.charname_map.contains_key("quotedbl"));
        assert_eq!(subset.charname_map.get(".notdef"), Some(&0));
    }

    #[test]
    fn subset_always_keeps_notdef_even_if_not_requested() {
        let raw = four_glyph_cff();
        let cff = Cff::parse(&raw).unwrap();
        let subset = Subset::new(&cff, HashSet::new());
        assert_eq!(subset.charname_map.get(".notdef"), Some(&0));
        let reparsed = Cff::parse(&subset.raw).unwrap();
        assert_eq!(reparsed.char_strings[0], vec![0x0e]);
        assert_eq!(reparsed.char_strings[1], vec![0x0e]);
    }

    #[test]
    fn index_builder_compiles_an_empty_index_to_a_bare_zero_count() {
        assert_eq!(IndexBuilder::new().compile(), vec![0, 0]);
    }

    #[test]
    fn index_builder_round_trips_through_the_real_reader() {
        let mut idx = IndexBuilder::new();
        idx.push(b"AB".to_vec());
        idx.push(b"DEF".to_vec());
        idx.push(b"".to_vec());
        let raw = idx.compile();
        let (items, pos) = parse_index(&raw, 0).unwrap();
        assert_eq!(items, vec![b"AB".to_vec(), b"DEF".to_vec(), b"".to_vec()]);
        assert_eq!(pos, raw.len());
    }

    #[test]
    fn index_builder_picks_a_wider_offset_size_once_data_exceeds_255_bytes() {
        let mut idx = IndexBuilder::new();
        idx.push(vec![b'x'; 300]);
        let raw = idx.compile();
        assert_eq!(raw[2], 2, "300+1 byte offsets need the 2-byte form");
        let (items, _) = parse_index(&raw, 0).unwrap();
        assert_eq!(items[0].len(), 300);
    }

    #[test]
    fn strings_writer_reuses_real_standard_sids_without_growing() {
        let mut w = StringsWriter::new();
        assert_eq!(w.intern(".notdef"), 0);
        assert_eq!(w.intern("space"), 1);
        assert_eq!(w.compile(), vec![0, 0], "no new strings were ever interned");
    }

    #[test]
    fn strings_writer_assigns_fresh_sids_in_first_seen_order_and_is_idempotent() {
        let mut w = StringsWriter::new();
        let first = w.intern("MyCustomGlyphName");
        assert_eq!(first, CFF_STANDARD_STRINGS.len() as u16);
        assert_eq!(w.intern("MyCustomGlyphName"), first, "re-interning the same string returns the same SID");
        let second = w.intern("AnotherOne");
        assert_eq!(second, first + 1);
    }

    #[test]
    fn charsets_writer_round_trips_through_the_real_charset_reader() {
        let mut strings = StringsWriter::new();
        let writer = CharsetsWriter::new(vec!["space".to_string(), "exclam".to_string(), "MyGlyph".to_string()]);
        let compiled = writer.compile(&mut strings);
        // Offsets 0/1/2 are real CFF sentinels for the 3 predefined
        // charsets (see `Charset::parse`'s own early-return) -- pad 3
        // leading bytes so this custom table starts at a real,
        // non-sentinel offset, exactly as it always would inside an
        // actual CFF table.
        let mut raw = vec![0u8; 3];
        raw.extend(compiled);

        // Resolve via a combined standard+interned strings table, exactly
        // as a real font's Strings INDEX would present it to the reader.
        let mut resolved: Vec<String> = CFF_STANDARD_STRINGS.iter().map(|s| s.to_string()).collect();
        resolved.extend(strings.items.iter().cloned());

        let charset = Charset::parse(&raw, 3, &resolved, 4, false).unwrap();
        assert_eq!(charset.lookup(0).as_deref(), Some(".notdef"));
        assert_eq!(charset.lookup(1).as_deref(), Some("space"));
        assert_eq!(charset.lookup(2).as_deref(), Some("exclam"));
        assert_eq!(charset.lookup(3).as_deref(), Some("MyGlyph"));
    }
}
