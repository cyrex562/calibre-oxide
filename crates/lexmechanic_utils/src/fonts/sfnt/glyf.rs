//! Port of `calibre.utils.fonts.sfnt.glyf` (`SimpleGlyph`/
//! `CompositeGlyph`/`GlyfTable`, issue #550).

use std::collections::BTreeMap;

use super::errors::UnsupportedFont;

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

/// Port of `SimpleGlyph`/`CompositeGlyph`, unified into one enum since
/// both share the same real byte-payload/length interface and differ
/// only in whether they carry component `glyph_indices` (real Python's
/// own `CompositeGlyph(SimpleGlyph)` subclass relationship, expressed
/// here as two variants of one type rather than an inheritance chain).
#[derive(Debug, Clone)]
pub enum Glyph {
    Simple { num_of_contours: i16, raw: Vec<u8> },
    /// `num_of_contours` is always negative for a composite glyph (the
    /// real, if slightly odd, sfnt convention this distinguishes
    /// simple/composite glyphs by). `glyph_indices` are the other
    /// glyphs this one references as components -- real glyph-level
    /// subsetting must keep all of them too, transitively.
    Composite { num_of_contours: i16, raw: Vec<u8>, glyph_indices: Vec<u16> },
}

impl Glyph {
    pub fn raw(&self) -> &[u8] {
        match self {
            Glyph::Simple { raw, .. } => raw,
            Glyph::Composite { raw, .. } => raw,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw().to_vec()
    }

    pub fn len(&self) -> usize {
        self.raw().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_composite(&self) -> bool {
        matches!(self, Glyph::Composite { .. })
    }

    pub fn glyph_indices(&self) -> &[u16] {
        match self {
            Glyph::Simple { .. } => &[],
            Glyph::Composite { glyph_indices, .. } => glyph_indices,
        }
    }
}

/// Port of `CompositeGlyph.__init__`'s real component-record walk.
fn parse_composite_glyph_indices(raw: &[u8]) -> Result<Vec<u16>, UnsupportedFont> {
    let mut indices = Vec::new();
    let mut flags = MORE_COMPONENTS;
    let mut offset = 10usize;
    while flags & MORE_COMPONENTS != 0 {
        let bytes = raw.get(offset..offset + 4).ok_or_else(|| UnsupportedFont("truncated composite glyph component record".to_string()))?;
        flags = u16::from_be_bytes([bytes[0], bytes[1]]);
        let glyph_index = u16::from_be_bytes([bytes[2], bytes[3]]);
        indices.push(glyph_index);
        offset += 4;
        if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            offset += 4;
        } else {
            offset += 2;
        }
        if flags & WE_HAVE_A_SCALE != 0 {
            offset += 2;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            offset += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            offset += 8;
        }
    }
    Ok(indices)
}

/// Port of `GlyfTable`.
#[derive(Debug, Clone)]
pub struct GlyfTable {
    pub raw: Vec<u8>,
}

impl GlyfTable {
    pub fn new(raw: Vec<u8>) -> Self {
        GlyfTable { raw }
    }

    /// Port of `GlyfTable.glyph_data(as_raw=False)`: parses the glyph
    /// at `[offset, offset+length)` into a real [`Glyph`].
    pub fn glyph_data(&self, offset: usize, length: usize) -> Result<Glyph, UnsupportedFont> {
        let raw = self.raw.get(offset..offset + length).ok_or_else(|| UnsupportedFont("glyph data offset/length out of range".to_string()))?.to_vec();
        if raw.is_empty() {
            return Ok(Glyph::Simple { num_of_contours: 0, raw });
        }
        let num_of_contours = i16::from_be_bytes(raw[0..2].try_into().unwrap());
        if num_of_contours >= 0 {
            Ok(Glyph::Simple { num_of_contours, raw })
        } else {
            let glyph_indices = parse_composite_glyph_indices(&raw)?;
            Ok(Glyph::Composite { num_of_contours, raw, glyph_indices })
        }
    }

