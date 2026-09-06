//! Port of `calibre.utils.fonts.sfnt.loca` (`LocaTable`, issue #550).
//!
//! # Disclosed narrowings
//!
//! - **`read_array`/`four_byte_type_code`**: real Python's own
//!   byteswap dance (`array.array(fmt, data)` interprets `data` in
//!   *native* byte order, then `byteswap()`s on a little-endian host to
//!   correct it) exists only to cope with Python's `array` module
//!   having no direct "read big-endian" mode. The platform-independent
//!   *result* is simply "read a sequence of big-endian `u16`/`u32`
//!   values", which is what this port does directly via
//!   `u16::from_be_bytes`/`u32::from_be_bytes` -- no byte-order dance
//!   needed, and no platform-dependent "which array type code is 4
//!   bytes" question either (`u32` always is).
//! - **`load_offsets`**'s real Python signature takes `maxp_table` as a
//!   parameter but never reads it in the body (only
//!   `head_table.index_to_loc_format` is used) -- ported without the
//!   unused parameter rather than threading through a value nothing
//!   reads.
//! - **`dump_glyphs`** is a `print`-based debugging helper, not real
//!   library API -- not ported, matching this project's convention for
//!   debug/print-only helpers.

use std::collections::BTreeMap;

use super::errors::UnsupportedFont;

fn read_u16_array(raw: &[u8]) -> Result<Vec<u16>, UnsupportedFont> {
    if raw.len() % 2 != 0 {
        return Err(UnsupportedFont("loca table has an odd number of bytes for its 16-bit format".to_string()));
    }
    Ok(raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect())
}

