//! Port of `calibre.utils.fonts.utils` (issue #548, split from #63): a
//! standalone, dependency-free set of low-level sfnt (TrueType/OpenType)
//! utilities. Deliberately does *not* need the fuller `sfnt/` object
//! model (issues #549-555) -- raw table-directory access plus name/OS2
//! parsing and checksum fixups are simple enough to work with directly.
//!
//! Real payoff: [`get_font_names`]/[`get_font_characteristics`] are
//! exactly what `oeb::polish::embed.rs`'s `embed_all_fonts` (issue #169)
//! cites as a real missing dependency -- whole-font embedding needs
//! font names/characteristics/embeddability, not glyph-level
//! subsetting (that needs the fuller `sfnt` object model, #553).
//!
//! # Disclosed narrowings
//!
//! - **`get_font_characteristics_from_ttlib_os2_table`** (the
//!   `hasattr(raw, 'getUnicodeRanges')` branch) is real upstream's
//!   alternate calling convention for an already-parsed `fontTools`
//!   `ttLib` OS/2 table *object*, not raw bytes. Nothing in this port
//!   constructs such an object, so it's unreachable here -- not
//!   ported.
//! - **`get_printable_characters`** filters out Unicode general
//!   categories C (control/format/private-use/surrogate), Z
//!   (separator), and M (mark/combining) after NFC normalization; this
//!   port only filters `char::is_control()`/`char::is_whitespace()`
//!   (stdlib-only, no combining-mark filtering) since this function is
//!   only reachable from `supports_text`'s default path and
//!   `get_font_for_text` -- both part of the font-scanning/picking
//!   workflow deferred to issue #556, not exercised by anything ported
//!   in this issue.
//! - **`get_font_for_text`/`test_find_font`** need
//!   `calibre.utils.fonts.scanner.font_scanner` (a system-installed-
//!   font resolver), deferred to issue #556. Not ported here.
//! - **`checksum_of_block`**'s real Python always pads with at least
//!   one full zero word, even when `len(raw)` is already a multiple of
//!   4 (`extra = 4 - len(raw) % 4` is `4`, not `0`, when the remainder
//!   is `0`). This has zero effect on the resulting sum (padding with
//!   zero bytes never changes a sum), so this port pads only to the
//!   next multiple of 4 (possibly zero extra bytes) -- same real
//!   checksum value, without the pointless always-at-least-one-word
//!   quirk.

use std::collections::HashMap;

use indexmap::IndexMap;

/// Port of `is_truetype_font`: returns whether `raw` starts with a
/// recognized sfnt version signature, plus the raw 4-byte signature
/// itself (real callers destructure both, e.g. to report the
/// unrecognized signature in an error message).
pub fn is_truetype_font(raw: &[u8]) -> (bool, [u8; 4]) {
    let mut sig = [0u8; 4];
    let n = raw.len().min(4);
    sig[..n].copy_from_slice(&raw[..n]);
    let ok = sig == [0x00, 0x01, 0x00, 0x00] || &sig == b"OTTO";
    (ok, sig)
}

/// One entry from an sfnt file's table directory, port of the tuple
/// `get_tables` yields.
#[derive(Debug, Clone)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub data: Vec<u8>,
    /// Byte offset of this table's own record in the table directory
    /// (not the table's own data -- used by [`set_table_checksum`] to
    /// patch the directory's checksum field back in).
    pub record_offset: usize,
    pub table_offset: usize,
    pub checksum: u32,
}

/// Port of `get_tables`.
pub fn get_tables(raw: &[u8]) -> Vec<TableRecord> {
    let mut out = Vec::new();
    let Some(num_tables_bytes) = raw.get(4..6) else {
        return out;
    };
    let num_tables = u16::from_be_bytes(num_tables_bytes.try_into().unwrap()) as usize;
    let mut offset = 4 * 3;
    for _ in 0..num_tables {
        let Some(record) = raw.get(offset..offset + 16) else {
            break;
        };
        let tag = [record[0], record[1], record[2], record[3]];
        let checksum = u32::from_be_bytes(record[4..8].try_into().unwrap());
        let table_offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
        let table_length = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
        let data = raw.get(table_offset..table_offset.saturating_add(table_length)).unwrap_or(&[]).to_vec();
        out.push(TableRecord {
            tag,
            data,
            record_offset: offset,
            table_offset,
            checksum,
        });
        offset += 4 * 4;
    }
    out
}

/// Port of `get_table`: the named table's directory entry, if present
/// (case-insensitive tag match).
pub fn get_table(raw: &[u8], name: &str) -> Option<TableRecord> {
    let name = name.as_bytes();
    get_tables(raw).into_iter().find(|t| t.tag.iter().zip(name).all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase()) && name.len() == 4)
}