    /// Port of `GlyfTable.glyph_data(as_raw=True)`: the glyph's raw
    /// bytes without parsing them into a [`Glyph`].
    pub fn raw_glyph_data(&self, offset: usize, length: usize) -> Option<&[u8]> {
        self.raw.get(offset..offset + length)
    }

    /// Port of `GlyfTable.update`: rewrites this table to contain only
    /// the glyphs in `sorted_glyph_map` (already in the caller's
    /// desired final order), 4-byte-padding each one. Returns each
    /// glyph id's new `(offset, length)` in the rebuilt table -- the
    /// same shape [`super::loca::LocaTable::update`] needs.
    pub fn update(&mut self, sorted_glyph_map: &[(usize, Vec<u8>)]) -> BTreeMap<usize, (u32, u32)> {
        let mut ans = BTreeMap::new();
        let mut offset: u32 = 0;
        let mut block = Vec::new();
        for (glyph_id, raw) in sorted_glyph_map {
            let mut raw = raw.clone();
            let pad = (4 - (raw.len() % 4)) % 4;
            raw.extend(std::iter::repeat_n(0u8, pad));
            ans.insert(*glyph_id, (offset, raw.len() as u32));
            offset += raw.len() as u32;
            block.push(raw);
        }
        self.raw = block.concat();
        ans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_glyph_bytes(num_of_contours: i16) -> Vec<u8> {
        let mut raw = num_of_contours.to_be_bytes().to_vec();
        raw.extend_from_slice(&[0u8; 8]); // bounding box, contents don't matter for these tests
        raw
    }

    fn composite_glyph_bytes(component_glyph_ids: &[u16]) -> Vec<u8> {
        let mut raw = (-1i16).to_be_bytes().to_vec();
        raw.extend_from_slice(&[0u8; 8]); // bounding box
        for (i, &gid) in component_glyph_ids.iter().enumerate() {
            let last = i == component_glyph_ids.len() - 1;
            let mut flags: u16 = 0; // ARGS are bytes (not words), no scale
            if !last {
                flags |= MORE_COMPONENTS;
            }
            raw.extend_from_slice(&flags.to_be_bytes());
            raw.extend_from_slice(&gid.to_be_bytes());
            raw.extend_from_slice(&[0u8, 0u8]); // 2 byte-sized args
        }
        raw
    }

    #[test]
    fn a_simple_glyph_is_recognized_by_its_non_negative_contour_count() {
        let data = simple_glyph_bytes(3);
        let table = GlyfTable::new(data.clone());
        let glyph = table.glyph_data(0, data.len()).unwrap();
        assert!(!glyph.is_composite());
        assert!(glyph.glyph_indices().is_empty());
        assert_eq!(glyph.to_bytes(), data);
    }

    #[test]
    fn a_composite_glyph_reports_every_referenced_component() {
        let data = composite_glyph_bytes(&[5, 9, 12]);
        let table = GlyfTable::new(data.clone());
        let glyph = table.glyph_data(0, data.len()).unwrap();
        assert!(glyph.is_composite());
        assert_eq!(glyph.glyph_indices(), &[5, 9, 12]);
    }

    #[test]
    fn an_empty_glyph_slot_is_a_real_zero_length_simple_glyph() {
        let table = GlyfTable::new(Vec::new());
        let glyph = table.glyph_data(0, 0).unwrap();
        assert!(!glyph.is_composite());
        assert_eq!(glyph.len(), 0);
    }

    #[test]
    fn update_rebuilds_the_table_with_4_byte_padding_between_glyphs() {
        let mut table = GlyfTable::new(Vec::new());
        let map = vec![(0usize, vec![1u8, 2, 3]), (2usize, vec![9u8, 9, 9, 9, 9])];
        let offsets = table.update(&map);
        assert_eq!(offsets[&0], (0, 4), "3 bytes should be padded up to 4");
        assert_eq!(offsets[&2], (4, 8), "5 bytes should be padded up to 8");
        assert_eq!(table.raw.len(), 12);
        assert_eq!(&table.raw[0..3], &[1, 2, 3]);
        assert_eq!(table.raw[3], 0, "padding byte should be zero");
    }
}
