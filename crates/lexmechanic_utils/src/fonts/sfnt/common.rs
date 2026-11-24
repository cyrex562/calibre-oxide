//! Port of `calibre/utils/fonts/sfnt/common.py` -- the generic
//! OpenType layout table structures shared by GSUB (and, upstream,
//! GPOS) (issue #555).
//!
//! # Scope
//!
//! Issue #549 deliberately deferred this file here after confirming
//! `container.py` does not import it at all, and that its only real
//! importer is `gsub.py`. So everything here exists to serve
//! [`super::gsub`], and is shaped by what GSUB parsing actually needs.
//!
//! # `Unpackable` and why offsets are relative
//!
//! Almost every offset in an OpenType layout table is relative to the
//! start of the *subtable that contains it*, not to the table or the
//! font. [`Unpackable`] therefore records a `start_pos` when it is
//! created and exposes it, exactly as upstream's own class does --
//! this is the single most error-prone thing about parsing these
//! tables, and matching upstream's structure keeps the arithmetic
//! directly comparable.
//!
//! It is a separate type from the crate's existing
//! `fonts::utils::Cursor` because that cursor is purely sequential
//! with no seek, while this parsing is fundamentally
//! jump-to-an-offset-and-read.

use std::collections::{BTreeMap, BTreeSet};

use super::errors::UnsupportedFont;

fn bad(msg: impl Into<String>) -> UnsupportedFont {
    UnsupportedFont(msg.into())
}

/// Port of `common.Unpackable`: a big-endian reader over a table's raw
/// bytes that remembers where it started.
pub(crate) struct Unpackable<'a> {
    raw: &'a [u8],
    pos: usize,
    start_pos: usize,
}

