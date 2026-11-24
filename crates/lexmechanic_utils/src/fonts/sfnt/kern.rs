//! Port of `calibre.utils.fonts.sfnt.kern` (`KernTable`, issue #552).
//!
//! # Disclosed narrowings
//!
//! - Real `KernTable.version` is a `FixedProperty` (16.16 fixed-point)
//!   descriptor, but every real comparison in this module (version
//!   dispatch, format-0 detection) operates on the RAW integer
//!   (`self._version`), never the fixed-point-converted value. This
//!   port keeps [`KernTable::version`] as that same raw integer (`0`
//!   for a classic Microsoft-format table, `0x10000` for an Apple-
//!   format table) and doesn't add a separate fixed-point accessor,
//!   since nothing in the real module ever reads the converted value.

use std::collections::HashSet;

use super::errors::UnsupportedFont;
use super::max_power_of_two;

/// Port of `KernTable`.
pub struct KernTable {
    pub version: u32,
    pub num_tables: u32,
    pub raw: Vec<u8>,
}

impl KernTable {
    /// Port of `KernTable.__init__`'s real classic-vs-Apple-format
    /// version detection: read as two `u16`s first; if the first looks
    /// like the literal value `1` (which is what the high 16 bits of an
    /// Apple-format `Fixed` version `0x00010000` decode to when
    /// misread as a plain `u16`), re-read as two `u32`s instead.
    pub fn parse(raw: Vec<u8>) -> Result<Self, UnsupportedFont> {
        let head = raw.get(0..4).ok_or_else(|| UnsupportedFont("truncated kern table".to_string()))?;
        let mut version = u16::from_be_bytes([head[0], head[1]]) as u32;
        let mut num_tables = u16::from_be_bytes([head[2], head[3]]) as u32;
        if version == 1 && raw.len() >= 8 {
            let head32 = &raw[0..8];
            version = u32::from_be_bytes(head32[0..4].try_into().unwrap());
            num_tables = u32::from_be_bytes(head32[4..8].try_into().unwrap());
        }
        Ok(KernTable { version, num_tables, raw })
    }

    /// Port of `KernTable.restrict_to_glyphs`: keeps only format-0
    /// kerning pairs where both glyphs survive subsetting, dropping
    /// (or fully removing) any subtable left with zero surviving
    /// pairs. Non-format-0 subtables are kept as-is (real upstream
    /// only knows how to restrict format 0).
    pub fn restrict_to_glyphs(&mut self, glyph_ids: &HashSet<u16>) -> Result<(), UnsupportedFont> {
        if self.version != 0 && self.version != 0x10000 {
            return Err(UnsupportedFont(format!("kern table has version: {:x}", self.version)));
        }
        let mut offset: usize = if self.version == 0 { 4 } else { 8 };
        let mut tables: Vec<Vec<u8>> = Vec::new();

        for _ in 0..self.num_tables {
            let bytes = self.raw.get(offset..offset + 6).ok_or_else(|| UnsupportedFont("truncated kern subtable header".to_string()))?;
            let (length, table_format) = if self.version == 0 {
                let version_field = u16::from_be_bytes(bytes[0..2].try_into().unwrap());
                let length = u16::from_be_bytes(bytes[2..4].try_into().unwrap()) as usize;
                (length, version_field)
            } else {
                let length = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
                let coverage = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
                (length, coverage & 0xff)
            };
            let end = (offset + length).min(self.raw.len());
            let sub_raw = self.raw[offset..end].to_vec();

            let keep = if table_format == 0 {
                let restricted = restrict_format_0(&sub_raw, self.version, glyph_ids)?;
                if restricted.is_empty() { None } else { Some(restricted) }
            } else {
                Some(sub_raw)
            };
            if let Some(t) = keep {
                tables.push(t);
            }
            offset += length;
        }

        let mut new_raw = Vec::new();
        if self.version == 0 {
            new_raw.extend_from_slice(&(self.version as u16).to_be_bytes());
            new_raw.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        } else {
            new_raw.extend_from_slice(&self.version.to_be_bytes());
            new_raw.extend_from_slice(&(tables.len() as u32).to_be_bytes());
        }
        for t in &tables {
            new_raw.extend_from_slice(t);
        }
        self.raw = new_raw;
        Ok(())
    }
}

