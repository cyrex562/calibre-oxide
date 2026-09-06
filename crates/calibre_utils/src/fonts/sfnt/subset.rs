//! Port of `calibre.utils.fonts.sfnt.subset` (issue #553): the real
//! glyph-reachability-closure TrueType subsetting algorithm --
//! `resolve_glyphs`, `subset_truetype`, and the top-level `subset`
//! orchestration (table stripping, cmap/kern restriction,
//! re-serialization). TrueType (`glyf`-flavored) fonts only, per this
//! issue's own scope note -- CFF-flavored OpenType subsetting is
//! issue #554.
//!
//! # A real scope correction found while researching this issue
//!
//! This issue's own filing (and `oeb::polish::subset.rs`'s pre-existing
//! doc comment) describe `calibre.utils.fonts.subset.subset` --
//! **the top-level `utils/fonts/subset.py`** -- as the function real
//! `oeb.polish.subset.subset_all_fonts` calls, and describe it as doing
//! "cmap/glyf/loca table rewriting". Reading the actual current
//! upstream source shows this is stale: **the current top-level
//! `utils/fonts/subset.py` is a 2023 rewrite that delegates entirely to
//! the third-party `fontTools` library's own `Subsetter`** (layout-
//! feature-aware GSUB/GPOS subsetting, CFF charstring subsetting,
//! hinting-program handling, etc.) -- it does no byte-level table
//! editing of its own at all. `subset_all_fonts` really does call this
//! fontTools-wrapping function, not the hand-rolled algorithm this
//! module ports.
//!
//! Reimplementing fontTools' `Subsetter` (a large, mature library) is
//! disproportionate to this cluster and not attempted. What THIS
//! module ports instead is `sfnt/subset.py`'s own `subset()` function
//! -- an older, still-real, still-tested (it has its own
//! upstream `test()`), fully hand-rolled, dependency-free TrueType
//! subsetter that this cluster (#548-552) has now built every
//! prerequisite for. It performs the same real *observable* operation
//! (shrink an embedded font to only the glyphs a book's text actually
//! uses) via a different, self-contained implementation, rather than
//! wrapping fontTools. [`super::super::super::sfnt::subset`]'s own
//! result won't be byte-identical to current real upstream's fontTools
//! output, but it is real, tested, cross-validated subsetting -- not a
//! stub. See `oeb::polish::subset.rs`'s updated doc comment for how
//! this is wired into `subset_all_fonts`.
//!
//! # Disclosed narrowings
//!
//! - **GSUB substitution closure** (real `subset()`'s `extra_glyphs =
//!   gsub.all_substitutions(...)` step, which keeps glyphs reachable
//!   only via an OpenType layout substitution rule) isn't ported: GSUB
//!   parsing is separate, low-priority, not-yet-ported scope (#555).
//!   `extra_glyphs` is always empty here, matching what real upstream
//!   itself falls back to when a font has no `GSUB` table (which is
//!   the common case for most fonts).
//! - **CFF-flavored (`subset_postscript`) fonts** are out of scope for
//!   this issue (#554); [`subset`] returns
//!   [`SubsetError::UnsupportedFont`] for a font with a `CFF ` table
//!   and no `glyf`/`loca` pair, matching real upstream's own
//!   `UnsupportedFont` for a font with neither TrueType nor PostScript
//!   outlines (just for a different reason: not-yet-implemented rather
//!   than genuinely absent).
//! - **`subset()`'s CLI-facing `individual_chars`/`ranges` parameters**
//!   (a comma-separated-string convenience layer for the `subset-font`
//!   command-line tool) aren't ported -- this port's [`subset`] takes
//!   the already-resolved character set directly, matching what
//!   `oeb::polish::subset.rs::subset_all_fonts`'s own `font_stats:
//!   &HashMap<String, HashSet<char>>` already provides.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use indexmap::IndexMap;