/// A minimal big-endian binary cursor, port of `Unpackable`'s role in
/// `struct.unpack_from` call sites throughout this file.
pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let v = self.data.get(self.pos..self.pos + n).ok_or("truncated font table")?;
        self.pos += n;
        Ok(v)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }

    pub(crate) fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Port of the fields `get_font_characteristics`/
/// `get_font_characteristics_from_ttlib_os2_table` return -- a real
/// struct instead of Python's giant `return_all`-sized tuple (the same
/// data either way; Rust has no need for the two-arity split a tuple
/// return required).
#[derive(Debug, Clone)]
pub struct FontCharacteristics {
    pub version: u16,
    pub char_width: i16,
    pub weight: u16,
    pub width: u16,
    pub fs_type: u16,
    pub subscript_x_size: i16,
    pub subscript_y_size: i16,
    pub subscript_x_offset: i16,
    pub subscript_y_offset: i16,
    pub superscript_x_size: i16,
    pub superscript_y_size: i16,
    pub superscript_x_offset: i16,
    pub superscript_y_offset: i16,
    pub strikeout_size: i16,
    pub strikeout_position: i16,
    pub family_class: i16,
    pub panose: [u8; 10],
    pub selection: u16,
    pub is_italic: bool,
    pub is_bold: bool,
    pub is_regular: bool,
    pub is_wws: bool,
    pub is_oblique: bool,
}

/// Port of `get_font_characteristics(raw, raw_is_table=False)`. See
/// the module doc for the disclosed `ttLib`-object-calling-convention
/// narrowing.
pub fn get_font_characteristics(raw: &[u8]) -> Result<FontCharacteristics, String> {
    let table = get_table(raw, "os/2").ok_or("Not a supported font, has no OS/2 table")?;
    get_font_characteristics_from_os2_table(&table.data)
}

/// Port of the `raw_is_table=True` path.
pub fn get_font_characteristics_from_os2_table(os2: &[u8]) -> Result<FontCharacteristics, String> {
    let mut c = Cursor::new(os2);
    let version = c.u16()?;
    let char_width = c.i16()?;
    let weight = c.u16()?;
    let width = c.u16()?;
    let fs_type = c.u16()?;
    let subscript_x_size = c.i16()?;
    let subscript_y_size = c.i16()?;
    let subscript_x_offset = c.i16()?;
    let subscript_y_offset = c.i16()?;
    let superscript_x_size = c.i16()?;
    let superscript_y_size = c.i16()?;
    let superscript_x_offset = c.i16()?;
    let superscript_y_offset = c.i16()?;
    let strikeout_size = c.i16()?;
    let strikeout_position = c.i16()?;
    let family_class = c.i16()?;
    let mut panose = [0u8; 10];
    panose.copy_from_slice(c.take(10)?);
    let _range1 = c.u32()?;
    let _range2 = c.u32()?;
    let _range3 = c.u32()?;
    let _range4 = c.u32()?;
    let _vendor_id = c.take(4)?;
    let selection = c.u16()?;

    Ok(FontCharacteristics {
        version,
        char_width,
        weight,
        width,
        fs_type,
        subscript_x_size,
        subscript_y_size,
        subscript_x_offset,
        subscript_y_offset,
        superscript_x_size,
        superscript_y_size,
        superscript_x_offset,
        superscript_y_offset,
        strikeout_size,
        strikeout_position,
        family_class,
        panose,
        selection,
        is_italic: (selection & (1 << 0)) != 0,
        is_bold: (selection & (1 << 5)) != 0,
        is_regular: (selection & (1 << 6)) != 0,
        is_wws: (selection & (1 << 8)) != 0,
        is_oblique: (selection & (1 << 9)) != 0,
    })
}

/// Port of `panose_to_css_generic_family`. The canonical copy --
/// `docx::fonts::panose_to_css_generic_family` (ported before this
/// crate had a `fonts` module of its own) delegates here.
pub fn panose_to_css_generic_family(panose: &[u8]) -> String {
    let proportion = panose.get(3).copied().unwrap_or(0);
    if proportion == 9 {
        return "monospace".to_string();
    }
    let family_type = panose.first().copied().unwrap_or(0);
    if family_type == 3 {
        return "cursive".to_string();
    }
    if family_type == 4 {
        return "fantasy".to_string();
    }
    let serif_style = panose.get(1).copied().unwrap_or(0);
    if matches!(serif_style, 11 | 12 | 13) {
        return "sans-serif".to_string();
    }
    "serif".to_string()
}

fn decode_with(codec: &str, src: &[u8]) -> Option<String> {
    match codec {
        "ascii" => {
            if src.is_ascii() {
                String::from_utf8(src.to_vec()).ok()
            } else {
                None
            }
        }
        "iso-8859-1" => Some(src.iter().map(|&b| b as char).collect()),
        "utf-16-be" => decode_utf16_be(src),
        "utf-32-be" => decode_utf32_be(src),
        _ => None,
    }
}

