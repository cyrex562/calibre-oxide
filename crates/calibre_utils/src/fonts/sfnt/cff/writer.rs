//! Partial port of `calibre.utils.fonts.sfnt.cff.writer` (issue #565,
//! split from #554/#65): the mechanical, self-contained CFF write-side
//! primitives -- the variable-offset-width INDEX writer
//! ([`IndexBuilder`], the reverse of #564's `table::parse_index`), the
//! SID-interning string table ([`StringsWriter`], the reverse of
//! #564's `Strings`), and the glyph-name-array charset writer
//! ([`CharsetsWriter`], the reverse of #564's format-0 `Charset`).
//!
//! # Not in this slice: `Subset`, and wiring into `sfnt::subset`
//!
//! Real upstream's `Subset` class is the actual subsetting
//! orchestrator: it ties every one of these writers together with a
//! two-pass offset-patching dance (compile the Top DICT with dummy
//! `charset`/`CharStrings`/`Private` offsets to learn its own encoded
//! size, then recompile with the real offsets once every other
//! section's size is known -- the same shape of problem #564's own
//! test fixture solved by using fixed-width offsets instead of a
//! two-pass guess). Building `Subset` needs one real design decision
//! this slice deliberately doesn't make: real Python's `strings`
//! parameter to `Dict.compile` is a *stateful* closure over a mutable
//! `Strings` object (interning as it goes), but
//! [`crate::fonts::sfnt::cff::dict_data::Dict::compile`]'s
//! `resolve_sid` parameter is `&dyn Fn(&str) -> u16` (no interior
//! mutation) -- wiring `StringsWriter::intern` (which needs `&mut
//! self`) into that call site needs an interior-mutability wrapper
//! (`RefCell<StringsWriter>` captured by the closure) at the call
//! site, a standard pattern but a real decision for whoever builds
//! `Subset`, not addressed here. `CFFTable`'s own container-level
//! wiring and the `sfnt::subset` CFF branch (currently reports
//! CFF-flavored fonts as `UnsupportedFont`) also remain, since neither
//! has a `Subset` to call yet. Left open rather than rushed.

use std::collections::HashMap;

use crate::fonts::sfnt::cff::constants::CFF_STANDARD_STRINGS;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::sfnt::cff::table::{parse_index, Charset};

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
