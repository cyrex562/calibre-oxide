//! Port of `calibre/utils/fonts/sfnt/gsub.py` -- the GSUB
//! (glyph substitution) table (issue #555).
//!
//! # Why this exists, concretely
//!
//! [`super::subset`] already threads an `extra_glyphs` set through
//! both [`super::subset::subset_truetype`] and
//! [`super::subset::subset_cff`], but before this module it was
//! **always empty**, with a disclosed narrowing saying so.
//!
//! That set is what keeps glyphs reachable *only* through an OpenType
//! substitution rule from being dropped by subsetting. Concretely: if
//! a font maps `f` and `i` to glyphs, and a ligature rule substitutes
//! the pair for an `ﬁ` glyph that no character maps to directly,
//! subsetting without GSUB silently deletes the `ﬁ` glyph and the
//! ligature stops rendering. [`GsubTable::all_substitutions`] is the
//! closure that prevents that.
//!
//! # Faithfulness notes
//!
//! - **Upstream's final `return ans - {glyph_ids}` is a no-op bug.**
//!   `ans` is a set of glyph ids while `{glyph_ids}` is a set
//!   *containing a frozenset*, so the subtraction removes nothing and
//!   upstream effectively returns the full closure including the
//!   originals. The intent was plainly `ans - glyph_ids`. This port
//!   returns the full closure, which is what upstream really returns
//!   -- and it is harmless either way, because the one real caller
//!   immediately unions the result back with the character map
//!   (`unresolved_glyphs = set(character_map.values()) | extra_glyphs`).
//!   Reproducing a dead expression would have been faithfulness to the
//!   typo rather than to the behavior.
//! - **Contextual and chaining-contextual subtables (types 5 and 6)
//!   contribute nothing**, matching upstream: their `initialize` is
//!   `pass  # TODO` and their `all_substitutions` returns an empty set,
//!   because they express substitutions only in terms of *other*
//!   lookups. Faithfully inert here too, rather than silently
//!   "improved".
//! - **Type 7 (Extension) is a redirect**, not a substitution: it
//!   names another lookup type and an offset, and is resolved to that
//!   type's subtable at parse time.
//! - `all_substitutions` makes **one pass** over lookups, accumulating
//!   into the working set as it goes (so a later lookup sees earlier
//!   lookups' output), exactly as upstream does. It is not iterated to
//!   a fixpoint.

use std::collections::BTreeSet;

use super::common::{read_index_sets, read_item_sets, Coverage, FeatureListTable, ScriptListTable, Unpackable};
use super::errors::UnsupportedFont;

fn bad(msg: impl Into<String>) -> UnsupportedFont {
    UnsupportedFont(msg.into())
}

/// One ligature rule: the glyph produced, and the components that must
/// all be present after the first (covered) glyph.
#[derive(Debug, Clone)]
pub struct Ligature {
    pub glyph: u16,
    pub components: Vec<u16>,
}

/// The GSUB lookup subtable types this port understands.
///
/// Port of `gsub.py`'s `subtable_map` plus the `UnknownLookupSubTable`
/// base each entry derives from.
#[derive(Debug, Clone)]
pub enum SubstitutionSubtable {
    /// Type 1 format 1: every covered glyph maps to `gid + delta`.
    SingleDelta { coverage: Coverage, delta: i16 },
    /// Type 1 format 2: an explicit substitute per coverage index.
    SingleList { coverage: Coverage, substitutes: Vec<u16> },
    /// Types 2 and 3 (Multiple/Alternate, identical in structure).
    MultipleOrAlternate { coverage: Coverage, sets: Vec<Vec<u16>> },
    /// Type 4: ligatures.
    Ligature { coverage: Coverage, sets: Vec<Vec<Ligature>> },
    /// Types 5 and 6: contribute nothing; see the module doc.
    Contextual,
    /// Type 8: reverse chaining single substitution.
    ReverseChainSingle { coverage: Coverage, substitutes: Vec<u16> },
}