fn read_u32_array(raw: &[u8]) -> Result<Vec<u32>, UnsupportedFont> {
    if raw.len() % 4 != 0 {
        return Err(UnsupportedFont("loca table has an incomplete final entry for its 32-bit format".to_string()));
    }
    Ok(raw.chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// Port of `LocaTable`. `offset_map[i]` is glyph `i`'s byte offset into
/// the `glyf` table; `offset_map.len() - 1` entries describe
/// `offset_map.len() - 2` real glyphs plus one trailing sentinel offset
/// (the table always has one more entry than there are glyphs, so a
/// glyph's length can be computed as `offset_map[i+1] - offset_map[i]`
/// without special-casing the last glyph).
#[derive(Debug, Clone, Default)]
pub struct LocaTable {
    pub offset_map: Vec<u32>,
    /// `true` when offsets are stored as 4-byte values (real `loca`
    /// format 1, `head_table.index_to_loc_format != 0`); `false` for
    /// the 2-byte format-0 encoding (where the stored value is half the
    /// real byte offset).
    pub is_long_format: bool,
}

impl LocaTable {
    /// Port of `LocaTable.load_offsets`.
    pub fn load_offsets(raw: &[u8], index_to_loc_format: i16) -> Result<Self, UnsupportedFont> {
        let is_long_format = index_to_loc_format != 0;
        let offset_map = if is_long_format {
            read_u32_array(raw)?
        } else {
            read_u16_array(raw)?.into_iter().map(|v| v as u32 * 2).collect()
        };
        Ok(LocaTable { offset_map, is_long_format })
    }

    /// Port of `LocaTable.glyph_location`: `(offset, length)` in the
    /// `glyf` table for `glyph_id`.
    pub fn glyph_location(&self, glyph_id: usize) -> Option<(u32, u32)> {
        let offset = *self.offset_map.get(glyph_id)?;
        let next_offset = *self.offset_map.get(glyph_id + 1)?;
        Some((offset, next_offset - offset))
    }

    /// Port of `LocaTable.update` (real Python's own `subset = update`
    /// alias isn't a separate method here -- callers just call
    /// `update` directly). `resolved_glyph_map` maps a surviving
    /// glyph's id to its `(offset, size)` in the *rebuilt* `glyf`
    /// table. Recomputes [`Self::is_long_format`] -- callers doing real
    /// subsetting must also update the sibling `head` table's
    /// `index_to_loc_format` field to match (real Python leaves that to
    /// the caller too; `LocaTable` doesn't hold a reference to `head`).
    pub fn update(&mut self, resolved_glyph_map: &BTreeMap<usize, (u32, u32)>) {
        let current_max_glyph_id = self.offset_map.len().saturating_sub(2);
        let max_glyph_id = resolved_glyph_map.keys().copied().max().unwrap_or(0).max(current_max_glyph_id);
        self.offset_map = vec![0u32; max_glyph_id + 2];

        let mut glyphs: Vec<(usize, u32, u32)> = resolved_glyph_map.iter().map(|(&gid, &(off, sz))| (gid, off, sz)).collect();
        glyphs.sort_by_key(|&(_, off, _)| off);
        for (glyph_id, offset, sz) in glyphs {
            self.offset_map[glyph_id] = offset;
            self.offset_map[glyph_id + 1] = offset + sz;
        }
        // A zero entry at position i (other than a genuine zero-offset
        // glyph 0) means glyph i-1 has no data -- carry the previous
        // offset forward so it reads as a real zero-length glyph.
        for i in 1..self.offset_map.len() {
            if self.offset_map[i] == 0 {
                self.offset_map[i] = self.offset_map[i - 1];
            }
        }

        let max_offset = self.offset_map.iter().copied().max().unwrap_or(0);
        self.is_long_format = !(max_offset < 0x20000 && self.offset_map.iter().all(|&l| l % 2 == 0));
    }

    /// Port of the raw-bytes half of `LocaTable.update`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.is_long_format {
            for &v in &self.offset_map {
                out.extend_from_slice(&v.to_be_bytes());
            }
        } else {
            for &v in &self.offset_map {
                out.extend_from_slice(&((v / 2) as u16).to_be_bytes());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_format_offsets_are_doubled() {
        let raw: Vec<u8> = [0u16, 5, 10, 10, 20].iter().flat_map(|v| v.to_be_bytes()).collect();
        let loca = LocaTable::load_offsets(&raw, 0).unwrap();
        assert_eq!(loca.offset_map, vec![0, 10, 20, 20, 40]);
        assert!(!loca.is_long_format);
    }

    #[test]
    fn long_format_offsets_are_read_directly() {
        let raw: Vec<u8> = [0u32, 100, 250].iter().flat_map(|v| v.to_be_bytes()).collect();
        let loca = LocaTable::load_offsets(&raw, 1).unwrap();
        assert_eq!(loca.offset_map, vec![0, 100, 250]);
        assert!(loca.is_long_format);
    }

    #[test]
    fn glyph_location_computes_offset_and_length() {
        let raw: Vec<u8> = [0u16, 5, 10, 10].iter().flat_map(|v| v.to_be_bytes()).collect();
        let loca = LocaTable::load_offsets(&raw, 0).unwrap();
        assert_eq!(loca.glyph_location(0), Some((0, 10)));
        assert_eq!(loca.glyph_location(1), Some((10, 10)));
        assert_eq!(loca.glyph_location(2), Some((20, 0)), "an unused trailing glyph should read as zero-length");
    }

    #[test]
    fn update_rebuilds_a_compact_offset_table_for_surviving_glyphs() {
        let mut loca = LocaTable::default();
        let mut resolved = BTreeMap::new();
        resolved.insert(0usize, (0u32, 10u32));
        resolved.insert(2usize, (10u32, 20u32));
        loca.update(&resolved);
        // glyph 1 has no data -- its slot carries glyph 0's end offset
        // forward, matching real upstream's own "no data" convention.
        assert_eq!(loca.offset_map, vec![0, 10, 10, 30]);
        assert!(!loca.is_long_format, "all offsets are small and even, so the compact 2-byte format should be chosen");
    }

    #[test]
    fn update_picks_the_long_format_when_offsets_are_too_large() {
        let mut loca = LocaTable::default();
        let mut resolved = BTreeMap::new();
        resolved.insert(0usize, (0u32, 0x30000u32));
        loca.update(&resolved);
        assert!(loca.is_long_format);
    }
}