/// Port of `KernTable.restrict_format_0`.
fn restrict_format_0(raw: &[u8], version: u32, glyph_ids: &HashSet<u16>) -> Result<Vec<u8>, UnsupportedFont> {
    enum Header {
        Classic { version_field: u16, coverage: u16 },
        Apple { coverage: u16, tuple_index: u16 },
    }

    let (header, npairs_declared, header_len) = if version == 0 {
        let b = raw.get(0..8).ok_or_else(|| UnsupportedFont("truncated format 0 kern subtable header".to_string()))?;
        let version_field = u16::from_be_bytes(b[0..2].try_into().unwrap());
        let coverage = u16::from_be_bytes(b[4..6].try_into().unwrap());
        let npairs = u16::from_be_bytes(b[6..8].try_into().unwrap());
        (Header::Classic { version_field, coverage }, npairs, 14usize)
    } else {
        let b = raw.get(0..10).ok_or_else(|| UnsupportedFont("truncated format 0 kern subtable header".to_string()))?;
        let coverage = u16::from_be_bytes(b[4..6].try_into().unwrap());
        let tuple_index = u16::from_be_bytes(b[6..8].try_into().unwrap());
        let npairs = u16::from_be_bytes(b[8..10].try_into().unwrap());
        (Header::Apple { coverage, tuple_index }, npairs, 16usize)
    };

    let mut offset = header_len;
    let mut entries: Vec<[u8; 6]> = Vec::new();
    for _ in 0..npairs_declared {
        let Some(rec) = raw.get(offset..offset + 6) else {
            offset = raw.len();
            break; // Buggy kern table -- matches real Python's struct.error catch.
        };
        let left = u16::from_be_bytes(rec[0..2].try_into().unwrap());
        let right = u16::from_be_bytes(rec[2..4].try_into().unwrap());
        let value = i16::from_be_bytes(rec[4..6].try_into().unwrap());
        if glyph_ids.contains(&left) && glyph_ids.contains(&right) {
            let mut e = [0u8; 6];
            e[0..2].copy_from_slice(&left.to_be_bytes());
            e[2..4].copy_from_slice(&right.to_be_bytes());
            e[4..6].copy_from_slice(&value.to_be_bytes());
            entries.push(e);
        }
        offset += 6;
    }

    if offset != raw.len() {
        return Err(UnsupportedFont("This font has extra data at the end of a Format 0 kern subtable".to_string()));
    }

    let npairs = entries.len();
    if npairs == 0 {
        return Ok(Vec::new());
    }

    let entry_selector = max_power_of_two(npairs as u32);
    let search_range = (1u32 << entry_selector) * 6;
    let range_shift = (npairs as u32 - (1u32 << entry_selector)) * 6;
    let entries_bytes: Vec<u8> = entries.into_iter().flatten().collect();
    let length = header_len + entries_bytes.len();

    let mut out = Vec::with_capacity(length + 8);
    match header {
        Header::Classic { version_field, coverage } => {
            out.extend_from_slice(&version_field.to_be_bytes());
            out.extend_from_slice(&(length as u16).to_be_bytes());
            out.extend_from_slice(&coverage.to_be_bytes());
        }
        Header::Apple { coverage, tuple_index } => {
            out.extend_from_slice(&(length as u32).to_be_bytes());
            out.extend_from_slice(&coverage.to_be_bytes());
            out.extend_from_slice(&tuple_index.to_be_bytes());
        }
    }
    out.extend_from_slice(&(npairs as u16).to_be_bytes());
    out.extend_from_slice(&(search_range as u16).to_be_bytes());
    out.extend_from_slice(&(entry_selector as u16).to_be_bytes());
    out.extend_from_slice(&(range_shift as u16).to_be_bytes());
    out.extend_from_slice(&entries_bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format0_subtable_v0(pairs: &[(u16, u16, i16)]) -> Vec<u8> {
        let mut data = Vec::new();
        for &(l, r, v) in pairs {
            data.extend_from_slice(&l.to_be_bytes());
            data.extend_from_slice(&r.to_be_bytes());
            data.extend_from_slice(&v.to_be_bytes());
        }
        let length = 14 + data.len();
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // subtable version
        out.extend_from_slice(&(length as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // coverage (format 0 in low byte)
        out.extend_from_slice(&(pairs.len() as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // searchRange (unused by the reader)
        out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        out.extend_from_slice(&data);
        out
    }

    fn kern_table_v0(subtables: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&(subtables.len() as u16).to_be_bytes());
        for s in subtables {
            out.extend_from_slice(s);
        }
        out
    }

    #[test]
    fn parses_a_classic_microsoft_format_kern_table() {
        let sub = format0_subtable_v0(&[(1, 2, 10)]);
        let raw = kern_table_v0(&[sub]);
        let table = KernTable::parse(raw).unwrap();
        assert_eq!(table.version, 0);
        assert_eq!(table.num_tables, 1);
    }

    #[test]
    fn parses_an_apple_format_kern_table_via_the_version_1_heuristic() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&0x00010000u32.to_be_bytes()); // Fixed version 1.0
        raw.extend_from_slice(&1u32.to_be_bytes()); // num_tables
        let table = KernTable::parse(raw).unwrap();
        assert_eq!(table.version, 0x10000);
        assert_eq!(table.num_tables, 1);
    }

    #[test]
    fn restrict_to_glyphs_drops_pairs_referencing_a_removed_glyph() {
        let sub = format0_subtable_v0(&[(1, 2, 10), (3, 4, 20)]);
        let raw = kern_table_v0(&[sub]);
        let mut table = KernTable::parse(raw).unwrap();
        let surviving: HashSet<u16> = [1u16, 2].into_iter().collect();
        table.restrict_to_glyphs(&surviving).unwrap();

        // Re-parse the rebuilt raw bytes to verify only the surviving pair remains.
        let rebuilt = KernTable::parse(table.raw.clone()).unwrap();
        assert_eq!(rebuilt.num_tables, 1);
        let sub_npairs = u16::from_be_bytes(rebuilt.raw[4 + 6..4 + 8].try_into().unwrap());
        assert_eq!(sub_npairs, 1, "only the (1,2) pair should survive");
    }

    #[test]
    fn restrict_to_glyphs_drops_a_subtable_left_with_zero_pairs() {
        let sub = format0_subtable_v0(&[(1, 2, 10)]);
        let raw = kern_table_v0(&[sub]);
        let mut table = KernTable::parse(raw).unwrap();
        let surviving: HashSet<u16> = [9u16, 10].into_iter().collect(); // neither glyph survives
        table.restrict_to_glyphs(&surviving).unwrap();
        let rebuilt = KernTable::parse(table.raw.clone()).unwrap();
        assert_eq!(rebuilt.num_tables, 0);
    }

    #[test]
    fn rejects_an_unsupported_kern_version() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&2u16.to_be_bytes()); // version 2, not 0 or 0x10000
        raw.extend_from_slice(&0u16.to_be_bytes());
        let mut table = KernTable::parse(raw).unwrap();
        let err = table.restrict_to_glyphs(&HashSet::new()).unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
    }
}