impl SubstitutionSubtable {
    /// Port of each subtable class's `all_substitutions`.
    pub fn all_substitutions(&self, glyph_ids: &BTreeSet<u16>) -> BTreeSet<u16> {
        match self {
            SubstitutionSubtable::SingleDelta { coverage, delta } => coverage
                .coverage_indices(glyph_ids)
                .into_iter()
                // Upstream computes `gid + self.delta` in Python ints,
                // where it cannot wrap. A real font's delta keeps the
                // result a valid glyph id; wrapping here just avoids a
                // panic on a malformed one.
                .map(|(gid, _)| gid.wrapping_add(*delta as u16))
                .collect(),
            SubstitutionSubtable::SingleList { coverage, substitutes } => {
                coverage.coverage_indices(glyph_ids).into_iter().filter_map(|(_, idx)| substitutes.get(idx as usize).copied()).collect()
            }
            SubstitutionSubtable::MultipleOrAlternate { coverage, sets } => {
                let mut ans = BTreeSet::new();
                for (_, idx) in coverage.coverage_indices(glyph_ids) {
                    if let Some(set) = sets.get(idx as usize) {
                        ans.extend(set.iter().copied());
                    }
                }
                ans
            }
            SubstitutionSubtable::Ligature { coverage, sets } => {
                let mut ans = BTreeSet::new();
                for (start_glyph_id, idx) in coverage.coverage_indices(glyph_ids) {
                    let Some(set) = sets.get(idx as usize) else { continue };
                    for lig in set {
                        // The full component sequence is the covered
                        // glyph followed by the stored components; the
                        // ligature is only reachable if EVERY component
                        // is present.
                        let present = std::iter::once(start_glyph_id).chain(lig.components.iter().copied()).all(|c| glyph_ids.contains(&c));
                        if present {
                            ans.insert(lig.glyph);
                        }
                    }
                }
                ans
            }
            SubstitutionSubtable::Contextual => BTreeSet::new(),
            SubstitutionSubtable::ReverseChainSingle { coverage, substitutes } => {
                coverage.coverage_indices(glyph_ids).into_iter().filter_map(|(_, idx)| substitutes.get(idx as usize).copied()).collect()
            }
        }
    }

    fn parse(raw: &[u8], offset: usize, lookup_type: u16) -> Result<SubstitutionSubtable, UnsupportedFont> {
        match lookup_type {
            1 => Self::parse_single(raw, offset),
            2 | 3 => Self::parse_multiple(raw, offset),
            4 => Self::parse_ligature(raw, offset),
            5 | 6 => Self::parse_contextual(raw, offset),
            7 => Self::parse_extension(raw, offset),
            8 => Self::parse_reverse_chain(raw, offset),
            other => Err(bad(format!("Unknown GSUB lookup type: {other}"))),
        }
    }