impl<'a> Unpackable<'a> {
    pub(crate) fn new(raw: &'a [u8], offset: usize) -> Unpackable<'a> {
        Unpackable { raw, pos: offset, start_pos: offset }
    }

    /// The offset this reader was created at. Sibling offsets inside a
    /// subtable are relative to this.
    pub(crate) fn start_pos(&self) -> usize {
        self.start_pos
    }

    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub(crate) fn u16(&mut self) -> Result<u16, UnsupportedFont> {
        let end = self.pos.checked_add(2).ok_or_else(|| bad("offset overflow reading u16"))?;
        let b = self.raw.get(self.pos..end).ok_or_else(|| bad(format!("truncated table: no u16 at offset {}", self.pos)))?;
        self.pos = end;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, UnsupportedFont> {
        Ok(self.u16()? as i16)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, UnsupportedFont> {
        let end = self.pos.checked_add(4).ok_or_else(|| bad("offset overflow reading u32"))?;
        let b = self.raw.get(self.pos..end).ok_or_else(|| bad(format!("truncated table: no u32 at offset {}", self.pos)))?;
        self.pos = end;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn tag(&mut self) -> Result<[u8; 4], UnsupportedFont> {
        let end = self.pos.checked_add(4).ok_or_else(|| bad("offset overflow reading tag"))?;
        let b = self.raw.get(self.pos..end).ok_or_else(|| bad(format!("truncated table: no tag at offset {}", self.pos)))?;
        self.pos = end;
        Ok([b[0], b[1], b[2], b[3]])
    }

    pub(crate) fn u16s(&mut self, count: usize) -> Result<Vec<u16>, UnsupportedFont> {
        (0..count).map(|_| self.u16()).collect()
    }
}

/// Port of `common.CoverageRange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageRange {
    pub start: u16,
    pub end: u16,
    pub start_coverage_index: u16,
}

/// Port of `common.Coverage`: which glyphs a lookup subtable applies
/// to, and at what index within that subtable's own arrays.
#[derive(Debug, Clone)]
pub enum Coverage {
    /// Format 1: an explicit list of glyph ids.
    Glyphs(BTreeMap<u16, u16>),
    /// Format 2: ranges of glyph ids.
    Ranges(Vec<CoverageRange>),
}

impl Coverage {
    pub(crate) fn parse(raw: &[u8], offset: usize, parent: &str) -> Result<Coverage, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let format = data.u16()?;
        let count = data.u16()? as usize;

        match format {
            1 => {
                let ids = data.u16s(count)?;
                Ok(Coverage::Glyphs(ids.into_iter().enumerate().map(|(i, gid)| (gid, i as u16)).collect()))
            }
            2 => {
                let mut ranges = Vec::with_capacity(count);
                for _ in 0..count {
                    let start = data.u16()?;
                    let end = data.u16()?;
                    let start_coverage_index = data.u16()?;
                    ranges.push(CoverageRange { start, end, start_coverage_index });
                }
                Ok(Coverage::Ranges(ranges))
            }
            other => Err(bad(format!("Unknown Coverage format: 0x{other:x} in {parent}"))),
        }
    }

    /// Port of `Coverage.coverage_indices`: `(glyph_id, coverage index)`
    /// for exactly those `glyph_ids` this coverage covers.
    ///
    /// Upstream returns an `OrderedDict` built by iterating a Python
    /// `set`, whose order is unspecified. Iterating a `BTreeSet` here
    /// makes the result deterministic instead -- the callers either
    /// collect into a set (order-insensitive) or, for
    /// [`super::gsub::SubstitutionSubtable::Ligature`], test set
    /// membership, so determinism changes no outcome and makes the
    /// tests meaningful.
    pub fn coverage_indices(&self, glyph_ids: &BTreeSet<u16>) -> Vec<(u16, u16)> {
        let mut ans = Vec::new();
        for &gid in glyph_ids {
            match self {
                Coverage::Glyphs(map) => {
                    if let Some(&idx) = map.get(&gid) {
                        ans.push((gid, idx));
                    }
                }
                Coverage::Ranges(ranges) => {
                    for r in ranges {
                        if r.start <= gid && gid <= r.end {
                            // Upstream does not break here, so a font
                            // with overlapping ranges keeps the LAST
                            // match. Preserved deliberately.
                            let idx = r.start_coverage_index.wrapping_add(gid - r.start);
                            match ans.iter_mut().find(|(g, _)| *g == gid) {
                                Some(slot) => slot.1 = idx,
                                None => ans.push((gid, idx)),
                            }
                        }
                    }
                }
            }
        }
        ans
    }
}

/// Port of `common.LanguageSystemTable`.
#[derive(Debug, Clone)]
pub struct LanguageSystemTable {
    pub lookup_order: u16,
    pub required_feature_index: u16,
    /// The `IndexTable` body: feature indices.
    pub feature_indices: Vec<u16>,
}

impl LanguageSystemTable {
    fn parse(raw: &[u8], offset: usize) -> Result<LanguageSystemTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let lookup_order = data.u16()?;
        let required_feature_index = data.u16()?;
        if lookup_order != 0 {
            return Err(bad(format!("This LanguageSystemTable has an unknown lookup order: 0x{lookup_order:x}")));
        }
        let count = data.u16()? as usize;
        let feature_indices = data.u16s(count)?;
        Ok(LanguageSystemTable { lookup_order, required_feature_index, feature_indices })
    }
}

/// Port of `common.ScriptTable`.
#[derive(Debug, Clone)]
pub struct ScriptTable {
    /// The `b'default'` entry upstream stores alongside the tagged
    /// ones; `None` when the font's default-offset is 0.
    pub default: Option<LanguageSystemTable>,
    pub language_systems: Vec<([u8; 4], LanguageSystemTable)>,
}

impl ScriptTable {
    fn parse(raw: &[u8], offset: usize) -> Result<ScriptTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        // `read_extra_header`: the default-langsys offset, relative to
        // this table's own start.
        let default_offset = data.u16()? as usize;
        let default = if default_offset != 0 { Some(LanguageSystemTable::parse(raw, data.start_pos() + default_offset)?) } else { None };

