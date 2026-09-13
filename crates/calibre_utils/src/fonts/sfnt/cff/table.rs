//! Port of `calibre.utils.fonts.sfnt.cff.table` (issue #564, split from
//! #554/#65): the CFF `Index`/`Strings`/`Charset`/`CFF` table reader,
//! built on #563's DICT codec ([`crate::fonts::sfnt::cff::dict_data`]).
//!
//! # Scope: the reader only, not `CFFTable`'s container wiring
//!
//! Real upstream's `CFFTable` (the bottom of this file) bridges into
//! `sfnt/container.py`'s generic per-table `UnknownTable` dispatch and
//! its `subset()` method calls straight into `writer.py`'s `Subset`
//! class -- issue #565's scope, not yet built. This module ports
//! [`Cff::parse`] (real `CFF.__init__`) and the structures it needs
//! (`Index`/`Strings`/`Charset`/`Subrs`/`CharStringsIndex`), which is
//! everything a real caller needs to *read* a CFF table; wiring a
//! `CFFTable`-equivalent into this crate's own sfnt container, and
//! `subset()`, are #565's job once the writer exists to subset onto.
//!
//! # `Index`'s real offset-table layout
//!
//! A CFF INDEX is a count-prefixed table of variable-length byte
//! strings: a 2-byte count, a 1-byte offset-size (1/2/3/4, the 3-byte
//! form packed as 3 raw bytes -- no native integer type has that
//! width, so it's read as `\0 || 3 bytes` big-endian, same as real
//! upstream's own `unpack('>L', b'\0' + raw[i:i+3])`), then
//! `count + 1` offsets (the last one giving the final entry's end),
//! all of which are **1-based relative to one byte before the data
//! section** -- real upstream's own `offset += offset_size*(count+1)
//! - 1` computes exactly that base once, and every entry is sliced as
//! `raw[base+off : base+noff]`.

use crate::fonts::sfnt::cff::constants::{CFF_STANDARD_STRINGS, STANDARD_CHARSETS};
use crate::fonts::sfnt::cff::dict_data::{Dict, Operand, Value};
use crate::fonts::sfnt::errors::UnsupportedFont;

fn trunc() -> UnsupportedFont {
    UnsupportedFont("Truncated CFF table".to_string())
}

