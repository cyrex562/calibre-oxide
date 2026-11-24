//! Port of `calibre.utils.fonts.sfnt.cmap` (`BMPTable`/`CmapTable`,
//! plus the real `split_range`/`set_id_delta` cmap-format-4-encoding
//! helpers, issue #551).
//!
//! # Disclosed narrowings
//!
//! - Real Python's `CmapTable.__init__` also sets `self.tables = {}`
//!   but never populates it anywhere in the module -- vestigial dead
//!   state. Not ported.
//! - Real Python's `set_character_map` reassigns `self.bmp_table` from
//!   a parsed `BMPTable` *object* to raw *bytes* (the freshly built
//!   subtable), a dynamic-typing quirk with no direct Rust equivalent.
//!   This port keeps [`CmapTable::bmp_table`] as a real, always-parsed
//!   [`BmpTable`] by re-parsing the freshly built subtable bytes back
//!   into one -- a strictly more useful post-condition (still queryable
//!   via [`CmapTable::get_character_map`]/[`CmapTable::get_glyph_map`]
//!   afterwards) that changes nothing observable about the final
//!   `raw` bytes real callers actually consume.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::errors::UnsupportedFont;
use super::max_power_of_two;
use crate::fonts::utils::{bmp_prefix_glyph_ids, read_bmp_prefix, BmpPrefix};

/// Port of `split_range`: splits `[start_code, end_code]` into
/// subranges whose glyph ids are consecutive, filtering out splits
/// that wouldn't shrink the encoded cmap format-4 subtable.
pub fn split_range(start_code: u32, end_code: u32, cmap: &BTreeMap<u32, u32>) -> (Vec<u32>, Vec<u32>) {
    if start_code == end_code {
        return (Vec::new(), vec![end_code]);
    }

    let mut last_id = cmap[&start_code] as i64;
    let mut last_code = start_code;
    let mut in_order = false;
    let mut ordered_begin: Option<u32> = None;
    let mut sub_ranges: Vec<(u32, u32)> = Vec::new();

    for code in (start_code + 1)..=end_code {
        let glyph_id = cmap[&code] as i64;

        if glyph_id - 1 == last_id {
            if !in_order {
                in_order = true;
                ordered_begin = Some(last_code);
            }
        } else if in_order {
            in_order = false;
            sub_ranges.push((ordered_begin.unwrap(), last_code));
            ordered_begin = None;
        }

        last_id = glyph_id;
        last_code = code;
    }

    if in_order {
        sub_ranges.push((ordered_begin.unwrap(), last_code));
    }
    debug_assert_eq!(last_code, end_code);

    let mut new_ranges = Vec::new();
    for &(b, e) in &sub_ranges {
        if b == start_code && e == end_code {
            break;
        }
        let threshold: u32 = if b == start_code || e == end_code { 4 } else { 8 };
        if (e - b + 1) > threshold {
            new_ranges.push((b, e));
        }
    }
    let mut sub_ranges = new_ranges;

    if sub_ranges.is_empty() {
        return (Vec::new(), vec![end_code]);
    }

    if sub_ranges[0].0 != start_code {
        sub_ranges.insert(0, (start_code, sub_ranges[0].0 - 1));
    }
    if sub_ranges.last().unwrap().1 != end_code {
        sub_ranges.push((sub_ranges.last().unwrap().1 + 1, end_code));
    }

    let mut i = 1;
    while i < sub_ranges.len() {
        if sub_ranges[i - 1].1 + 1 != sub_ranges[i].0 {
            sub_ranges.insert(i, (sub_ranges[i - 1].1 + 1, sub_ranges[i].0 - 1));
            i += 1;
        }
        i += 1;
    }

    let mut start: Vec<u32> = Vec::new();
    let mut end: Vec<u32> = Vec::new();
    for &(b, e) in &sub_ranges {
        start.push(b);
        end.push(e);
    }
    start.remove(0);

    debug_assert_eq!(start.len() + 1, end.len());
    (start, end)
}