        let count = data.u16()? as usize;
        let mut language_systems = Vec::with_capacity(count);
        for _ in 0..count {
            let tag = data.tag()?;
            let coffset = data.u16()? as usize;
            language_systems.push((tag, LanguageSystemTable::parse(raw, data.start_pos() + coffset)?));
        }
        Ok(ScriptTable { default, language_systems })
    }
}

/// Port of `common.ScriptListTable`.
#[derive(Debug, Clone, Default)]
pub struct ScriptListTable(pub Vec<([u8; 4], ScriptTable)>);

impl ScriptListTable {
    pub(crate) fn parse(raw: &[u8], offset: usize) -> Result<ScriptListTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let count = data.u16()? as usize;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let tag = data.tag()?;
            let coffset = data.u16()? as usize;
            out.push((tag, ScriptTable::parse(raw, data.start_pos() + coffset)?));
        }
        Ok(ScriptListTable(out))
    }
}

/// Port of `common.FeatureTable`.
#[derive(Debug, Clone)]
pub struct FeatureTable {
    pub feature_params: u16,
    /// The `IndexTable` body: lookup indices.
    pub lookup_indices: Vec<u16>,
}

impl FeatureTable {
    fn parse(raw: &[u8], offset: usize) -> Result<FeatureTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        // Upstream has a disabled check here rejecting a non-NULL
        // FeatureParams, with the comment "Source code pro sets this to
        // non NULL". Kept disabled for the same reason -- real fonts in
        // the wild set it.
        let feature_params = data.u16()?;
        let count = data.u16()? as usize;
        let lookup_indices = data.u16s(count)?;
        Ok(FeatureTable { feature_params, lookup_indices })
    }
}

/// Port of `common.FeatureListTable`.
#[derive(Debug, Clone, Default)]
pub struct FeatureListTable(pub Vec<([u8; 4], FeatureTable)>);

impl FeatureListTable {
    pub(crate) fn parse(raw: &[u8], offset: usize) -> Result<FeatureListTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let count = data.u16()? as usize;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let tag = data.tag()?;
            let coffset = data.u16()? as usize;
            out.push((tag, FeatureTable::parse(raw, data.start_pos() + coffset)?));
        }
        Ok(FeatureListTable(out))
    }
}