fn decode_utf16_be(src: &[u8]) -> Option<String> {
    if src.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = src.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

fn decode_utf32_be(src: &[u8]) -> Option<String> {
    if src.len() % 4 != 0 {
        return None;
    }
    src.chunks_exact(4).map(|c| char::from_u32(u32::from_be_bytes([c[0], c[1], c[2], c[3]]))).collect()
}

/// Port of `decode_name_record`. `recs` is `(platform_id, encoding_id,
/// language_id, raw_bytes)` per record for one `name_id`, matching
/// real upstream's own grouping.
fn decode_name_record(recs: &[(u16, u16, u16, Vec<u8>)]) -> Option<String> {
    if recs.is_empty() {
        return None;
    }
    let mut unicode_names: IndexMap<u16, String> = IndexMap::new();
    let mut windows_names: IndexMap<u16, String> = IndexMap::new();
    let mut mac_names: IndexMap<u16, String> = IndexMap::new();

    for (platform_id, encoding_id, language_id, src) in recs {
        if *language_id > 0x8000 {
            continue;
        }
        match platform_id {
            0 => {
                if *encoding_id < 4 {
                    if let Some(s) = decode_utf16_be(src) {
                        unicode_names.insert(*language_id, s);
                    }
                }
            }
            1 => {
                if let Ok(s) = String::from_utf8(src.clone()) {
                    mac_names.insert(*language_id, s);
                }
            }
            2 => {
                let codec = match encoding_id {
                    0 => Some("ascii"),
                    1 => Some("utf-16-be"),
                    2 => Some("iso-8859-1"),
                    _ => None,
                };
                if let Some(codec) = codec {
                    if let Some(s) = decode_with(codec, src) {
                        unicode_names.insert(*language_id, s);
                    }
                }
            }
            3 => {
                let bits = match encoding_id {
                    1 => Some("utf-16-be"),
                    10 => Some("utf-32-be"),
                    _ => None,
                };
                if let Some(codec) = bits {
                    if let Some(s) = decode_with(codec, src) {
                        windows_names.insert(*language_id, s);
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(s) = windows_names.get(&1033) {
        return Some(s.clone());
    }
    for lang in [3081u16, 10249, 4105, 9225, 16393, 6153, 8201, 17417, 5129, 13321, 18441, 7177, 11273, 2057, 12297] {
        if let Some(s) = windows_names.get(&lang) {
            return Some(s.clone());
        }
    }
    if let Some(s) = mac_names.get(&0) {
        return Some(s.clone());
    }
    unicode_names.into_values().next()
}

struct NameRecord {
    platform_id: u16,
    encoding_id: u16,
    language_id: u16,
    name_id: u16,
    data: Vec<u8>,
}

/// Port of `_get_font_names`, minus the `raw`/`raw_is_table` dispatch
/// (callers already have `table: &[u8]`).
fn parse_name_records(table: &[u8]) -> IndexMap<u16, Vec<(u16, u16, u16, Vec<u8>)>> {
    let mut out: IndexMap<u16, Vec<(u16, u16, u16, Vec<u8>)>> = IndexMap::new();
    let Some(header) = table.get(0..6) else {
        return out;
    };
    let count = u16::from_be_bytes(header[2..4].try_into().unwrap()) as usize;
    let string_offset = u16::from_be_bytes(header[4..6].try_into().unwrap()) as usize;

    let mut records = Vec::new();
    for i in 0..count {
        let rec_offset = 6 + i * 12;
        let Some(bytes) = table.get(rec_offset..rec_offset + 12) else {
            break;
        };
        let platform_id = u16::from_be_bytes(bytes[0..2].try_into().unwrap());
        let encoding_id = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
        let language_id = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
        let name_id = u16::from_be_bytes(bytes[6..8].try_into().unwrap());
        let length = u16::from_be_bytes(bytes[8..10].try_into().unwrap()) as usize;
        let str_off = u16::from_be_bytes(bytes[10..12].try_into().unwrap()) as usize + string_offset;
        let data = table.get(str_off..str_off.saturating_add(length)).unwrap_or(&[]).to_vec();
        records.push(NameRecord {
            platform_id,
            encoding_id,
            language_id,
            name_id,
            data,
        });
    }

    for r in records {
        out.entry(r.name_id).or_default().push((r.platform_id, r.encoding_id, r.language_id, r.data));
    }
    out
}

fn get_name_table(raw: &[u8]) -> Result<Vec<u8>, String> {
    get_table(raw, "name").map(|t| t.data).ok_or_else(|| "Not a supported font, has no name table".to_string())
}

/// Port of `get_font_names`: `(family_name, subfamily_name, full_name)`.
pub fn get_font_names(raw: &[u8]) -> Result<(Option<String>, Option<String>, Option<String>), String> {
    let table = get_name_table(raw)?;
    let records = parse_name_records(&table);
    Ok((
        records.get(&1).and_then(|r| decode_name_record(r)),
        records.get(&2).and_then(|r| decode_name_record(r)),
        records.get(&4).and_then(|r| decode_name_record(r)),
    ))
}

/// The 7-field shape `get_font_names2` (and real upstream's own
/// `get_font_names_from_ttlib_names_table`) returns.
#[derive(Debug, Clone, Default)]
pub struct ExtendedFontNames {
    pub family_name: Option<String>,
    pub subfamily_name: Option<String>,
    pub full_name: Option<String>,
    pub preferred_family_name: Option<String>,
    pub preferred_subfamily_name: Option<String>,
    pub wws_family_name: Option<String>,
    pub wws_subfamily_name: Option<String>,
}

/// Port of `get_font_names2`.
pub fn get_font_names2(raw: &[u8]) -> Result<ExtendedFontNames, String> {
    let table = get_name_table(raw)?;
    let records = parse_name_records(&table);
    let get = |id: u16| records.get(&id).and_then(|r| decode_name_record(r));
    Ok(ExtendedFontNames {
        family_name: get(1),
        subfamily_name: get(2),
        full_name: get(4),
        preferred_family_name: get(16),
        preferred_subfamily_name: get(17),
        wws_family_name: get(21),
        wws_subfamily_name: get(22),
    })
}

/// Port of `get_all_font_names`.
pub fn get_all_font_names(raw: &[u8]) -> Result<HashMap<String, String>, String> {
    get_all_font_names_from_table(&get_name_table(raw)?)
}

/// Port of `get_all_font_names(raw_is_table=True)`: `raw` is already an
/// extracted `name` table's own bytes (as
/// [`crate::fonts::sfnt::container::Sfnt::get`] returns), not a whole
/// font -- so the "find the name table within a full font" step
/// [`get_all_font_names`] does is skipped.
pub fn get_all_font_names_from_table(table: &[u8]) -> Result<HashMap<String, String>, String> {
    let records = parse_name_records(table);
    let mut ans = HashMap::new();

    for (name, id) in [
        ("family_name", 1u16),
        ("subfamily_name", 2),
        ("full_name", 4),
        ("preferred_family_name", 16),
        ("preferred_subfamily_name", 17),
        ("wws_family_name", 21),
        ("wws_subfamily_name", 22),
    ] {
        if let Some(recs) = records.get(&id) {
            if let Some(v) = decode_name_record(recs) {
                if !v.is_empty() {
                    ans.insert(name.to_string(), v);
                }
            }
        }
    }

    if let Some(recs) = records.get(&6) {
        for (platform_id, encoding_id, language_id, src) in recs {
            if (*platform_id, *encoding_id, *language_id) == (1, 0, 0) {
                if let Ok(s) = String::from_utf8(src.clone()) {
                    ans.insert("postscript_name".to_string(), s);
                    break;
                }
            } else if (*platform_id, *encoding_id, *language_id) == (3, 1, 1033) {
                if let Some(s) = decode_utf16_be(src) {
                    ans.insert("postscript_name".to_string(), s);
                    break;
                }
            }
        }
    }

    Ok(ans)
}

/// Port of `checksum_of_block`. See the module doc for the disclosed
/// (behaviorally identical) padding simplification.
pub fn checksum_of_block(raw: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for chunk in raw.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// Port of `verify_checksums`.
pub fn verify_checksums(raw: &[u8]) -> Result<(), String> {
    let mut head: Option<(Vec<u8>, usize, u32)> = None;
    for t in get_tables(raw) {
        if t.tag.eq_ignore_ascii_case(b"head") {
            head = Some((t.data, t.table_offset, t.checksum));
        } else if checksum_of_block(&t.data) != t.checksum {
            return Err(format!("The {:?} table has an incorrect checksum", String::from_utf8_lossy(&t.tag)));
        }
    }
    let Some((mut table, table_offset, checksum)) = head else {
        return Ok(());
    };
    if table.len() < 12 {
        return Err("head table is too short".to_string());
    }
    let checksum_adj = u32::from_be_bytes(table[8..12].try_into().unwrap());
    table[8..12].copy_from_slice(&0u32.to_be_bytes());
    if checksum_of_block(&table) != checksum {
        return Err("Checksum of head table not correct".to_string());
    }
    let mut patched = raw.to_vec();
    let end = (table_offset + table.len()).min(patched.len());
    patched[table_offset..end].copy_from_slice(&table[..end - table_offset]);
    let full_checksum = checksum_of_block(&patched);
    let q = 0xB1B0AFBAu32.wrapping_sub(full_checksum);
    if q != checksum_adj {
        return Err("Checksum of entire font incorrect".to_string());
    }
    Ok(())
}

/// Port of `set_checksum_adjustment`, operating on a byte buffer
/// directly rather than a Python `BytesIO` stream.
pub fn set_checksum_adjustment(buf: &mut [u8]) -> Result<(), String> {
    let head = get_table(buf, "head").ok_or("Not a supported font, has no head table")?;
    let offset = head.table_offset + 8;
    if offset + 4 > buf.len() {
        return Err("head table is too short".to_string());
    }
    buf[offset..offset + 4].copy_from_slice(&0u32.to_be_bytes());
    let checksum = checksum_of_block(buf);
    let q = 0xB1B0AFBAu32.wrapping_sub(checksum);
    buf[offset..offset + 4].copy_from_slice(&q.to_be_bytes());
    Ok(())
}

/// Port of `set_table_checksum`.
pub fn set_table_checksum(buf: &mut [u8], name: &str) -> Result<(), String> {
    let t = get_table(buf, name).ok_or_else(|| format!("Not a supported font, has no {name} table"))?;
    let checksum = checksum_of_block(&t.data);
    if checksum != t.checksum {
        let offset = t.record_offset + 4;
        buf[offset..offset + 4].copy_from_slice(&checksum.to_be_bytes());
    }
    Ok(())
}

/// Port of `remove_embed_restriction`.
pub fn remove_embed_restriction(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (ok, sig) = is_truetype_font(raw);
    if !ok {
        return Err(format!("Not a supported font, sfnt_version: {sig:?}"));
    }
    let os2 = get_table(raw, "os/2").ok_or("Not a supported font, has no OS/2 table")?;
    let fs_type_offset = 8; // '>HhHH' = 2+2+2+2
    let fs_type = u16::from_be_bytes(os2.data.get(fs_type_offset..fs_type_offset + 2).ok_or("OS/2 table is too short")?.try_into().unwrap());
    if fs_type == 0 {
        return Ok(raw.to_vec());
    }
    let mut buf = raw.to_vec();
    let abs_offset = os2.table_offset + fs_type_offset;
    buf[abs_offset..abs_offset + 2].copy_from_slice(&0u16.to_be_bytes());
    set_table_checksum(&mut buf, "os/2")?;
    set_checksum_adjustment(&mut buf)?;
    verify_checksums(&buf)?;
    Ok(buf)
}

/// Port of `is_font_embeddable`.
pub fn is_font_embeddable(raw: &[u8]) -> Result<(bool, u16), String> {
    let (ok, sig) = is_truetype_font(raw);
    if !ok {
        return Err(format!("Not a supported font, sfnt_version: {sig:?}"));
    }
    let os2 = get_table(raw, "os/2").ok_or("Not a supported font, has no OS/2 table")?;
    let fs_type_offset = 8;
    let fs_type = u16::from_be_bytes(os2.data.get(fs_type_offset..fs_type_offset + 2).ok_or("OS/2 table is too short")?.try_into().unwrap());
    if fs_type == 0 || fs_type & 0x8 != 0 {
        return Ok((true, fs_type));
    }
    if fs_type & 1 != 0 {
        return Ok((false, fs_type));
    }
    if fs_type & 0x200 != 0 {
        return Ok((false, fs_type));
    }
    Ok((true, fs_type))
}

/// Port of `get_printable_characters`. See the module doc's disclosed
/// narrowing (stdlib control/whitespace filtering only, no combining-
/// mark filtering).
pub fn get_printable_characters(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect()
}

fn read_u16_array(table: &[u8], offset: usize, n: usize) -> Result<Vec<u16>, String> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b = table.get(offset + i * 2..offset + i * 2 + 2).ok_or("truncated cmap subtable")?;
        out.push(u16::from_be_bytes(b.try_into().unwrap()));
    }
    Ok(out)
}

/// Port of `read_bmp_prefix`: the parsed pieces of a format-4 cmap
/// subtable. `pub(crate)` (rather than fully private) so
/// [`crate::fonts::sfnt::cmap::BmpTable`] (issue #551) can parse once
/// and reuse the same fields, instead of re-parsing raw bytes on every
/// lookup the way [`get_bmp_glyph_ids`] does.
#[derive(Debug)]
pub(crate) struct BmpPrefix {
    pub(crate) start_count: Vec<u16>,
    pub(crate) end_count: Vec<u16>,
    pub(crate) range_offset: Vec<u16>,
    pub(crate) id_delta: Vec<i16>,
    pub(crate) glyph_id_map: Vec<u16>,
    pub(crate) array_len: usize,
}

pub(crate) fn read_bmp_prefix(table: &[u8], bmp: usize) -> Result<BmpPrefix, String> {
    let base = bmp + 2;
    let length = u16::from_be_bytes(table.get(base..base + 2).ok_or("truncated cmap subtable")?.try_into().unwrap()) as usize;
    let segcount = u16::from_be_bytes(table.get(base + 4..base + 6).ok_or("truncated cmap subtable")?.try_into().unwrap()) as usize;
    let array_len = segcount / 2;
    let mut offset = bmp + 7 * 2;
    let array_sz = 2 * array_len;

    let end_count = read_u16_array(table, offset, array_len)?;
    offset += array_sz + 2;
    let start_count = read_u16_array(table, offset, array_len)?;
    offset += array_sz;
    let id_delta: Vec<i16> = read_u16_array(table, offset, array_len)?.into_iter().map(|v| v as i16).collect();
    offset += array_sz;
    let range_offset = read_u16_array(table, offset, array_len)?;
    if length + bmp < offset + array_sz {
        return Err("cmap subtable length is too small".to_string());
    }
    let glyph_id_len = (length + bmp - (offset + array_sz)) / 2;
    let glyph_id_map = read_u16_array(table, offset + array_sz, glyph_id_len)?;

    Ok(BmpPrefix {
        start_count,
        end_count,
        range_offset,
        id_delta,
        glyph_id_map,
        array_len,
    })
}

/// Port of `BMPTable.get_glyph_ids`'s real per-code segment lookup.
/// `pub(crate)` so [`crate::fonts::sfnt::cmap::BmpTable`] (issue #551)
/// can reuse it against an already-parsed [`BmpPrefix`] rather than
/// re-deriving the same segment-walk logic a second time.
pub(crate) fn bmp_prefix_glyph_ids(p: &BmpPrefix, codes: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut out = Vec::new();
    for code in codes {
        let mut found = false;
        for (i, &ec) in p.end_count.iter().enumerate() {
            if ec as u32 >= code {
                let sc = p.start_count[i];
                if sc as u32 <= code {
                    found = true;
                    let ro = p.range_offset[i];
                    let glyph_id: i64 = if ro == 0 {
                        p.id_delta[i] as i64 + code as i64
                    } else {
                        let idx = (ro as usize) / 2 + (code as usize - sc as usize) + i - p.array_len;
                        let mapped = *p.glyph_id_map.get(idx).unwrap_or(&0) as i64;
                        if mapped != 0 {
                            mapped + p.id_delta[i] as i64
                        } else {
                            0
                        }
                    };
                    out.push((glyph_id.rem_euclid(0x10000)) as u32);
                    break;
                }
            }
        }
        if !found {
            out.push(0);
        }
    }
    out
}

/// Port of `get_bmp_glyph_ids`.
pub fn get_bmp_glyph_ids(table: &[u8], bmp: usize, codes: impl Iterator<Item = u32>) -> Result<Vec<u32>, String> {
    let p = read_bmp_prefix(table, bmp)?;
    Ok(bmp_prefix_glyph_ids(&p, codes))
}

/// Port of `get_glyph_ids`.
pub fn get_glyph_ids(raw: &[u8], text: &str) -> Result<Vec<u32>, String> {
    let table = get_table(raw, "cmap").ok_or("Not a supported font, has no cmap table")?.data;
    let num_tables = u16::from_be_bytes(table.get(2..4).ok_or("truncated cmap table")?.try_into().unwrap()) as usize;
    let mut bmp_table = None;
    for i in 0..num_tables {
        let base = 4 + i * 8;
        let Some(bytes) = table.get(base..base + 8) else { break };
        let platform_id = u16::from_be_bytes(bytes[0..2].try_into().unwrap());
        let encoding_id = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
        let offset = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if platform_id == 3 && encoding_id == 1 {
            let table_format = u16::from_be_bytes(table.get(offset..offset + 2).ok_or("truncated cmap subtable")?.try_into().unwrap());
            if table_format == 4 {
                bmp_table = Some(offset);
                break;
            }
        }
    }
    let bmp_table = bmp_table.ok_or("Not a supported font, has no format 4 cmap table")?;
    get_bmp_glyph_ids(&table, bmp_table, text.chars().map(|c| c as u32))
}

/// Port of `supports_text`.
pub fn supports_text(raw: &[u8], text: &str, has_only_printable_chars: bool) -> bool {
    let text = if has_only_printable_chars { text.to_string() } else { get_printable_characters(text) };
    match get_glyph_ids(raw, &text) {
        Ok(ids) => ids.iter().all(|&id| id != 0),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal, real, parseable sfnt file with exactly the
    /// tables a test needs -- same "hand-craft a real binary fixture"
    /// technique this project uses for other binary formats (MOBI/
    /// DjVu/ZIP), rather than depending on an external font file being
    /// present at test time.
    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // sfnt version
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // searchRange/entrySelector/rangeShift, unused by this port

        let header_len = 12 + tables.len() * 16;
        let mut data_section = Vec::new();
        let mut records = Vec::new();
        let mut offset = header_len;
        for (tag, data) in tables {
            let checksum = checksum_of_block(data);
            records.push((**tag, checksum, offset, data.len()));
            data_section.extend_from_slice(data);
            // Pad each table to a 4-byte boundary, matching real sfnt layout.
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

    fn name_record(platform_id: u16, encoding_id: u16, language_id: u16, name_id: u16, text: &[u8]) -> (u16, u16, u16, u16, Vec<u8>) {
        (platform_id, encoding_id, language_id, name_id, text.to_vec())
    }

    fn build_name_table(records: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0u16.to_be_bytes()); // format 0
        header.extend_from_slice(&(records.len() as u16).to_be_bytes());
        let string_storage_offset = 6 + records.len() * 12;
        header.extend_from_slice(&(string_storage_offset as u16).to_be_bytes());

        let mut string_storage = Vec::new();
        let mut record_entries = Vec::new();
        for (platform_id, encoding_id, language_id, name_id, text) in records {
            let str_offset = string_storage.len();
            string_storage.extend_from_slice(text);
            record_entries.extend_from_slice(&platform_id.to_be_bytes());
            record_entries.extend_from_slice(&encoding_id.to_be_bytes());
            record_entries.extend_from_slice(&language_id.to_be_bytes());
            record_entries.extend_from_slice(&name_id.to_be_bytes());
            record_entries.extend_from_slice(&(text.len() as u16).to_be_bytes());
            record_entries.extend_from_slice(&(str_offset as u16).to_be_bytes());
        }

        let mut out = header;
        out.extend_from_slice(&record_entries);
        out.extend_from_slice(&string_storage);
        out
    }

    fn utf16_be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    fn build_os2_table(fs_type: u16, weight: u16, selection: u16, panose: [u8; 10]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&0i16.to_be_bytes()); // char_width
        out.extend_from_slice(&weight.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // width
        out.extend_from_slice(&fs_type.to_be_bytes());
        for _ in 0..11 {
            out.extend_from_slice(&0i16.to_be_bytes()); // the 11 'h' fields
        }
        out.extend_from_slice(&panose);
        out.extend_from_slice(&[0u8; 16]); // 4 unicode range u32s
        out.extend_from_slice(&[0u8; 4]); // vendor id
        out.extend_from_slice(&selection.to_be_bytes());
        out
    }

    fn build_head_table() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1.0f32.to_be_bytes()); // version (fixed-point, close enough for byte layout)
        out.extend_from_slice(&1.0f32.to_be_bytes()); // fontRevision
        out.extend_from_slice(&0u32.to_be_bytes()); // checksumAdjustment (patched later)
        out
    }

    #[test]
    fn is_truetype_font_recognizes_both_real_signatures() {
        assert_eq!(is_truetype_font(&[0x00, 0x01, 0x00, 0x00, 1, 2]).0, true);
        assert_eq!(is_truetype_font(b"OTTOxxxx").0, true);
        assert_eq!(is_truetype_font(b"NOPE").0, false);
    }

    #[test]
    fn get_tables_finds_a_real_table_by_tag_case_insensitively() {
        let name = build_name_table(&[]);
        let font = build_sfnt(&[(b"name", &name)]);
        let t = get_table(&font, "NAME").expect("should find the name table case-insensitively");
        assert_eq!(&t.tag, b"name");
        assert_eq!(t.data, name);
    }

    #[test]
    fn font_names_are_recovered_from_a_real_windows_name_record() {
        let records = vec![
            name_record(3, 1, 1033, 1, &utf16_be("Test Family")),
            name_record(3, 1, 1033, 2, &utf16_be("Regular")),
            name_record(3, 1, 1033, 4, &utf16_be("Test Family Regular")),
        ];
        let name_table = build_name_table(&records);
        let font = build_sfnt(&[(b"name", &name_table)]);
        let (family, subfamily, full) = get_font_names(&font).unwrap();
        assert_eq!(family.as_deref(), Some("Test Family"));
        assert_eq!(subfamily.as_deref(), Some("Regular"));
        assert_eq!(full.as_deref(), Some("Test Family Regular"));
    }

    #[test]
    fn font_names_fall_back_to_mac_platform_when_no_windows_record_exists() {
        let records = vec![name_record(1, 0, 0, 1, b"Mac Family")];
        let name_table = build_name_table(&records);
        let font = build_sfnt(&[(b"name", &name_table)]);
        let (family, _, _) = get_font_names(&font).unwrap();
        assert_eq!(family.as_deref(), Some("Mac Family"));
    }

    #[test]
    fn get_all_font_names_includes_postscript_name() {
        let records = vec![
            name_record(3, 1, 1033, 1, &utf16_be("Test Family")),
            name_record(1, 0, 0, 6, b"TestFamily-Regular"),
        ];
        let name_table = build_name_table(&records);
        let font = build_sfnt(&[(b"name", &name_table)]);
        let names = get_all_font_names(&font).unwrap();
        assert_eq!(names.get("family_name").map(String::as_str), Some("Test Family"));
        assert_eq!(names.get("postscript_name").map(String::as_str), Some("TestFamily-Regular"));
    }

    #[test]
    fn font_characteristics_reads_weight_and_selection_flags() {
        // is_bold (bit 5) and is_italic (bit 0) set.
        let os2 = build_os2_table(0, 700, 0b0010_0001, [0, 0, 0, 9, 0, 0, 0, 0, 0, 0]);
        let font = build_sfnt(&[(b"OS/2", &os2)]);
        let fc = get_font_characteristics(&font).unwrap();
        assert_eq!(fc.weight, 700);
        assert!(fc.is_bold);
        assert!(fc.is_italic);
        assert!(!fc.is_regular);
        assert_eq!(panose_to_css_generic_family(&fc.panose), "monospace");
    }

    #[test]
    fn panose_to_css_generic_family_covers_every_branch() {
        assert_eq!(panose_to_css_generic_family(&[0, 0, 0, 9]), "monospace");
        assert_eq!(panose_to_css_generic_family(&[3, 0, 0, 0]), "cursive");
        assert_eq!(panose_to_css_generic_family(&[4, 0, 0, 0]), "fantasy");
        assert_eq!(panose_to_css_generic_family(&[0, 11, 0, 0]), "sans-serif");
        assert_eq!(panose_to_css_generic_family(&[0, 12, 0, 0]), "sans-serif");
        assert_eq!(panose_to_css_generic_family(&[0, 0, 0, 0]), "serif");
    }

    #[test]
    fn is_font_embeddable_honors_the_fstype_restriction_bits() {
        let os2_open = build_os2_table(0, 400, 0, [0; 10]);
        let font_open = build_sfnt(&[(b"OS/2", &os2_open)]);
        assert_eq!(is_font_embeddable(&font_open).unwrap().0, true);

        // fsType bit 0 ("restricted license embedding") forbids embedding.
        let os2_no_embed = build_os2_table(0x1, 400, 0, [0; 10]);
        let font_no_embed = build_sfnt(&[(b"OS/2", &os2_no_embed)]);
        assert_eq!(is_font_embeddable(&font_no_embed).unwrap().0, false);

        // fsType bit 3 ("editable embedding") permits it despite other bits.
        let os2_editable = build_os2_table(0x9, 400, 0, [0; 10]);
        let font_editable = build_sfnt(&[(b"OS/2", &os2_editable)]);
        assert_eq!(is_font_embeddable(&font_editable).unwrap().0, true);
    }

    #[test]
    fn remove_embed_restriction_clears_the_fstype_field_and_fixes_checksums() {
        let os2 = build_os2_table(0x1, 400, 0, [0; 10]);
        let head = build_head_table();
        let mut font = build_sfnt(&[(b"OS/2", &os2), (b"head", &head)]);
        set_checksum_adjustment(&mut font).unwrap();
        verify_checksums(&font).expect("the hand-built fixture's checksums should already be internally consistent");

        let fixed = remove_embed_restriction(&font).unwrap();
        let (embeddable, _) = is_font_embeddable(&fixed).unwrap();
        assert!(embeddable, "fsType should have been cleared");
        verify_checksums(&fixed).expect("checksums should still be internally consistent after the fix");
    }

    #[test]
    fn get_glyph_ids_resolves_a_real_format_4_cmap_segment() {
        // A single-segment format-4 cmap mapping codepoints 'A'..='Z'
        // (0x41..=0x5a) to glyph ids offset by 1 (idDelta), plus the
        // required terminator segment at 0xffff.
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        let seg_count_x2 = 4u16; // 2 segments (A-Z, then the 0xffff terminator)
        // length placeholder, patched below
        let length_pos = sub.len();
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        sub.extend_from_slice(&seg_count_x2.to_be_bytes());
        sub.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // searchRange/entrySelector/rangeShift
        // endCode[]
        sub.extend_from_slice(&0x5au16.to_be_bytes());
        sub.extend_from_slice(&0xffffu16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        // startCode[]
        sub.extend_from_slice(&0x41u16.to_be_bytes());
        sub.extend_from_slice(&0xffffu16.to_be_bytes());
        // idDelta[]
        sub.extend_from_slice(&1i16.to_be_bytes());
        sub.extend_from_slice(&1i16.to_be_bytes());
        // idRangeOffset[] (0 => use idDelta directly)
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        let len = sub.len() as u16;
        sub[length_pos..length_pos + 2].copy_from_slice(&len.to_be_bytes());

        let mut cmap = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes()); // version
        cmap.extend_from_slice(&1u16.to_be_bytes()); // numTables
        cmap.extend_from_slice(&3u16.to_be_bytes()); // platformID (windows)
        cmap.extend_from_slice(&1u16.to_be_bytes()); // encodingID (unicode BMP)
        let subtable_offset = 4 + 8;
        cmap.extend_from_slice(&(subtable_offset as u32).to_be_bytes());
        cmap.extend_from_slice(&sub);

        let font = build_sfnt(&[(b"cmap", &cmap)]);
        let ids = get_glyph_ids(&font, "AB").unwrap();
        assert_eq!(ids, vec![0x41 + 1, 0x42 + 1]);

        assert!(supports_text(&font, "A", true));
        assert!(!supports_text(&font, "!", true), "'!' is outside the mapped A-Z range and should report as unsupported");
    }
}