    /// Reads the shared `format` + initial coverage-offset preamble
    /// that `UnknownLookupSubTable.__init__` applies to every subtable
    /// with `has_initial_coverage`.
    fn read_format_and_coverage<'a>(raw: &'a [u8], offset: usize, name: &str) -> Result<(u16, Coverage, Unpackable<'a>), UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let format = data.u16()?;
        let coverage_offset = data.u16()? as usize + data.start_pos();
        let coverage = Coverage::parse(raw, coverage_offset, name)?;
        Ok((format, coverage, data))
    }

    fn parse_single(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        let (format, coverage, mut data) = Self::read_format_and_coverage(raw, offset, "SingleSubstitution")?;
        match format {
            1 => Ok(SubstitutionSubtable::SingleDelta { coverage, delta: data.i16()? }),
            2 => {
                let count = data.u16()? as usize;
                Ok(SubstitutionSubtable::SingleList { coverage, substitutes: data.u16s(count)? })
            }
            other => Err(bad(format!("Unknown format for Lookup Subtable SingleSubstitution: 0x{other:x}"))),
        }
    }

    fn parse_multiple(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        let (format, coverage, mut data) = Self::read_format_and_coverage(raw, offset, "MultipleSubstitution")?;
        if format != 1 {
            return Err(bad(format!("Unknown format for Lookup Subtable MultipleSubstitution: 0x{format:x}")));
        }
        // The stored "offsets" are the substitute glyph ids.
        let sets = read_index_sets(&mut data)?;
        Ok(SubstitutionSubtable::MultipleOrAlternate { coverage, sets })
    }

    fn parse_ligature(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        let (format, coverage, mut data) = Self::read_format_and_coverage(raw, offset, "LigatureSubstitution")?;
        if format != 1 {
            return Err(bad(format!("Unknown format for Lookup Subtable LigatureSubstitution: 0x{format:x}")));
        }
        let sets = read_item_sets(&mut data, |d| {
            // Port of `read_ligature`: the produced glyph, then a
            // component count that INCLUDES the covered first glyph,
            // so only `count - 1` components are stored.
            let glyph = d.u16()?;
            let count = d.u16()?;
            let components = d.u16s(count.saturating_sub(1) as usize)?;
            Ok(Ligature { glyph, components })
        })?;
        Ok(SubstitutionSubtable::Ligature { coverage, sets })
    }

    fn parse_contextual(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        // Format 3 has no initial coverage (`has_initial_coverage`
        // returns `self.format != 3`), so only the format is validated.
        // `initialize` is `pass` upstream, so nothing else is read.
        let mut data = Unpackable::new(raw, offset);
        let format = data.u16()?;
        if !matches!(format, 1 | 2 | 3) {
            return Err(bad(format!("Unknown format for Lookup Subtable ContextualSubstitution: 0x{format:x}")));
        }
        if format != 3 {
            let coverage_offset = data.u16()? as usize + data.start_pos();
            // Parsed for validation only: upstream builds it too, and a
            // malformed coverage must reject the table the same way.
            Coverage::parse(raw, coverage_offset, "ContextualSubstitution")?;
        }
        Ok(SubstitutionSubtable::Contextual)
    }

    fn parse_extension(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        // Port of `ExtensionSubstitution`: a redirect to a subtable of
        // another type, at an offset relative to the extension record.
        let mut data = Unpackable::new(raw, offset);
        let subst_format = data.u16()?;
        let extension_lookup_type = data.u16()?;
        let extension_offset = data.u32()? as usize;
        if subst_format != 1 {
            return Err(bad(format!("ExtensionSubstitution has unknown format: 0x{subst_format:x}")));
        }
        if extension_lookup_type == 7 {
            // Not reachable in a valid font, and following it would let
            // a malformed one recurse forever.
            return Err(bad("ExtensionSubstitution may not point at another extension"));
        }
        Self::parse(raw, data.start_pos() + extension_offset, extension_lookup_type)
    }

    fn parse_reverse_chain(raw: &[u8], offset: usize) -> Result<SubstitutionSubtable, UnsupportedFont> {
        let (format, coverage, mut data) = Self::read_format_and_coverage(raw, offset, "ReverseChainSingleSubstitution")?;
        if format != 1 {
            return Err(bad(format!("Unknown format for Lookup Subtable ReverseChainSingleSubstitution: 0x{format:x}")));
        }
        // Backtrack and lookahead coverage offsets are read past but
        // unused, exactly as upstream ("TODO: Use these").
        let backtrack_count = data.u16()? as usize;
        let _backtrack = data.u16s(backtrack_count)?;
        let lookahead_count = data.u16()? as usize;
        let _lookahead = data.u16s(lookahead_count)?;

        let count = data.u16()? as usize;
        Ok(SubstitutionSubtable::ReverseChainSingle { coverage, substitutes: data.u16s(count)? })
    }
}