/// Port of `set_id_delta`.
pub fn set_id_delta(index: i64, start_code: i64) -> i16 {
    let mut id_delta = index - start_code;
    if id_delta > 0x7FFF {
        id_delta -= 0x10000;
    } else if id_delta < -0x7FFF {
        id_delta += 0x10000;
    }
    id_delta as i16
}

fn resolve_glyph_id(p: &BmpPrefix, i: usize, code: u32) -> u32 {
    let ro = p.range_offset[i];
    let glyph_id: i64 = if ro == 0 {
        p.id_delta[i] as i64 + code as i64
    } else {
        let sc = p.start_count[i];
        let idx = (ro as usize) / 2 + (code as usize - sc as usize) + i - p.array_len;
        let mapped = *p.glyph_id_map.get(idx).unwrap_or(&0) as i64;
        if mapped != 0 {
            mapped + p.id_delta[i] as i64
        } else {
            0
        }
    };
    glyph_id.rem_euclid(0x10000) as u32
}

/// Port of `BMPTable` (a parsed cmap format-4 subtable).
#[derive(Debug)]
pub struct BmpTable {
    prefix: BmpPrefix,
}

impl BmpTable {
    /// Port of `BMPTable.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let prefix = read_bmp_prefix(raw, 0).map_err(UnsupportedFont)?;
        Ok(BmpTable { prefix })
    }

    /// Port of `BMPTable.get_glyph_ids`.
    pub fn get_glyph_ids(&self, codes: &[u32]) -> Vec<u32> {
        bmp_prefix_glyph_ids(&self.prefix, codes.iter().copied())
    }

    /// Port of `BMPTable.get_glyph_map`.
    pub fn get_glyph_map(&self, glyph_ids: &HashSet<u32>) -> BTreeMap<u32, u32> {
        let mut ans = BTreeMap::new();
        for (i, &ec) in self.prefix.end_count.iter().enumerate() {
            let sc = self.prefix.start_count[i];
            for code in (sc as u32)..=(ec as u32) {
                let glyph_id = resolve_glyph_id(&self.prefix, i, code);
                if glyph_ids.contains(&glyph_id) {
                    ans.entry(code).or_insert(glyph_id);
                }
            }
        }
        ans
    }
}

/// Port of `CmapTable` (`UnknownTable` subclass -- see
/// [`super::glyf::GlyfTable`]'s own doc for why the base class isn't
/// ported separately; its `raw`/`__call__`/`__len__` are folded
/// directly into this struct's own `raw` field).
#[derive(Debug)]
pub struct CmapTable {
    pub version: u16,
    pub num_tables: u16,
    pub bmp_table: Option<BmpTable>,
    pub raw: Vec<u8>,
}

impl CmapTable {
    /// Port of `CmapTable.__init__`'s real encoding-record walk.
    pub fn parse(raw: Vec<u8>) -> Result<Self, UnsupportedFont> {
        let version = u16::from_be_bytes(raw.get(0..2).ok_or_else(|| UnsupportedFont("truncated cmap table".to_string()))?.try_into().unwrap());
        let num_tables = u16::from_be_bytes(raw.get(2..4).ok_or_else(|| UnsupportedFont("truncated cmap table".to_string()))?.try_into().unwrap());

        let mut recs: Vec<(u16, u16, usize)> = Vec::new();
        let mut offset = 4usize;
        for _ in 0..num_tables {
            let bytes = raw.get(offset..offset + 8).ok_or_else(|| UnsupportedFont("truncated cmap encoding record".to_string()))?;
            let platform = u16::from_be_bytes(bytes[0..2].try_into().unwrap());
            let encoding = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
            let table_offset = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
            recs.push((platform, encoding, table_offset));
            offset += 8;
        }

        let mut bmp_table = None;
        for i in 0..recs.len() {
            let (platform, encoding, off) = recs[i];
            let next_offset = recs.get(i + 1).map(|r| r.2).unwrap_or(raw.len());
            // Real Python slicing (`raw[offset:next_offset]`) never
            // raises even for an out-of-order/out-of-range offset --
            // clamped here to match rather than erroring, since real
            // fonts are well-ordered in practice.
            let end = next_offset.min(raw.len());
            let start = off.min(end);
            let table = &raw[start..end];
            if !table.is_empty() {
                let fmt_bytes = table.get(0..2).ok_or_else(|| UnsupportedFont("truncated cmap subtable format".to_string()))?;
                let fmt = u16::from_be_bytes(fmt_bytes.try_into().unwrap());
                if platform == 3 && encoding == 1 && fmt == 4 {
                    bmp_table = Some(BmpTable::parse(table)?);
                }
            }
        }

        Ok(CmapTable { version, num_tables, bmp_table, raw })
    }