use super::cmap::CmapTable;
use super::container::Sfnt;
use super::errors::{NoGlyphs, UnsupportedFont};
use super::glyf::GlyfTable;
use super::head::HeadTable;
use super::kern::KernTable;
use super::loca::LocaTable;
use super::maxp::MaxpTable;

/// Either real error [`subset`]/[`subset_truetype`] can raise, unified
/// since real Python's `subset()` can propagate either kind.
#[derive(Debug)]
pub enum SubsetError {
    UnsupportedFont(UnsupportedFont),
    NoGlyphs(NoGlyphs),
}

impl fmt::Display for SubsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubsetError::UnsupportedFont(e) => write!(f, "{e}"),
            SubsetError::NoGlyphs(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SubsetError {}

impl From<UnsupportedFont> for SubsetError {
    fn from(e: UnsupportedFont) -> Self {
        SubsetError::UnsupportedFont(e)
    }
}

impl From<NoGlyphs> for SubsetError {
    fn from(e: NoGlyphs) -> Self {
        SubsetError::NoGlyphs(e)
    }
}

/// Port of `resolve_glyphs`: the transitive closure of every glyph
/// reachable from `character_map`'s glyph ids or `extra_glyphs`, via
/// composite-glyph component references (always includes glyph 0, the
/// `.notdef` glyph). A glyph id with no real `loca` entry, or whose
/// `glyf` data fails to parse, is simply dropped from the closure --
/// matching real Python's own broad `except (IndexError, ValueError,
/// KeyError, TypeError): continue`.
pub fn resolve_glyphs(loca: &LocaTable, glyf: &GlyfTable, character_map: &BTreeMap<u32, u32>, extra_glyphs: &HashSet<usize>) -> BTreeMap<usize, Vec<u8>> {
    let mut unresolved: HashSet<usize> = character_map.values().map(|&g| g as usize).collect();
    unresolved.extend(extra_glyphs.iter().copied());
    unresolved.insert(0);

    let mut resolved: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    while let Some(&glyph_id) = unresolved.iter().next() {
        unresolved.remove(&glyph_id);
        if resolved.contains_key(&glyph_id) {
            continue;
        }
        let Some((offset, length)) = loca.glyph_location(glyph_id) else { continue };
        let Ok(glyph) = glyf.glyph_data(offset as usize, length as usize) else { continue };
        for &gid in glyph.glyph_indices() {
            let gid = gid as usize;
            if !resolved.contains_key(&gid) {
                unresolved.insert(gid);
            }
        }
        resolved.insert(glyph_id, glyph.to_bytes());
    }
    resolved
}

/// Port of `subset_truetype`: rewrites `head`/`maxp`/`loca`/`glyf` in
/// `sfnt` to contain only the glyphs reachable from `character_map`/
/// `extra_glyphs`, dropping any `character_map` entry whose glyph
/// wasn't resolved.
pub fn subset_truetype(sfnt: &mut Sfnt, character_map: &mut BTreeMap<u32, u32>, extra_glyphs: &HashSet<usize>) -> Result<(), SubsetError> {
    let head_raw = sfnt.get(b"head").cloned();
    let maxp_raw = sfnt.get(b"maxp").cloned();
    let (Some(head_raw), Some(maxp_raw)) = (head_raw, maxp_raw) else {
        return Err(UnsupportedFont("This font does not contain head and/or maxp tables".to_string()).into());
    };
    let mut head = HeadTable::parse(&head_raw)?;
    let mut maxp = MaxpTable::parse(&maxp_raw)?;

    let loca_raw = sfnt.get(b"loca").cloned().ok_or_else(|| UnsupportedFont("This font has no loca table".to_string()))?;
    let loca = LocaTable::load_offsets(&loca_raw, head.index_to_loc_format)?;
    let glyf_raw = sfnt.get(b"glyf").cloned().ok_or_else(|| UnsupportedFont("This font has no glyf table".to_string()))?;
    let mut glyf = GlyfTable::new(glyf_raw);

    let resolved_glyphs = resolve_glyphs(&loca, &glyf, character_map, extra_glyphs);
    if resolved_glyphs.is_empty() || (resolved_glyphs.len() == 1 && resolved_glyphs.contains_key(&0)) {
        return Err(NoGlyphs("This font has no glyphs for the specified character set, subsetting it is pointless".to_string()).into());
    }

    character_map.retain(|_, glyph_id| resolved_glyphs.contains_key(&(*glyph_id as usize)));

    let sorted_glyph_map: Vec<(usize, Vec<u8>)> = resolved_glyphs.into_iter().collect();
    let glyph_offset_map = glyf.update(&sorted_glyph_map);

    let mut loca = loca;
    loca.update(&glyph_offset_map);
    head.index_to_loc_format = if loca.is_long_format { 1 } else { 0 };
    maxp.num_glyphs = (loca.offset_map.len() - 1) as u16;

    sfnt.insert(*b"glyf", glyf.raw);
    sfnt.insert(*b"loca", loca.to_bytes());
    sfnt.insert(*b"head", head.to_bytes());
    sfnt.insert(*b"maxp", maxp.to_bytes());
    Ok(())
}

/// Real sfnt tables `subset` keeps -- everything else is stripped
/// before subsetting, matching real Python's `core_tables` set.
const CORE_TABLES: &[[u8; 4]] = &[
    *b"cmap", *b"hhea", *b"head", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"post", *b"cvt ", *b"fpgm", *b"glyf", *b"loca", *b"prep", *b"CFF ", *b"VORG", *b"EBDT", *b"EBLC", *b"EBSC",
    *b"BASE", *b"GSUB", *b"GPOS", *b"GDEF", *b"JSTF", *b"gasp", *b"hdmx", *b"kern", *b"LTSH", *b"PCLT", *b"VDMX", *b"vhea", *b"vmtx", *b"MATH",
];

/// Port of `subset`: the real, complete subsetting orchestration --
/// strip non-core tables, resolve `chars` to a character map, subset
/// the TrueType outline tables, restrict `cmap`/`kern` to the
/// surviving glyphs, and re-serialize. Returns `(new_bytes, old_sizes,
/// new_sizes, warnings)`. See the module doc for why this takes an
/// already-resolved `chars: &HashSet<char>` rather than real Python's
/// CLI-facing `individual_chars`/`ranges` strings, and why GSUB-derived
/// `extra_glyphs` is always empty.
pub fn subset(raw: &[u8], chars: &HashSet<char>) -> Result<(Vec<u8>, IndexMap<[u8; 4], usize>, IndexMap<[u8; 4], usize>, Vec<String>), SubsetError> {
    let mut char_codes: HashSet<u32> = chars.iter().map(|&c| c as u32).collect();
    char_codes.insert(' ' as u32); // Always keep space, for ease of use.

    let mut sfnt = Sfnt::parse(raw)?;
    let old_sizes = sfnt.sizes();

    sfnt.remove(b"DSIG");
    for tag in sfnt.tags() {
        if !CORE_TABLES.contains(&tag) {
            sfnt.remove(&tag);
        }
    }

    let cmap_raw = sfnt.get(b"cmap").cloned().ok_or_else(|| UnsupportedFont("This font has no cmap table".to_string()))?;
    let mut cmap = CmapTable::parse(cmap_raw)?;
    let codes: Vec<u32> = char_codes.into_iter().collect();
    let mut character_map = cmap.get_character_map(&codes)?;

    // GSUB substitution-derived extra glyphs -- always empty; see the
    // module doc's disclosed narrowing (GSUB parsing is #555, not yet
    // ported).
    let extra_glyphs: HashSet<usize> = HashSet::new();

    if sfnt.contains(b"loca") && sfnt.contains(b"glyf") {
        subset_truetype(&mut sfnt, &mut character_map, &extra_glyphs)?;
    } else if sfnt.contains(b"CFF ") {
        return Err(UnsupportedFont("CFF-flavored (PostScript outline) font subsetting is not yet implemented".to_string()).into());
    } else {
        return Err(UnsupportedFont("This font does not contain TrueType or PostScript outlines".to_string()).into());
    }

    cmap.set_character_map(&character_map);
    sfnt.insert(*b"cmap", cmap.raw);

    let mut warnings = Vec::new();
    if let Some(kern_raw) = sfnt.get(b"kern").cloned() {
        match KernTable::parse(kern_raw) {
            Ok(mut kern) => {
                let surviving: HashSet<u16> = character_map.values().map(|&g| g as u16).collect();
                match kern.restrict_to_glyphs(&surviving) {
                    Ok(()) => sfnt.insert(*b"kern", kern.raw),
                    Err(e) => warnings.push(format!("kern table unsupported, ignoring: {e}")),
                }
            }
            Err(e) => warnings.push(format!("kern table unsupported, ignoring: {e}")),
        }
    }

    let (new_raw, new_sizes) = sfnt.to_bytes()?;
    Ok((new_raw, old_sizes, new_sizes, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::sfnt::max_power_of_two;
    use crate::fonts::utils::checksum_of_block;

    fn head_bytes(index_to_loc_format: i16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(1i32 << 16).to_be_bytes());
        out.extend_from_slice(&0i32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0x5f0f3cf5u32.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&1000u16.to_be_bytes());
        out.extend_from_slice(&0i64.to_be_bytes());
        out.extend_from_slice(&0i64.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&1000i16.to_be_bytes());
        out.extend_from_slice(&1000i16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&9u16.to_be_bytes());
        out.extend_from_slice(&2i16.to_be_bytes());
        out.extend_from_slice(&index_to_loc_format.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out
    }

    fn maxp_bytes(num_glyphs: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(0x0000_5000i32).to_be_bytes());
        out.extend_from_slice(&num_glyphs.to_be_bytes());
        out
    }

    fn simple_glyph(num_of_contours: i16, extra: &[u8]) -> Vec<u8> {
        let mut g = num_of_contours.to_be_bytes().to_vec();
        g.extend_from_slice(&[0u8; 8]); // bounding box
        g.extend_from_slice(extra);
        g
    }

    fn composite_glyph(component_glyph_ids: &[u16]) -> Vec<u8> {
        let mut g = (-1i16).to_be_bytes().to_vec();
        g.extend_from_slice(&[0u8; 8]);
        for (i, &gid) in component_glyph_ids.iter().enumerate() {
            let last = i == component_glyph_ids.len() - 1;
            let flags: u16 = if last { 0 } else { 0x0020 }; // MORE_COMPONENTS
            g.extend_from_slice(&flags.to_be_bytes());
            g.extend_from_slice(&gid.to_be_bytes());
            g.extend_from_slice(&[0u8, 0u8]);
        }
        g
    }

    fn build_cmap_table(mappings: &[(u16, u16)]) -> Vec<u8> {
        // One format-4 segment per mapping (kept simple/deliberately
        // inefficient -- this is a test fixture, not a real encoder),
        // plus the required 0xffff terminator segment.
        let mut end_codes: Vec<u16> = mappings.iter().map(|&(c, _)| c).collect();
        let mut start_codes: Vec<u16> = mappings.iter().map(|&(c, _)| c).collect();
        let mut id_deltas: Vec<i16> = mappings.iter().map(|&(c, g)| (g as i32 - c as i32) as i16).collect();
        end_codes.push(0xffff);
        start_codes.push(0xffff);
        id_deltas.push(1);
        let seg_count = end_codes.len() as u16;

        let mut data = Vec::new();
        for v in &end_codes {
            data.extend_from_slice(&v.to_be_bytes());
        }
        data.extend_from_slice(&0u16.to_be_bytes());
        for v in &start_codes {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in &id_deltas {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for _ in 0..seg_count {
            data.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset: all zero (use idDelta)
        }

        let length = 14 + data.len();
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes());
        sub.extend_from_slice(&(length as u16).to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&(2 * seg_count).to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&data);

        let mut raw = Vec::new();
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&1u16.to_be_bytes());
        raw.extend_from_slice(&3u16.to_be_bytes());
        raw.extend_from_slice(&1u16.to_be_bytes());
        raw.extend_from_slice(&12u32.to_be_bytes());
        raw.extend_from_slice(&sub);
        raw
    }

    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let num_tables = tables.len() as u32;
        let ln2 = max_power_of_two(num_tables);
        let srange = (1u32 << ln2) * 16;

        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        out.extend_from_slice(&(num_tables as u16).to_be_bytes());
        out.extend_from_slice(&(srange as u16).to_be_bytes());
        out.extend_from_slice(&(ln2 as u16).to_be_bytes());
        out.extend_from_slice(&((num_tables * 16).wrapping_sub(srange) as u16).to_be_bytes());

        let header_len = 12 + tables.len() * 16;
        let mut data_section = Vec::new();
        let mut records = Vec::new();
        let mut offset = header_len;
        for (tag, data) in tables {
            let checksum = checksum_of_block(data);
            records.push((**tag, checksum, offset, data.len()));
            data_section.extend_from_slice(data);
            while data_section.len() % 4 != 0 {
                data_section.push(0);
            }
            offset = header_len + data_section.len();
        }
        for (tag, checksum, table_offset, table_length) in records {
            out.extend_from_slice(&tag);
            out.extend_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(&(table_offset as u32).to_be_bytes());
            out.extend_from_slice(&(table_length as u32).to_be_bytes());
        }
        out.extend_from_slice(&data_section);
        out
    }

    /// Builds a real, minimal, 4-glyph TrueType font: glyph 0 (.notdef,
    /// simple), glyph 1 ('A', simple), glyph 2 ('B', a composite
    /// referencing glyph 1), glyph 3 ('Z', simple, unused by the
    /// requested character set). `loca`/`glyf`/`cmap`/`head`/`maxp`
    /// are all real and mutually consistent.
    fn build_four_glyph_font() -> Vec<u8> {
        let glyphs = [
            simple_glyph(1, &[0xAAu8]),          // 0: .notdef
            simple_glyph(1, &[0xBBu8, 0xBBu8]),  // 1: 'A'
            composite_glyph(&[1]),               // 2: 'B', references glyph 1
            simple_glyph(1, &[0xCCu8]),           // 3: 'Z'
        ];
        let mut glyf = Vec::new();
        let mut offsets = vec![0u32];
        for g in &glyphs {
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
            offsets.push(glyf.len() as u32);
        }
        // Short (format 0) loca: values are offset/2 as u16.
        let loca_short: Vec<u8> = offsets.iter().flat_map(|&o| ((o / 2) as u16).to_be_bytes()).collect();

        let head = head_bytes(0); // index_to_loc_format = 0 (short)
        let maxp = maxp_bytes(glyphs.len() as u16);
        let cmap = build_cmap_table(&[(0x41, 1), (0x42, 2), (0x5a, 3)]); // A->1, B->2, Z->3

        build_sfnt(&[(b"head", &head), (b"maxp", &maxp), (b"loca", &loca_short), (b"glyf", &glyf), (b"cmap", &cmap)])
    }

    #[test]
    fn resolve_glyphs_includes_composite_glyph_dependencies_and_notdef() {
        let font = build_four_glyph_font();
        let sfnt = Sfnt::parse(&font).unwrap();
        let head = HeadTable::parse(sfnt.get(b"head").unwrap()).unwrap();
        let loca = LocaTable::load_offsets(sfnt.get(b"loca").unwrap(), head.index_to_loc_format).unwrap();
        let glyf = GlyfTable::new(sfnt.get(b"glyf").unwrap().clone());

        // Only request 'B' (glyph 2), which references glyph 1 as a component.
        let mut character_map = BTreeMap::new();
        character_map.insert(0x42u32, 2u32);
        let resolved = resolve_glyphs(&loca, &glyf, &character_map, &HashSet::new());

        assert!(resolved.contains_key(&0), "glyph 0 (.notdef) should always be kept");
        assert!(resolved.contains_key(&1), "glyph 1 should be kept transitively, since glyph 2 references it as a component");
        assert!(resolved.contains_key(&2));
        assert!(!resolved.contains_key(&3), "glyph 3 was never requested and isn't referenced by anything that was");
    }

    #[test]
    fn subset_drops_unrequested_glyphs_and_shrinks_the_font() {
        let font = build_four_glyph_font();
        let mut chars = HashSet::new();
        chars.insert('B'); // 0x42 -> glyph 2, which transitively needs glyph 1

        let (new_raw, old_sizes, new_sizes, warnings) = subset(&font, &chars).unwrap();
        assert!(warnings.is_empty());
        assert!(new_raw.len() < font.len(), "subsetting to one character should shrink the font");
        assert!(new_sizes[&*b"glyf"] < old_sizes[&*b"glyf"], "the glyf table should shrink once glyph 3 is dropped");

        // Re-parse the subset output and confirm 'B' (and its
        // component dependency) still round-trip through a real cmap
        // lookup -- the strongest possible cross-validation without a
        // vendored external font file (see the module doc).
        let resfnt = Sfnt::parse(&new_raw).unwrap();
        let cmap = CmapTable::parse(resfnt.get(b"cmap").unwrap().clone()).unwrap();
        let map = cmap.get_character_map(&[0x42, 0x41, 0x5a]).unwrap();
        assert!(map.contains_key(&0x42), "'B' was requested and must still resolve");
        assert!(!map.contains_key(&0x5a), "'Z' was never requested and its glyph should have been dropped");

        let head = HeadTable::parse(resfnt.get(b"head").unwrap()).unwrap();
        let maxp = MaxpTable::parse(resfnt.get(b"maxp").unwrap()).unwrap();
        let loca = LocaTable::load_offsets(resfnt.get(b"loca").unwrap(), head.index_to_loc_format).unwrap();
        // Real upstream's LocaTable::update deliberately preserves the
        // ORIGINAL glyph-id space size (dropped glyphs become
        // zero-length rather than being renumbered out of existence),
        // per its own doc comment -- so the original 4-glyph space (5
        // loca entries) survives even though only 3 glyphs have data.
        assert_eq!(loca.offset_map.len(), 5);
        assert_eq!(maxp.num_glyphs, 4);
        assert_eq!(loca.glyph_location(3), Some((loca.offset_map[3], 0)), "glyph 3 ('Z') should be hollowed out to zero length, not removed from the id space");
    }

    #[test]
    fn subset_rejects_a_font_with_no_cmap_table() {
        let head = head_bytes(0);
        let font = build_sfnt(&[(b"head", &head)]);
        let mut chars = HashSet::new();
        chars.insert('A');
        let err = subset(&font, &chars).unwrap_err();
        assert!(matches!(err, SubsetError::UnsupportedFont(_)));
        assert!(err.to_string().contains("cmap"), "{err}");
    }

    #[test]
    fn subset_errors_with_no_glyphs_when_the_character_set_matches_nothing() {
        // NoGlyphs fires only when even the fallback (.notdef-only)
        // closure is empty -- which can't happen for a font whose
        // glyph 0 actually resolves (glyph 0 is always requested).
        // Exercise the real NoGlyphs path directly instead, with a
        // font whose `loca`/`glyf` can't resolve even glyph 0.
        let head = head_bytes(0);
        let maxp = maxp_bytes(0);
        let loca = [0u16, 0u16].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>();
        let glyf: Vec<u8> = Vec::new();
        let cmap = build_cmap_table(&[]);
        let font = build_sfnt(&[(b"head", &head), (b"maxp", &maxp), (b"loca", &loca), (b"glyf", &glyf), (b"cmap", &cmap)]);
        let mut chars = HashSet::new();
        chars.insert('Q');
        let err = subset(&font, &chars).unwrap_err();
        assert!(matches!(err, SubsetError::NoGlyphs(_)), "{err}");
    }
}
