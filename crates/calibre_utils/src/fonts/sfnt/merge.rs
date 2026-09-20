//! Port of `calibre/utils/fonts/sfnt/merge.py` (issue #555):
//! combining several TrueType fonts' glyph and metrics tables into one.
//!
//! # Status: real, tested, and currently without a caller here
//!
//! Upstream has exactly one caller,
//! `ebooks/pdf/html_writer.py`'s `merge_fonts`/`merge_font_files`, and
//! it operates on fonts **extracted from an already-Qt-rendered PDF**
//! (`pdf_doc.list_fonts()`). That whole rendering core is this port's
//! long-standing documented gap -- see
//! `calibre_ebooks::pdf::html_writer`, whose `convert` is a `todo!()`
//! naming the missing browser engine.
//!
//! So this module is deliberately complete and tested but unwired: the
//! work is small, every table API it needs already exists, and leaving
//! the cluster's last file unported would have been the only thing
//! keeping issue #555 open. When an HTML→PDF path does land, the
//! merging half is ready rather than being discovered missing then.
//!
//! This is a different situation from [`super::gsub`], which had a real
//! caller waiting (`subset`'s permanently-empty `extra_glyphs`) and
//! closed a live gap.
//!
//! # Faithfulness notes
//!
//! - **Chosen glyph data wins, first font first.** For a glyph id
//!   present in several fonts, the first font to supply it is kept and
//!   later ones are only *checked*, never preferred -- matching
//!   upstream's `if not prev_glyph_data` branch.
//! - **Mismatches are reported, not fatal.** Upstream has a commented
//!   out `raise` above its size-mismatch log; this returns the same
//!   information as warnings so a caller can surface them, rather than
//!   failing a merge that upstream would have completed.
//! - **`hdmx`/`GPOS`/`GSUB` are dropped** from the result, as upstream
//!   does: they describe glyph relationships that no longer hold once
//!   glyph sets from different fonts are combined.

use std::collections::BTreeMap;

use super::container::Sfnt;
use super::errors::UnsupportedFont;
use super::glyf::GlyfTable;
use super::head::HeadTable;
use super::loca::LocaTable;
use super::maxp::MaxpTable;
use super::metrics::{HorizontalHeader, VerticalHeader};

/// Tables upstream removes from the merged result because they
/// describe relationships that no longer hold across combined fonts.
const DROPPED_TABLES: [&[u8; 4]; 3] = [b"hdmx", b"GPOS", b"GSUB"];

/// What a merge produced: the merged font, plus anything worth telling
/// the user about.
#[derive(Debug)]
pub struct Merged {
    pub sfnt: Sfnt,
    /// Port of upstream's `log(...)` calls: size and metrics
    /// disagreements between fonts for the same glyph id.
    pub warnings: Vec<String>,
}