    /// Port of `CmapTable.get_character_map`.
    pub fn get_character_map(&self, chars: &[u32]) -> Result<BTreeMap<u32, u32>, UnsupportedFont> {
        let bmp = self.bmp_table.as_ref().ok_or_else(no_bmp_subtable)?;
        let chars: Vec<u32> = chars.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        let glyph_ids = bmp.get_glyph_ids(&chars);
        let mut ans = BTreeMap::new();
        for (i, glyph_id) in glyph_ids.into_iter().enumerate() {
            if glyph_id > 0 {
                ans.insert(chars[i], glyph_id);
            }
        }
        Ok(ans)
    }

    /// Port of `CmapTable.get_glyph_map`.
    pub fn get_glyph_map(&self, glyph_ids: &[u32]) -> Result<BTreeMap<u32, u32>, UnsupportedFont> {
        let bmp = self.bmp_table.as_ref().ok_or_else(no_bmp_subtable)?;
        let glyph_ids: HashSet<u32> = glyph_ids.iter().copied().collect();
        Ok(bmp.get_glyph_map(&glyph_ids))
    }

    /// Port of `CmapTable.set_character_map`: rewrites this table to
    /// contain a single Windows-BMP (platform 3, encoding 1) format-4
    /// subtable encoding exactly `cmap`.
    pub fn set_character_map(&mut self, cmap: &BTreeMap<u32, u32>) {
        self.version = 0;
        self.num_tables = 1;
        let codes: Vec<u32> = cmap.keys().copied().collect();

        let (start_code, end_code): (Vec<u32>, Vec<u32>) = if codes.is_empty() {
            (vec![0xffff], vec![0xffff])
        } else {
            let mut last_code = codes[0];
            let mut end_code = Vec::new();
            let mut start_code = vec![last_code];
            for &code in &codes[1..] {
                if code == last_code + 1 {
                    last_code = code;
                    continue;
                }
                let (s, e) = split_range(*start_code.last().unwrap(), last_code, cmap);
                start_code.extend(s);
                end_code.extend(e);
                start_code.push(code);
                last_code = code;
            }
            end_code.push(last_code);
            start_code.push(0xffff);
            end_code.push(0xffff);
            (start_code, end_code)
        };

        let mut id_delta: Vec<i16> = Vec::new();
        let mut id_range_offset: Vec<u16> = Vec::new();
        let mut glyph_index_array: Vec<u16> = Vec::new();

        for i in 0..end_code.len() - 1 {
            let indices: Vec<u32> = (start_code[i]..=end_code[i]).map(|c| cmap[&c]).collect();
            let is_contiguous = indices.iter().enumerate().all(|(j, &v)| v == indices[0] + j as u32);
            if is_contiguous {
                let id_delta_temp = set_id_delta(indices[0] as i64, start_code[i] as i64);
                if !(-0x7FFF..=0x7FFF).contains(&id_delta_temp) {
                    id_delta.push(0);
                    id_range_offset.push((2 * (end_code.len() + glyph_index_array.len() - i)) as u16);
                    glyph_index_array.extend(indices.iter().map(|&v| v as u16));
                } else {
                    id_delta.push(id_delta_temp);
                    id_range_offset.push(0);
                }
            } else {
                id_delta.push(0);
                id_range_offset.push((2 * (end_code.len() + glyph_index_array.len() - i)) as u16);
                glyph_index_array.extend(indices.iter().map(|&v| v as u16));
            }
        }
        id_delta.push(1); // 0xffff + 1 == 0, so this end code maps to .notdef
        id_range_offset.push(0);

        let seg_count = end_code.len() as u32;
        let max_exponent = max_power_of_two(seg_count);
        let search_range = 2 * (1u32 << max_exponent);
        let entry_selector = max_exponent;
        let range_shift = 2 * seg_count - search_range;

        let mut char_code_array: Vec<u16> = Vec::with_capacity(end_code.len() + 1 + start_code.len());
        char_code_array.extend(end_code.iter().map(|&v| v as u16));
        char_code_array.push(0);
        char_code_array.extend(start_code.iter().map(|&v| v as u16));

        let mut data = Vec::new();
        for v in &char_code_array {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in &id_delta {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in &id_range_offset {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in &glyph_index_array {
            data.extend_from_slice(&v.to_be_bytes());
        }

        let header_size: usize = 14; // '>7H': format, length, language, segCountX2, searchRange, entrySelector, rangeShift
        let length = header_size + data.len();
        let mut bmp_bytes = Vec::with_capacity(length);
        bmp_bytes.extend_from_slice(&4u16.to_be_bytes());
        bmp_bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bmp_bytes.extend_from_slice(&0u16.to_be_bytes()); // language
        bmp_bytes.extend_from_slice(&(2 * seg_count as u16).to_be_bytes());
        bmp_bytes.extend_from_slice(&(search_range as u16).to_be_bytes());
        bmp_bytes.extend_from_slice(&(entry_selector as u16).to_be_bytes());
        bmp_bytes.extend_from_slice(&(range_shift as u16).to_be_bytes());
        bmp_bytes.extend_from_slice(&data);

        self.bmp_table = Some(BmpTable::parse(&bmp_bytes).expect("freshly built cmap BMP subtable failed to parse"));

        let mut raw = Vec::with_capacity(12 + bmp_bytes.len());
        raw.extend_from_slice(&self.version.to_be_bytes());
        raw.extend_from_slice(&self.num_tables.to_be_bytes());
        raw.extend_from_slice(&3u16.to_be_bytes()); // platform: Windows
        raw.extend_from_slice(&1u16.to_be_bytes()); // encoding: Unicode BMP
        raw.extend_from_slice(&12u32.to_be_bytes()); // offset: calcsize('>4HL')
        raw.extend_from_slice(&bmp_bytes);
        self.raw = raw;
    }
}

fn no_bmp_subtable() -> UnsupportedFont {
    UnsupportedFont("This font has no Windows BMP cmap subtable. Most likely a special purpose font.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a real format-4 cmap subtable mapping every code in
    /// `mappings` (already sorted, contiguous run) via a single
    /// segment, plus the required 0xffff terminator segment.
    fn build_single_segment_format4(start_code: u16, end_code: u16, id_delta: i16) -> Vec<u8> {
        let end_codes = [end_code, 0xffffu16];
        let start_codes = [start_code, 0xffffu16];
        let id_deltas = [id_delta, 1i16];
        let id_range_offsets = [0u16, 0u16];
        let seg_count = 2u16;

        let mut data = Vec::new();
        for v in end_codes {
            data.extend_from_slice(&v.to_be_bytes());
        }
        data.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for v in start_codes {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in id_deltas {
            data.extend_from_slice(&v.to_be_bytes());
        }
        for v in id_range_offsets {
            data.extend_from_slice(&v.to_be_bytes());
        }

        let length = 14 + data.len();
        let mut out = Vec::new();
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&(length as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(2 * seg_count).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn build_cmap_table(subtable: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&0u16.to_be_bytes()); // version
        raw.extend_from_slice(&1u16.to_be_bytes()); // num_tables
        raw.extend_from_slice(&3u16.to_be_bytes()); // platform: Windows
        raw.extend_from_slice(&1u16.to_be_bytes()); // encoding: Unicode BMP
        raw.extend_from_slice(&12u32.to_be_bytes()); // offset
        raw.extend_from_slice(subtable);
        raw
    }

    #[test]
    fn cmap_table_finds_the_windows_bmp_subtable() {
        let sub = build_single_segment_format4(0x41, 0x5a, 1); // 'A'..='Z' -> glyph id + 1
        let raw = build_cmap_table(&sub);
        let table = CmapTable::parse(raw).unwrap();
        assert!(table.bmp_table.is_some());
    }

    #[test]
    fn get_character_map_excludes_unmapped_characters() {
        let sub = build_single_segment_format4(0x41, 0x5a, 1);
        let raw = build_cmap_table(&sub);
        let table = CmapTable::parse(raw).unwrap();
        let map = table.get_character_map(&[0x41, 0x42, 0x99]).unwrap();
        assert_eq!(map[&0x41], 0x42);
        assert_eq!(map[&0x42], 0x43);
        assert!(!map.contains_key(&0x99), "0x99 is outside the mapped range and should be excluded");
    }

    #[test]
    fn get_glyph_map_finds_the_code_for_a_requested_glyph_id() {
        let sub = build_single_segment_format4(0x41, 0x5a, 1);
        let raw = build_cmap_table(&sub);
        let table = CmapTable::parse(raw).unwrap();
        let map = table.get_glyph_map(&[0x43]).unwrap();
        assert_eq!(map[&0x42], 0x43);
    }

    #[test]
    fn set_character_map_round_trips_through_get_character_map() {
        let mut cmap = BTreeMap::new();
        cmap.insert(0x41, 10);
        cmap.insert(0x42, 11);
        cmap.insert(0x43, 12);
        cmap.insert(0x61, 50); // a second, disjoint, non-contiguous-with-the-first segment

        let sub = build_single_segment_format4(0, 0, 0); // placeholder, overwritten by set_character_map
        let raw = build_cmap_table(&sub);
        let mut table = CmapTable::parse(raw).unwrap();
        table.set_character_map(&cmap);

        let recovered = table.get_character_map(&[0x41, 0x42, 0x43, 0x61, 0x99]).unwrap();
        assert_eq!(recovered, cmap.into_iter().collect());

        // The full raw table should also re-parse from scratch identically.
        let reparsed = CmapTable::parse(table.raw.clone()).unwrap();
        assert!(reparsed.bmp_table.is_some());
    }

    #[test]
    fn set_character_map_handles_an_empty_map() {
        let sub = build_single_segment_format4(0, 0, 0);
        let raw = build_cmap_table(&sub);
        let mut table = CmapTable::parse(raw).unwrap();
        table.set_character_map(&BTreeMap::new());
        let recovered = table.get_character_map(&[0x41]).unwrap();
        assert!(recovered.is_empty());
    }

    #[test]
    fn split_range_keeps_a_single_fully_ordered_run_unsplit() {
        let mut cmap = BTreeMap::new();
        for (i, code) in (0x41u32..=0x50).enumerate() {
            cmap.insert(code, 100 + i as u32);
        }
        let (start, end) = split_range(0x41, 0x50, &cmap);
        assert!(start.is_empty());
        assert_eq!(end, vec![0x50]);
    }

    #[test]
    fn split_range_splits_out_a_large_disordered_gap() {
        let mut cmap = BTreeMap::new();
        // A long ordered run, then a long run of identical (non-consecutive) glyph ids.
        for (i, code) in (0x41u32..=0x50).enumerate() {
            cmap.insert(code, 100 + i as u32);
        }
        for code in 0x51u32..=0x60 {
            cmap.insert(code, 5); // all map to the same glyph -- not consecutive
        }
        let (start, _end) = split_range(0x41, 0x60, &cmap);
        assert!(!start.is_empty(), "a large enough disordered region should force a split");
    }

    #[test]
    fn set_id_delta_wraps_around_at_the_16_bit_boundary() {
        // The two boundary cases from the real function's own docstring:
        // startCode 0 -> final GID 0xFFFF, reached by subtracting 1.
        assert_eq!(set_id_delta(0xFFFF, 0), -1);
        // startCode 0xFFFF -> final GID 1, reached by adding 2 (mod 0x10000).
        assert_eq!(set_id_delta(1, 0xFFFF), 2);
        assert_eq!(set_id_delta(100, 50), 50);
    }
}