/// Port of `gsub.GSUBLookupTable` + `LookupListTable`: one lookup, i.e.
/// an ordered list of subtables that share a type and flags.
#[derive(Debug, Clone)]
pub struct LookupTable {
    pub lookup_type: u16,
    pub lookup_flag: u16,
    pub mark_filtering_set: Option<u16>,
    pub subtables: Vec<SubstitutionSubtable>,
}

impl LookupTable {
    fn parse(raw: &[u8], offset: usize) -> Result<LookupTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, offset);
        let lookup_type = data.u16()?;
        let lookup_flag = data.u16()?;
        let count = data.u16()? as usize;
        let offsets = data.u16s(count)?;

        let mut subtables = Vec::with_capacity(offsets.len());
        for o in &offsets {
            subtables.push(SubstitutionSubtable::parse(raw, data.start_pos() + *o as usize, lookup_type)?);
        }

        // Port of `read_extra_footer`: the mark-filtering set follows
        // the subtable offsets when bit 4 of the flags is set.
        let mark_filtering_set = if lookup_flag & 0x0010 != 0 { Some(data.u16()?) } else { None };

        Ok(LookupTable { lookup_type, lookup_flag, mark_filtering_set, subtables })
    }
}

/// Port of `gsub.GSUBTable`.
#[derive(Debug, Clone)]
pub struct GsubTable {
    pub version: u32,
    pub script_list: ScriptListTable,
    pub feature_list: FeatureListTable,
    pub lookups: Vec<LookupTable>,
}

impl GsubTable {
    /// Port of `GSUBTable.decompile`.
    ///
    /// The script and feature lists are parsed but unused by
    /// [`GsubTable::all_substitutions`] (which only walks the lookup
    /// list), exactly as upstream. They are still parsed because a
    /// font whose script/feature lists are malformed is one upstream
    /// rejects, and rejecting the same fonts keeps behavior comparable.
    pub fn parse(raw: &[u8]) -> Result<GsubTable, UnsupportedFont> {
        let mut data = Unpackable::new(raw, 0);
        let version = data.u32()?;
        let scriptlist_offset = data.u16()? as usize;
        let featurelist_offset = data.u16()? as usize;
        let lookuplist_offset = data.u16()? as usize;

        if version != 0x0001_0000 {
            return Err(bad(format!("The GSUB table has unknown version: 0x{version:x}")));
        }

        let script_list = ScriptListTable::parse(raw, scriptlist_offset)?;
        let feature_list = FeatureListTable::parse(raw, featurelist_offset)?;

        let mut lookup_data = Unpackable::new(raw, lookuplist_offset);
        let count = lookup_data.u16()? as usize;
        let offsets = lookup_data.u16s(count)?;
        let mut lookups = Vec::with_capacity(offsets.len());
        for o in offsets {
            lookups.push(LookupTable::parse(raw, lookup_data.start_pos() + o as usize)?);
        }

        Ok(GsubTable { version, script_list, feature_list, lookups })
    }