fn read_u16(raw: &[u8], offset: usize) -> Result<u16, UnsupportedFont> {
    raw.get(offset..offset + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or_else(trunc)
}

fn read_u24(raw: &[u8], offset: usize) -> Result<u32, UnsupportedFont> {
    raw.get(offset..offset + 3).map(|b| u32::from_be_bytes([0, b[0], b[1], b[2]])).ok_or_else(trunc)
}

fn read_u32(raw: &[u8], offset: usize) -> Result<u32, UnsupportedFont> {
    raw.get(offset..offset + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(trunc)
}

/// Port of `Index.__init__`. Returns the parsed entries plus the real
/// `self.pos` (the byte offset immediately after this INDEX, where the
/// next structure begins).
fn parse_index_with_prepend(raw: &[u8], offset: usize, prepend: Vec<Vec<u8>>) -> Result<(Vec<Vec<u8>>, usize), UnsupportedFont> {
    let count = read_u16(raw, offset)?;
    let mut offset = offset + 2;
    let mut items = prepend;
    let mut pos = offset;
    if count > 0 {
        let offset_size = *raw.get(offset).ok_or_else(trunc)?;
        offset += 1;
        let n = count as usize + 1;
        let offsets: Vec<u32> = match offset_size {
            1 => (0..n).map(|i| raw.get(offset + i).map(|&b| b as u32).ok_or_else(trunc)).collect::<Result<_, _>>()?,
            2 => (0..n).map(|i| read_u16(raw, offset + i * 2).map(u32::from)).collect::<Result<_, _>>()?,
            3 => (0..n).map(|i| read_u24(raw, offset + i * 3)).collect::<Result<_, _>>()?,
            4 => (0..n).map(|i| read_u32(raw, offset + i * 4)).collect::<Result<_, _>>()?,
            _ => return Err(UnsupportedFont(format!("Unsupported CFF INDEX offset size: {offset_size}"))),
        };
        offset += offset_size as usize * n - 1;
        for pair in offsets.windows(2) {
            let (off, noff) = (pair[0] as usize, pair[1] as usize);
            let start = offset + off;
            let end = offset + noff;
            items.push(raw.get(start..end).ok_or_else(trunc)?.to_vec());
        }
        pos = offset + *offsets.last().unwrap_or(&0) as usize;
    }
    Ok((items, pos))
}

fn parse_index(raw: &[u8], offset: usize) -> Result<(Vec<Vec<u8>>, usize), UnsupportedFont> {
    parse_index_with_prepend(raw, offset, Vec::new())
}

/// Port of `Strings.__init__`: an `Index` prepended with the 391 CFF
/// Standard Strings, so SID `n` for `n < 391` resolves to a standard
/// string and `n >= 391` resolves to the font's own entry `n - 391`.
fn parse_strings(raw: &[u8], offset: usize) -> Result<(Vec<String>, usize), UnsupportedFont> {
    let prepend: Vec<Vec<u8>> = CFF_STANDARD_STRINGS.iter().map(|s| s.as_bytes().to_vec()).collect();
    let (items, pos) = parse_index_with_prepend(raw, offset, prepend)?;
    let strings = items.into_iter().map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
    Ok((strings, pos))
}

/// Port of `Charset`: resolves glyph ids to glyph names, either via one
/// of the 3 predefined [`STANDARD_CHARSETS`] or a font-embedded
/// charset table (format 0, 1, or 2).
#[derive(Debug, Clone)]
pub enum Charset {
    Standard(usize),
    Custom(Vec<String>),
}

impl Charset {
    /// Port of `Charset.__init__` + `parse_fmt0`/`parse_fmt1`.
    pub fn parse(raw: &[u8], offset: usize, strings: &[String], num_glyphs: usize, is_cid: bool) -> Result<Self, UnsupportedFont> {
        if matches!(offset, 0 | 1 | 2) {
            if is_cid {
                return Err(UnsupportedFont("CID font must not use a standard charset".to_string()));
            }
            return Ok(Charset::Standard(offset));
        }
        let mut names = vec![".notdef".to_string()];
        let fmt = *raw.get(offset).ok_or_else(trunc)?;
        let mut offset = offset + 1;
        let glyph_name = |x: u16| if is_cid { format!("cid{x:05}") } else { strings.get(x as usize).cloned().unwrap_or_default() };
        match fmt {
            0 => {
                for _ in 1..num_glyphs {
                    let id = read_u16(raw, offset)?;
                    offset += 2;
                    names.push(glyph_name(id));
                }
            }
            1 | 2 => {
                let two_byte = fmt == 2;
                let mut count = 1usize;
                while count < num_glyphs {
                    let first = read_u16(raw, offset)?;
                    offset += 2;
                    let nleft = if two_byte {
                        let v = read_u16(raw, offset)?;
                        offset += 2;
                        v
                    } else {
                        let v = *raw.get(offset).ok_or_else(trunc)?;
                        offset += 1;
                        v as u16
                    };
                    count += nleft as usize + 1;
                    for x in first..=first.saturating_add(nleft) {
                        names.push(glyph_name(x));
                    }
                }
            }
            other => return Err(UnsupportedFont(format!("This font uses unsupported charset table format: {other}"))),
        }
        Ok(Charset::Custom(names))
    }

    /// Port of `Charset.lookup`/`safe_lookup`. Real upstream has two
    /// methods (`lookup` raises, `safe_lookup` catches) because
    /// `lookup` can throw; this port's `lookup` already never panics
    /// (returns `None`), so both real methods collapse to one.
    pub fn lookup(&self, glyph_id: usize) -> Option<String> {
        match self {
            Charset::Standard(idx) => STANDARD_CHARSETS.get(*idx).and_then(|cs| cs.get(glyph_id)).map(|s| s.to_string()),
            Charset::Custom(names) => names.get(glyph_id).cloned(),
        }
    }

    pub fn safe_lookup(&self, glyph_id: usize) -> Option<String> {
        self.lookup(glyph_id)
    }
}

fn value_int(value: &Value, index: usize) -> Result<i64, UnsupportedFont> {
    match value.get(index) {
        Some(Operand::Int(n)) => Ok(*n),
        Some(Operand::Float(f)) => Ok(*f as i64),
        _ => Err(UnsupportedFont("Expected a numeric CFF DICT value".to_string())),
    }
}

/// Port of the top-level `CFF` class: parses a whole CFF table into its
/// header, name/top-dict/string/global-subr indices, the decompiled
/// `TopDict`/`PrivateDict`, the glyph CharStrings, and the [`Charset`].
#[derive(Debug)]
pub struct Cff {
    pub major_version: u8,
    pub minor_version: u8,
    pub top_dict: Dict,
    pub is_cid: bool,
    pub char_strings: Vec<Vec<u8>>,
    pub num_glyphs: usize,
    pub private_dict: Option<Dict>,
    pub private_subrs: Option<Vec<Vec<u8>>>,
    pub global_subrs: Vec<Vec<u8>>,
    pub strings: Vec<String>,
    pub charset: Charset,
}

impl Cff {
    /// Port of `CFF.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let major_version = *raw.first().ok_or_else(trunc)?;
        let minor_version = *raw.get(1).ok_or_else(trunc)?;
        let header_size = *raw.get(2).ok_or_else(trunc)?;
        if (major_version, minor_version) != (1, 0) {
            return Err(UnsupportedFont(format!("The CFF table has unknown version: ({major_version}, {minor_version})")));
        }
        let offset = header_size as usize;

        let (font_names, offset) = parse_index(raw, offset)?;
        if font_names.len() > 1 {
            return Err(UnsupportedFont("CFF table has more than one font.".to_string()));
        }

        let (top_index, offset) = parse_index(raw, offset)?;
        let (strings, offset) = parse_strings(raw, offset)?;
        let (global_subrs, _offset) = parse_index(raw, offset)?;

        let top0 = top_index.first().ok_or_else(|| UnsupportedFont("CFF Top DICT INDEX is empty".to_string()))?;
        let mut top_dict = Dict::top();
        top_dict.decompile(&strings, top0)?;

        let is_cid = top_dict.get("ROS").is_some();
        if is_cid {
            return Err(UnsupportedFont("Subsetting of CID keyed fonts is not supported".to_string()));
        }

        let cs_offset = top_dict
            .get("CharStrings")
            .ok_or_else(|| UnsupportedFont("This font has no CharStrings".to_string()))
            .and_then(|v| value_int(&v, 0))?;
        let cs_type = top_dict.safe_get("CharstringType").map(|v| value_int(&v, 0)).transpose()?.unwrap_or(2);
        if cs_type != 2 {
            return Err(UnsupportedFont(format!("This font has unsupported CharstringType: {cs_type}")));
        }
        let (char_strings, _) = parse_index(raw, cs_offset as usize)?;
        let num_glyphs = char_strings.len();

        let mut private_dict = None;
        let mut private_subrs = None;
        if let Some(pd) = top_dict.safe_get("Private") {
            if !pd.is_empty() {
                let size = value_int(&pd, 0)? as usize;
                let poffset = value_int(&pd, 1)? as usize;
                let slice = raw.get(poffset..poffset + size).ok_or_else(trunc)?;
                let mut pdict = Dict::private();
                pdict.decompile(&strings, slice)?;
                if let Some(subrs) = pdict.get("Subrs") {
                    let subrs_offset = poffset + value_int(&subrs, 0)? as usize;
                    let (subrs_items, _) = parse_index(raw, subrs_offset)?;
                    private_subrs = Some(subrs_items);
                }
                private_dict = Some(pdict);
            }
        }

        let charset_offset = top_dict.safe_get("charset").map(|v| value_int(&v, 0)).transpose()?.unwrap_or(0);
        let charset = Charset::parse(raw, charset_offset as usize, &strings, num_glyphs, is_cid)?;

        Ok(Cff {
            major_version,
            minor_version,
            top_dict,
            is_cid,
            char_strings,
            num_glyphs,
            private_dict,
            private_subrs,
            global_subrs,
            strings,
            charset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed-width (5-byte: opcode 29 + 4-byte big-endian value) CFF
    /// DICT integer encoding -- what real `compile()` always uses for
    /// `OFFSETS`-listed fields like `charset`/`CharStrings` (see
    /// `dict_data`'s own `TOP_DICT_SCHEMA.offsets`). Used directly here
    /// (rather than the compact variable-width form) so this fixture's
    /// own offsets can be computed in one pass instead of two.
    fn write_offset(v: u32) -> Vec<u8> {
        let mut out = vec![29u8];
        out.extend(v.to_be_bytes());
        out
    }

    /// Builds a minimal, real, parseable CFF table: 1 font, an empty
    /// TopDict (just enough to carry `CharStrings`/`charset`), 2
    /// glyphs (`.notdef` + one real glyph), a custom format-0 charset.
    fn minimal_cff() -> Vec<u8> {
        fn index(entries: &[&[u8]]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend((entries.len() as u16).to_be_bytes());
            if entries.is_empty() {
                return out;
            }
            out.push(1u8); // offset_size = 1
            let mut off = 1u32;
            out.extend((off as u8).to_le_bytes()); // first offset (1-based) = 1
            for e in entries {
                off += e.len() as u32;
                out.push(off as u8);
            }
            for e in entries {
                out.extend_from_slice(e);
            }
            out
        }

        let header = vec![1u8, 0, 4, 4]; // major, minor, header_size=4, offset_size(unused)=4
        let names = index(&[b"Font"]);

        // Fixed-width (5-byte) offset fields mean the Top DICT INDEX's
        // own size never changes once real offset values are known --
        // a single forward pass suffices.
        let build = |charstrings_off: u32, charset_off: u32| -> Vec<u8> {
            let mut dict = Vec::new();
            dict.extend(write_offset(charset_off));
            dict.push(15); // charset
            dict.extend(write_offset(charstrings_off));
            dict.push(17); // CharStrings
            index(&[&dict])
        };

        let strings = index(&[]); // no extra strings needed (charset uses raw glyph ids resolved to standard strings)
        let global_subrs = index(&[]);
        let char_strings = index(&[b"", b"\x0e"]); // .notdef + 1 glyph (endchar operator 14)

        let top_dict_index_len = build(0, 0).len();
        let after_top_dict = header.len() + names.len() + top_dict_index_len + strings.len() + global_subrs.len();
        let charstrings_offset = after_top_dict as u32;
        let charset_offset = charstrings_offset + char_strings.len() as u32;

        // Custom format-0 charset: 1 glyph beyond .notdef -> SID for "space" (SID 1).
        let charset_table = {
            let mut c = vec![0u8]; // format 0
            c.extend(1u16.to_be_bytes()); // SID for glyph 1 = "space"
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
    fn parses_a_minimal_real_cff_table() {
        let raw = minimal_cff();
        let cff = Cff::parse(&raw).unwrap();
        assert_eq!((cff.major_version, cff.minor_version), (1, 0));
        assert!(!cff.is_cid);
        assert_eq!(cff.num_glyphs, 2);
        assert_eq!(cff.char_strings[1], vec![0x0e]);
        assert_eq!(cff.charset.lookup(0).as_deref(), Some(".notdef"));
        assert_eq!(cff.charset.lookup(1).as_deref(), Some("space"));
    }

    #[test]
    fn rejects_an_unknown_cff_version() {
        let mut raw = minimal_cff();
        raw[1] = 5; // minor_version
        let err = Cff::parse(&raw).unwrap_err();
        assert!(err.0.contains("unknown version"));
    }

    #[test]
    fn standard_charset_lookup_resolves_via_the_predefined_table() {
        let cs = Charset::Standard(0); // ISOAdobe
        assert_eq!(cs.lookup(0).as_deref(), Some(".notdef"));
        assert_eq!(cs.lookup(1).as_deref(), Some("space"));
        assert_eq!(cs.safe_lookup(100_000), None);
    }

    #[test]
    fn charset_format1_expands_ranges() {
        let strings: Vec<String> = CFF_STANDARD_STRINGS.iter().map(|s| s.to_string()).collect();
        // format 1: one range (first=1 "space", nLeft=2 -> glyphs 1,2,3), num_glyphs=4 (.notdef+3)
        let mut raw = vec![1u8];
        raw.extend(1u16.to_be_bytes());
        raw.push(2); // nLeft
        let cs = Charset::parse(&raw, 0, &strings, 4, false).unwrap();
        assert_eq!(cs.lookup(0).as_deref(), Some(".notdef"));
        assert_eq!(cs.lookup(1).as_deref(), Some("space"));
        assert_eq!(cs.lookup(2).as_deref(), Some("exclam"));
        assert_eq!(cs.lookup(3).as_deref(), Some("quotedbl"));
    }

    #[test]
    fn index_parses_multiple_variable_length_entries() {
        let mut raw = Vec::new();
        raw.extend(2u16.to_be_bytes());
        raw.push(1u8); // offset_size = 1
        raw.extend([1u8, 3, 6]); // offsets: entry0 len 2, entry1 len 3
        raw.extend(b"ABDEF");
        let (items, pos) = parse_index(&raw, 0).unwrap();
        assert_eq!(items, vec![b"AB".to_vec(), b"DEF".to_vec()]);
        assert_eq!(pos, raw.len());
    }

    #[test]
    fn empty_index_reports_zero_count_and_correct_pos() {
        let raw = 0u16.to_be_bytes();
        let (items, pos) = parse_index(&raw, 0).unwrap();
        assert!(items.is_empty());
        assert_eq!(pos, 2);
    }
}