/// Port of `merge_truetype_fonts_for_pdf`.
///
/// Merges `fonts` into the **first** one, which upstream also treats as
/// the accumulator (`ans = fonts[0]`).
pub fn merge_truetype_fonts_for_pdf(fonts: Vec<Sfnt>) -> Result<Merged, UnsupportedFont> {
    let mut fonts = fonts;
    if fonts.is_empty() {
        return Err(UnsupportedFont("Cannot merge an empty list of fonts".to_string()));
    }

    let mut warnings = Vec::new();
    let mut all_glyphs: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut hmetrics_map: BTreeMap<usize, (u16, i16)> = BTreeMap::new();
    let mut vmetrics_map: BTreeMap<usize, (u16, i16)> = BTreeMap::new();

    for font in &fonts {
        let head = HeadTable::parse(font.get(b"head").ok_or_else(|| UnsupportedFont("A font being merged has no head table".to_string()))?)?;
        let maxp = MaxpTable::parse(font.get(b"maxp").ok_or_else(|| UnsupportedFont("A font being merged has no maxp table".to_string()))?)?;
        let loca_raw = font.get(b"loca").ok_or_else(|| UnsupportedFont("A font being merged has no loca table".to_string()))?;
        let glyf_raw = font.get(b"glyf").ok_or_else(|| UnsupportedFont("A font being merged has no glyf table".to_string()))?;

        let loca = LocaTable::load_offsets(loca_raw, head.index_to_loc_format)?;
        let glyf = GlyfTable::new(glyf_raw.clone());
        let num_glyphs = maxp.num_glyphs as usize;

        // `hhea`/`vhea` are optional; a font lacking one simply
        // contributes no metrics of that kind.
        let hhea = match (font.get(b"hhea"), font.get(b"hmtx")) {
            (Some(hhea_raw), Some(hmtx_raw)) => Some(HorizontalHeader::read_data(hhea_raw, hmtx_raw, num_glyphs)?),
            _ => None,
        };
        let vhea = match (font.get(b"vhea"), font.get(b"vmtx")) {
            (Some(vhea_raw), Some(vmtx_raw)) => Some(VerticalHeader::read_data(vhea_raw, vmtx_raw, num_glyphs)?),
            _ => None,
        };

        // `offset_map` has one entry per glyph plus a terminator,
        // so glyph ids run to len()-1 (upstream: `len(loca.offset_map) - 1`).
        for glyph_id in 0..loca.offset_map.len().saturating_sub(1) {
            let Some((offset, sz)) = loca.glyph_location(glyph_id) else { continue };

            // Upstream's `if not prev_glyph_data` is true for both
            // absent AND empty, so a later font's real outline can
            // replace an earlier empty one.
            let prev_is_empty = all_glyphs.get(&glyph_id).is_none_or(Vec::is_empty);

            if prev_is_empty {
                let data = glyf.raw_glyph_data(offset as usize, sz as usize).unwrap_or_default().to_vec();
                all_glyphs.insert(glyph_id, data);
                if let Some(h) = &hhea {
                    hmetrics_map.insert(glyph_id, h.metrics_for(glyph_id));
                }
                if let Some(v) = &vhea {
                    vmetrics_map.insert(glyph_id, v.metrics_for(glyph_id));
                }
            } else if sz > 0 {
                let prev_len = all_glyphs[&glyph_id].len() as i64;
                if (sz as i64 - prev_len).abs() > 8 {
                    warnings.push(format!("Size mismatch for glyph id: {glyph_id} prev_sz: {prev_len} sz: {sz}"));
                }
                if let Some(h) = &hhea {
                    let m = h.metrics_for(glyph_id);
                    match hmetrics_map.get(&glyph_id) {
                        None => {
                            hmetrics_map.insert(glyph_id, m);
                        }
                        Some(&old) if old != m => warnings.push(format!("Metrics mismatch for glyph id: {glyph_id} prev: {old:?} cur: {m:?}")),
                        Some(_) => {}
                    }
                }
                if let Some(v) = &vhea {
                    let m = v.metrics_for(glyph_id);
                    match vmetrics_map.get(&glyph_id) {
                        None => {
                            vmetrics_map.insert(glyph_id, m);
                        }
                        Some(&old) if old != m => warnings.push(format!("Vertical metrics mismatch for glyph id: {glyph_id} prev: {old:?} cur: {m:?}")),
                        Some(_) => {}
                    }
                }
            }
        }
    }

    // Rebuild the accumulator (the first font) from the merged glyphs.
    let mut ans = fonts.remove(0);
    let head_raw = ans.get(b"head").ok_or_else(|| UnsupportedFont("The target font has no head table".to_string()))?.clone();
    let maxp_raw = ans.get(b"maxp").ok_or_else(|| UnsupportedFont("The target font has no maxp table".to_string()))?.clone();
    let glyf_raw = ans.get(b"glyf").ok_or_else(|| UnsupportedFont("The target font has no glyf table".to_string()))?.clone();

    let mut head = HeadTable::parse(&head_raw)?;
    let mut maxp = MaxpTable::parse(&maxp_raw)?;
    let mut glyf = GlyfTable::new(glyf_raw);

    let sorted: Vec<(usize, Vec<u8>)> = all_glyphs.into_iter().collect();
    let offset_map = glyf.update(&sorted);

    let mut loca = LocaTable::load_offsets(ans.get(b"loca").ok_or_else(|| UnsupportedFont("The target font has no loca table".to_string()))?, head.index_to_loc_format)?;
    loca.update(&offset_map);

    head.index_to_loc_format = if loca.is_long_format { 1 } else { 0 };
    maxp.num_glyphs = u16::try_from(loca.offset_map.len().saturating_sub(1)).map_err(|_| UnsupportedFont("Merged font has more than 65535 glyphs".to_string()))?;

    ans.insert(*b"glyf", glyf.raw.clone());
    ans.insert(*b"loca", loca.to_bytes());
    ans.insert(*b"head", head.to_bytes());
    ans.insert(*b"maxp", maxp.to_bytes());

    if !hmetrics_map.is_empty() {
        if let (Some(hhea_raw), Some(hmtx_raw)) = (ans.get(b"hhea").cloned(), ans.get(b"hmtx").cloned()) {
            let mut hhea = HorizontalHeader::read_data(&hhea_raw, &hmtx_raw, maxp.num_glyphs as usize)?;
            let (new_hhea, new_hmtx) = hhea.update(&hmetrics_map);
            ans.insert(*b"hhea", new_hhea);
            ans.insert(*b"hmtx", new_hmtx);
        }
    }
    if !vmetrics_map.is_empty() {
        if let (Some(vhea_raw), Some(vmtx_raw)) = (ans.get(b"vhea").cloned(), ans.get(b"vmtx").cloned()) {
            let mut vhea = VerticalHeader::read_data(&vhea_raw, &vmtx_raw, maxp.num_glyphs as usize)?;
            let (new_vhea, new_vmtx) = vhea.update(&vmetrics_map);
            ans.insert(*b"vhea", new_vhea);
            ans.insert(*b"vmtx", new_vmtx);
        }
    }

    for tag in DROPPED_TABLES {
        ans.remove(tag);
    }

    Ok(Merged { sfnt: ans, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SANS: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
    const MONO: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";

    fn load(path: &str) -> Option<Sfnt> {
        let raw = std::fs::read(path).ok()?;
        Sfnt::parse(&raw).ok()
    }

    fn glyph_count(sfnt: &Sfnt) -> usize {
        let head = HeadTable::parse(sfnt.get(b"head").unwrap()).unwrap();
        let loca = LocaTable::load_offsets(sfnt.get(b"loca").unwrap(), head.index_to_loc_format).unwrap();
        loca.offset_map.len().saturating_sub(1)
    }

    #[test]
    fn merging_an_empty_list_is_an_error_rather_than_a_panic() {
        assert!(merge_truetype_fonts_for_pdf(Vec::new()).is_err());
    }

    #[test]
    fn merging_a_single_font_round_trips_it_into_a_still_valid_font() {
        let Some(font) = load(SANS) else {
            eprintln!("skipping: {SANS} not present");
            return;
        };
        let before = glyph_count(&font);

        let merged = merge_truetype_fonts_for_pdf(vec![font]).unwrap();

        // The result must still be a parseable font with its glyphs.
        assert_eq!(glyph_count(&merged.sfnt), before, "a one-font merge should preserve the glyph count");
        assert!(merged.sfnt.contains(b"glyf"));
        assert!(merged.sfnt.contains(b"loca"));
        assert!(merged.warnings.is_empty(), "merging a font with itself alone should warn about nothing: {:?}", merged.warnings);
    }

    #[test]
    fn merging_drops_the_tables_that_no_longer_apply_across_fonts() {
        let Some(font) = load(SANS) else { return };
        assert!(font.contains(b"GSUB"), "the fixture font should have a GSUB table to begin with");

        let merged = merge_truetype_fonts_for_pdf(vec![font]).unwrap();

        for tag in DROPPED_TABLES {
            assert!(!merged.sfnt.contains(tag), "{} should have been dropped from the merged font", String::from_utf8_lossy(tag));
        }
    }

    #[test]
    fn merging_two_real_different_fonts_produces_a_valid_font_and_reports_conflicts() {
        let (Some(a), Some(b)) = (load(SANS), load(MONO)) else {
            eprintln!("skipping: DejaVu fixtures not present");
            return;
        };

        let merged = merge_truetype_fonts_for_pdf(vec![a, b]).unwrap();

        assert!(glyph_count(&merged.sfnt) > 0, "the merged font should have glyphs");
        assert!(merged.sfnt.contains(b"head"));
        assert!(merged.sfnt.contains(b"maxp"));

        // Sans and Mono genuinely disagree about metrics for shared
        // glyph ids, so a real merge must surface that rather than
        // silently picking one -- this is upstream's `log(...)` path.
        assert!(
            !merged.warnings.is_empty(),
            "merging two genuinely different fonts should have reported at least one size/metrics conflict"
        );
    }

    #[test]
    fn the_first_font_wins_for_a_glyph_present_in_both() {
        let (Some(a), Some(b)) = (load(SANS), load(MONO)) else { return };

        // Capture what the first font has for a known glyph id.
        let head = HeadTable::parse(a.get(b"head").unwrap()).unwrap();
        let loca = LocaTable::load_offsets(a.get(b"loca").unwrap(), head.index_to_loc_format).unwrap();
        let glyf = GlyfTable::new(a.get(b"glyf").unwrap().clone());
        let (off, sz) = loca.glyph_location(43).expect("glyph 43 should exist in DejaVuSans");
        let expected = glyf.raw_glyph_data(off as usize, sz as usize).unwrap().to_vec();

        let merged = merge_truetype_fonts_for_pdf(vec![a, b]).unwrap();

        let mhead = HeadTable::parse(merged.sfnt.get(b"head").unwrap()).unwrap();
        let mloca = LocaTable::load_offsets(merged.sfnt.get(b"loca").unwrap(), mhead.index_to_loc_format).unwrap();
        let mglyf = GlyfTable::new(merged.sfnt.get(b"glyf").unwrap().clone());
        let (moff, msz) = mloca.glyph_location(43).unwrap();
        let got = mglyf.raw_glyph_data(moff as usize, msz as usize).unwrap();

        assert_eq!(&got[..expected.len().min(got.len())], &expected[..expected.len().min(got.len())], "the first font's outline should have been kept");
    }
}