    /// Port of `GSUBTable.all_substitutions`: every glyph reachable
    /// from `glyph_ids` by any substitution rule, **including the
    /// inputs** (see the module doc for why that matches upstream).
    pub fn all_substitutions(&self, glyph_ids: impl IntoIterator<Item = u16>) -> BTreeSet<u16> {
        let mut ans: BTreeSet<u16> = glyph_ids.into_iter().collect();
        for lookup in &self.lookups {
            for subtable in &lookup.subtables {
                let found = subtable.all_substitutions(&ans);
                if !found.is_empty() {
                    ans.extend(found);
                }
            }
        }
        ans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    /// format=1 coverage over `glyphs`, as a standalone byte block.
    fn coverage1(glyphs: &[u16]) -> Vec<u8> {
        let mut v = vec![1u16, glyphs.len() as u16];
        v.extend_from_slice(glyphs);
        be(&v)
    }

    #[test]
    fn a_single_delta_substitution_shifts_every_covered_glyph() {
        // SingleSubst format 1: format, coverage_offset=6, delta=100
        let mut raw = be(&[1, 6, 100]);
        assert_eq!(raw.len(), 6);
        raw.extend(coverage1(&[10, 11]));

        let sub = SubstitutionSubtable::parse(&raw, 0, 1).unwrap();
        let got = sub.all_substitutions(&[10, 11, 99].into_iter().collect());
        assert_eq!(got, [110, 111].into_iter().collect::<BTreeSet<_>>(), "99 is uncovered; 10/11 shift by the delta");
    }

    #[test]
    fn a_single_list_substitution_maps_by_coverage_index() {
        // format=2, coverage_offset, count=2, substitutes 500/600.
        // Header is 8 bytes, so the coverage goes straight after it.
        let header = be(&[2, 8, 2, 500, 600]);
        assert_eq!(header.len(), 10, "header layout changed; the coverage offset below must follow it");

        // Coverage sits after the whole header, so point at 10.
        let mut raw = be(&[2, 10, 2, 500, 600]);
        raw.extend(coverage1(&[10, 11]));

        let sub = SubstitutionSubtable::parse(&raw, 0, 1).unwrap();
        let got = sub.all_substitutions(&[10, 11].into_iter().collect());
        assert_eq!(got, [500, 600].into_iter().collect::<BTreeSet<_>>(), "coverage index 0 -> 500, index 1 -> 600");
    }

    #[test]
    fn a_ligature_fires_only_when_every_component_is_present() {
        // Build a LigatureSubst: format=1, coverage_offset, then one
        // ligature set for covered glyph 10 producing glyph 900 from
        // components (10, 20).
        //
        // Layout: [format][covOff][setCount=1][setOff] | coverage | set
        let header_len = 8usize;
        let coverage = coverage1(&[10]);
        let cov_off = header_len;
        let set_off = header_len + coverage.len();

        // LigatureSet: [ligCount=1][ligOff=4] then the Ligature record.
        let lig_set = {
            let mut v = be(&[1, 4]);
            // Ligature: glyph=900, comp_count=2 (includes the covered
            // glyph), then 1 stored component = 20
            v.extend(be(&[900, 2, 20]));
            v
        };

        let mut raw = be(&[1, cov_off as u16, 1, set_off as u16]);
        raw.extend(coverage);
        raw.extend(lig_set);

        let sub = SubstitutionSubtable::parse(&raw, 0, 4).unwrap();

        // Both components present -> ligature reachable.
        let got = sub.all_substitutions(&[10, 20].into_iter().collect());
        assert_eq!(got, [900].into_iter().collect::<BTreeSet<_>>());

        // Component 20 missing -> ligature must NOT be kept. This is
        // the whole point of the component check.
        let got = sub.all_substitutions(&[10].into_iter().collect());
        assert!(got.is_empty(), "a ligature whose components aren't all present must not be reachable");
    }

    #[test]
    fn a_contextual_subtable_contributes_nothing_like_upstream() {
        // format 3 has no initial coverage.
        let raw = be(&[3, 0, 0]);
        let sub = SubstitutionSubtable::parse(&raw, 0, 5).unwrap();
        assert!(sub.all_substitutions(&[1, 2, 3].into_iter().collect()).is_empty());
    }

    #[test]
    fn an_extension_subtable_resolves_to_the_type_it_names() {
        // Extension record: format=1, type=1 (Single), offset -> a real
        // SingleSubst delta subtable placed after it.
        let ext_len = 8usize;
        let inner_off = ext_len;
        let mut raw = be(&[1, 1]);
        raw.extend((inner_off as u32).to_be_bytes());
        assert_eq!(raw.len(), ext_len);

        // inner SingleSubst format 1 at `inner_off`
        let inner_cov_off = 6usize;
        let mut inner = be(&[1, inner_cov_off as u16, 7]);
        inner.extend(coverage1(&[10]));
        raw.extend(inner);

        let sub = SubstitutionSubtable::parse(&raw, 0, 7).unwrap();
        let got = sub.all_substitutions(&[10].into_iter().collect());
        assert_eq!(got, [17].into_iter().collect::<BTreeSet<_>>(), "the extension should have resolved to the Single delta subtable");
    }

    #[test]
    fn an_extension_pointing_at_another_extension_is_refused() {
        let mut raw = be(&[1, 7]);
        raw.extend(0u32.to_be_bytes());
        assert!(SubstitutionSubtable::parse(&raw, 0, 7).is_err(), "self-referential extensions must not be followed");
    }

    #[test]
    fn an_unknown_lookup_type_is_refused() {
        assert!(SubstitutionSubtable::parse(&be(&[1, 0]), 0, 99).is_err());
    }

    #[test]
    fn a_gsub_table_with_the_wrong_version_is_refused() {
        let mut raw = 0x0002_0000u32.to_be_bytes().to_vec();
        raw.extend(be(&[0, 0, 0]));
        let err = GsubTable::parse(&raw).unwrap_err();
        assert!(err.0.contains("unknown version"), "{}", err.0);
    }

    #[test]
    fn all_substitutions_returns_the_inputs_plus_what_they_reach() {
        // Upstream's final subtraction is a no-op, so the inputs come
        // back too -- see the module doc.
        let mut raw = be(&[1, 6, 100]);
        raw.extend(coverage1(&[10]));
        let sub = SubstitutionSubtable::parse(&raw, 0, 1).unwrap();

        let table = GsubTable {
            version: 0x0001_0000,
            script_list: Default::default(),
            feature_list: Default::default(),
            lookups: vec![LookupTable { lookup_type: 1, lookup_flag: 0, mark_filtering_set: None, subtables: vec![sub] }],
        };

        let got = table.all_substitutions([10u16, 55]);
        assert!(got.contains(&10), "inputs are included");
        assert!(got.contains(&55), "uncovered inputs are included");
        assert!(got.contains(&110), "and the substitution is reachable");
    }

    #[test]
    fn later_lookups_see_earlier_lookups_output_in_the_single_pass() {
        // lookup A: 10 -> 20 (delta 10). lookup B: 20 -> 40 (delta 20).
        // A single accumulating pass must reach 40 from input 10.
        fn delta_sub(covered: u16, delta: u16) -> SubstitutionSubtable {
            let mut raw = be(&[1, 6, delta]);
            raw.extend(coverage1(&[covered]));
            SubstitutionSubtable::parse(&raw, 0, 1).unwrap()
        }

        let table = GsubTable {
            version: 0x0001_0000,
            script_list: Default::default(),
            feature_list: Default::default(),
            lookups: vec![
                LookupTable { lookup_type: 1, lookup_flag: 0, mark_filtering_set: None, subtables: vec![delta_sub(10, 10)] },
                LookupTable { lookup_type: 1, lookup_flag: 0, mark_filtering_set: None, subtables: vec![delta_sub(20, 20)] },
            ],
        };

        let got = table.all_substitutions([10u16]);
        assert!(got.contains(&20));
        assert!(got.contains(&40), "the second lookup should have seen the first's output");
    }

    /// Cross-validation against **real upstream Python** on a **real
    /// font**: `DejaVuSans.ttf`'s own 5598-byte GSUB table.
    ///
    /// The expected set was produced by executing upstream calibre's
    /// own unmodified `gsub.py`/`common.py` (with only the two tiny
    /// `FixedProperty`/`UnknownTable` symbols faked, since they are all
    /// it imports from calibre) against this exact font and these exact
    /// input glyph ids.
    ///
    /// Skipped rather than failed when the font is absent, matching how
    /// this crate's other system-resource-dependent tests degrade.
    #[test]
    fn matches_real_upstream_python_on_a_real_font() {
        const FONT: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
        let Ok(data) = std::fs::read(FONT) else {
            eprintln!("skipping: {FONT} not present on this machine");
            return;
        };

        // Pull the GSUB table straight out of the sfnt directory.
        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
        let mut gsub_raw = None;
        for i in 0..num_tables {
            let rec = 12 + 16 * i;
            if &data[rec..rec + 4] == b"GSUB" {
                let off = u32::from_be_bytes(data[rec + 8..rec + 12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(data[rec + 12..rec + 16].try_into().unwrap()) as usize;
                gsub_raw = Some(data[off..off + len].to_vec());
                break;
            }
        }
        let gsub_raw = gsub_raw.expect("DejaVuSans should have a GSUB table");
        assert_eq!(gsub_raw.len(), 5598, "font differs from the one the expectation was generated against");

        let table = GsubTable::parse(&gsub_raw).expect("real DejaVuSans GSUB should parse");
        let got = table.all_substitutions([1u16, 2, 3, 4, 5, 36, 37, 38, 69, 70, 76]);

        let expected: BTreeSet<u16> = [1, 2, 3, 4, 5, 36, 37, 38, 69, 70, 76, 243, 2847].into_iter().collect();
        assert_eq!(got, expected, "diverged from real upstream calibre's own output for the same font and inputs");
    }

    /// The real font's GSUB must parse into something substantial --
    /// guards against a "parses to zero lookups, therefore trivially
    /// agrees with nothing" false pass.
    #[test]
    fn a_real_fonts_gsub_parses_into_real_lookups() {
        const FONT: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
        let Ok(data) = std::fs::read(FONT) else { return };
        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
        let mut gsub_raw = None;
        for i in 0..num_tables {
            let rec = 12 + 16 * i;
            if &data[rec..rec + 4] == b"GSUB" {
                let off = u32::from_be_bytes(data[rec + 8..rec + 12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(data[rec + 12..rec + 16].try_into().unwrap()) as usize;
                gsub_raw = Some(data[off..off + len].to_vec());
            }
        }
        let table = GsubTable::parse(&gsub_raw.unwrap()).unwrap();
        assert!(table.lookups.len() > 5, "expected a real lookup list, got {}", table.lookups.len());
        assert!(!table.script_list.0.is_empty(), "a real font declares scripts");
        assert!(!table.feature_list.0.is_empty(), "a real font declares features");
    }

    /// Six more real-font cases, every expectation generated by running
    /// upstream calibre's own `gsub.py` on the same font and inputs.
    /// A single agreeing case could be luck; these cover a ligature
    /// hit, a multi-substitution hit, and several no-op inputs.
    #[test]
    fn matches_real_upstream_python_across_several_glyph_sets() {
        const FONT: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
        let Ok(data) = std::fs::read(FONT) else { return };
        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
        let mut gsub_raw = None;
        for i in 0..num_tables {
            let rec = 12 + 16 * i;
            if &data[rec..rec + 4] == b"GSUB" {
                let off = u32::from_be_bytes(data[rec + 8..rec + 12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(data[rec + 12..rec + 16].try_into().unwrap()) as usize;
                gsub_raw = Some(data[off..off + len].to_vec());
            }
        }
        let table = GsubTable::parse(&gsub_raw.unwrap()).unwrap();

        let cases: &[(&[u16], &[u16])] = &[
            (&[43, 76], &[43, 76, 243]),
            (&[1, 2, 3], &[1, 2, 3]),
            (&[100, 101, 102, 103, 104, 105], &[100, 101, 102, 103, 104, 105]),
            (
                &[36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50],
                &[36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 5995, 6015],
            ),
            (&[2847], &[2847]),
            (&[500, 501, 502, 503], &[500, 501, 502, 503]),
        ];

        for (input, expected) in cases {
            let got = table.all_substitutions(input.iter().copied());
            let want: BTreeSet<u16> = expected.iter().copied().collect();
            assert_eq!(got, want, "diverged from upstream for input {input:?}");
        }
    }
}