/// Port of `UnknownLookupSubTable.read_sets`: reads a count-prefixed
/// array of offsets to sets, each itself a count-prefixed array of
/// offsets to items.
///
/// Upstream folds two genuinely different modes into one function via
/// a `set_is_index` flag. They are split here because they return
/// different things: [`read_index_sets`] collects the raw "offsets",
/// which in that mode are really glyph ids, while this one seeks to
/// each and reads a real item.
pub(crate) fn read_item_sets<T>(
    data: &mut Unpackable<'_>,
    mut read_item: impl FnMut(&mut Unpackable<'_>) -> Result<T, UnsupportedFont>,
) -> Result<Vec<Vec<T>>, UnsupportedFont> {
    each_set(data, |data, set_start, item_offset| {
        data.seek(set_start + item_offset as usize);
        read_item(data)
    })
}

/// [`read_item_sets`]'s sibling for upstream's `set_is_index=True`
/// mode, where each stored "offset" is itself the value.
pub(crate) fn read_index_sets(data: &mut Unpackable<'_>) -> Result<Vec<Vec<u16>>, UnsupportedFont> {
    each_set(data, |_, _, item_offset| Ok(item_offset))
}

fn each_set<T>(
    data: &mut Unpackable<'_>,
    mut handle: impl FnMut(&mut Unpackable<'_>, usize, u16) -> Result<T, UnsupportedFont>,
) -> Result<Vec<Vec<T>>, UnsupportedFont> {
    let count = data.u16()? as usize;
    let set_offsets = data.u16s(count)?;
    let base = data.start_pos();

    let mut out = Vec::with_capacity(set_offsets.len());
    for set_offset in set_offsets {
        let set_start = base + set_offset as usize;
        data.seek(set_start);
        let item_count = data.u16()? as usize;
        let item_offsets = data.u16s(item_count)?;

        let mut items = Vec::with_capacity(item_offsets.len());
        for item_offset in item_offsets {
            items.push(handle(data, set_start, item_offset)?);
        }
        out.push(items);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn a_format_1_coverage_maps_each_glyph_to_its_list_index() {
        // format=1, count=3, glyphs 10/20/30
        let raw = be(&[1, 3, 10, 20, 30]);
        let cov = Coverage::parse(&raw, 0, "test").unwrap();

        let ids: BTreeSet<u16> = [10, 20, 30].into_iter().collect();
        assert_eq!(cov.coverage_indices(&ids), vec![(10, 0), (20, 1), (30, 2)]);
    }

    #[test]
    fn a_format_1_coverage_ignores_glyphs_it_does_not_cover() {
        let raw = be(&[1, 2, 10, 20]);
        let cov = Coverage::parse(&raw, 0, "test").unwrap();

        let ids: BTreeSet<u16> = [10, 99].into_iter().collect();
        assert_eq!(cov.coverage_indices(&ids), vec![(10, 0)], "glyph 99 is not covered and must not appear");
    }

    #[test]
    fn a_format_2_coverage_computes_the_index_within_its_range() {
        // format=2, count=1, range start=10 end=14 start_coverage_index=100
        let raw = be(&[2, 1, 10, 14, 100]);
        let cov = Coverage::parse(&raw, 0, "test").unwrap();

        let ids: BTreeSet<u16> = [10, 12, 14, 15].into_iter().collect();
        assert_eq!(cov.coverage_indices(&ids), vec![(10, 100), (12, 102), (14, 104)], "15 is outside the range");
    }

    #[test]
    fn an_unknown_coverage_format_is_refused_with_the_parent_named() {
        let raw = be(&[7, 0]);
        let err = Coverage::parse(&raw, 0, "SomeSubtable").unwrap_err();
        assert!(err.0.contains("Unknown Coverage format"), "{}", err.0);
        assert!(err.0.contains("SomeSubtable"), "the message should name the parent table: {}", err.0);
    }

    #[test]
    fn a_truncated_coverage_is_an_error_not_a_panic() {
        // Claims 5 glyphs but supplies none.
        let raw = be(&[1, 5]);
        assert!(Coverage::parse(&raw, 0, "test").is_err());
    }

    #[test]
    fn a_language_system_table_with_a_nonzero_lookup_order_is_refused() {
        // lookup_order=1 (must be 0), required=0, count=0
        let raw = be(&[1, 0, 0]);
        let err = LanguageSystemTable::parse(&raw, 0).unwrap_err();
        assert!(err.0.contains("unknown lookup order"), "{}", err.0);
    }

    #[test]
    fn a_real_language_system_table_reads_its_feature_indices() {
        let raw = be(&[0, 0xFFFF, 3, 7, 8, 9]);
        let t = LanguageSystemTable::parse(&raw, 0).unwrap();
        assert_eq!(t.required_feature_index, 0xFFFF);
        assert_eq!(t.feature_indices, [7, 8, 9]);
    }

    #[test]
    fn a_feature_table_tolerates_a_non_null_feature_params() {
        // Upstream's check for this is deliberately disabled because
        // real fonts (Source Code Pro) set it; this must not reject.
        let raw = be(&[0x1234, 2, 4, 5]);
        let t = FeatureTable::parse(&raw, 0).unwrap();
        assert_eq!(t.feature_params, 0x1234);
        assert_eq!(t.lookup_indices, [4, 5]);
    }

    #[test]
    fn unpackable_offsets_are_relative_to_where_the_reader_started() {
        let raw = be(&[0, 0, 42]);
        let mut d = Unpackable::new(&raw, 4);
        assert_eq!(d.start_pos(), 4);
        assert_eq!(d.u16().unwrap(), 42);
    }
}
