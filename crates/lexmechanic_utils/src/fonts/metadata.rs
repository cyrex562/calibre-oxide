//! Port of `calibre.utils.fonts.metadata` (issue #556, split from #63):
//! real, structured font metadata extracted from a font file's `name`
//! and `OS/2` tables.
//!
//! # Disclosed simplification
//!
//! Upstream builds this via `fontTools.subset.load_font`, a full
//! `ttLib` font object. This port uses [`crate::fonts::sfnt::load_font`]
//! (this crate's own real sfnt table reader, from issue #549) directly
//! -- same data, no `ttLib`-object layer to go through.

use super::sfnt::errors::UnsupportedFont;
use super::sfnt::load_font;
use super::utils::{get_font_characteristics, get_font_names2, ExtendedFontNames, FontCharacteristics};

const FONT_STRETCH_NAMES: [&str; 9] = [
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "normal",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];

/// Port of `FontMetadata`. `names`/`characteristics` carry every field
/// upstream's `FontNames`/`FontCharacteristics` namedtuples do (via
/// this crate's own [`ExtendedFontNames`]/[`FontCharacteristics`]
/// structs, from issue #549); `font_family`/`font_weight`/`font_style`/
/// `font_stretch` are the derived CSS-shaped values upstream's own
/// `__init__` computes from them.
#[derive(Debug, Clone)]
pub struct FontMetadata {
    pub is_otf: bool,
    pub names: ExtendedFontNames,
    pub characteristics: FontCharacteristics,
    pub font_family: Option<String>,
    pub font_weight: String,
    pub font_stretch: &'static str,
    pub font_style: &'static str,
}

impl FontMetadata {
    /// Port of `FontMetadata.__init__`.
    pub fn new(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let sfnt = load_font(raw)?;
        let is_otf = sfnt.sfnt_version == *b"OTTO";

        if !sfnt.contains(b"name") {
            return Err(UnsupportedFont("This font has no name table".to_string()));
        }
        let names = get_font_names2(raw).map_err(UnsupportedFont)?;

        if !sfnt.contains(b"OS/2") {
            return Err(UnsupportedFont("This font has no OS/2 table".to_string()));
        }
        let characteristics = get_font_characteristics(raw).map_err(UnsupportedFont)?;

        let font_family = names.family_name.clone();
        let font_weight = match characteristics.weight {
            400 => "normal".to_string(),
            700 => "bold".to_string(),
            other => other.to_string(),
        };
        let width_index = characteristics.width.checked_sub(1).ok_or_else(|| UnsupportedFont("font-stretch (OS/2 usWidthClass) is 0, expected 1-9".to_string()))?;
        let font_stretch = *FONT_STRETCH_NAMES.get(width_index as usize).ok_or_else(|| UnsupportedFont(format!("font-stretch (OS/2 usWidthClass) {} is out of the expected 1-9 range", characteristics.width)))?;
        let font_style = if characteristics.is_oblique {
            "oblique"
        } else if characteristics.is_italic {
            "italic"
        } else {
            "normal"
        };

        Ok(FontMetadata { is_otf, names, characteristics, font_family, font_weight, font_stretch, font_style })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same "hand-craft a real binary fixture" technique (and the same
    // byte-level table formats) as `fonts::utils`'s own test module --
    // duplicated locally rather than shared, matching this crate's
    // existing convention of each sfnt-adjacent test module keeping
    // its own small builder helpers.
    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        let header_len = 12 + tables.len() * 16;
        let mut data_section = Vec::new();
        let mut records = Vec::new();
        let mut offset = header_len;
        for (tag, data) in tables {
            records.push((**tag, 0u32, offset, data.len()));
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

    fn utf16_be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    fn build_name_table(records: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0u16.to_be_bytes());
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

    fn build_os2_table(weight: u16, width: u16, selection: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&0i16.to_be_bytes()); // char_width
        out.extend_from_slice(&weight.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // fs_type
        for _ in 0..11 {
            out.extend_from_slice(&0i16.to_be_bytes());
        }
        out.extend_from_slice(&[0u8; 10]); // panose
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&selection.to_be_bytes());
        out
    }

    fn font(family: &str, subfamily: &str, weight: u16, width: u16, selection: u16) -> Vec<u8> {
        let name = build_name_table(&[
            (3, 1, 1033, 1, utf16_be(family)),
            (3, 1, 1033, 2, utf16_be(subfamily)),
            (3, 1, 1033, 4, utf16_be(&format!("{family} {subfamily}"))),
        ]);
        let os2 = build_os2_table(weight, width, selection);
        build_sfnt(&[(b"name", &name), (b"OS/2", &os2)])
    }

    #[test]
    fn reads_family_weight_style_and_stretch_from_a_real_font() {
        // selection bit 5 = bold.
        let raw = font("Test Family", "Bold", 700, 5, 1 << 5);
        let fm = FontMetadata::new(&raw).unwrap();
        assert_eq!(fm.font_family.as_deref(), Some("Test Family"));
        assert_eq!(fm.font_weight, "bold");
        assert_eq!(fm.font_style, "normal");
        assert_eq!(fm.font_stretch, "normal");
        assert!(!fm.is_otf);
    }

    #[test]
    fn maps_italic_and_oblique_to_font_style() {
        // selection bit 0 = italic.
        let raw = font("It", "Italic", 400, 5, 1 << 0);
        let fm = FontMetadata::new(&raw).unwrap();
        assert_eq!(fm.font_style, "italic");

        // selection bit 9 = oblique, which takes priority over italic.
        let raw = font("Ob", "Oblique", 400, 5, (1 << 0) | (1 << 9));
        let fm = FontMetadata::new(&raw).unwrap();
        assert_eq!(fm.font_style, "oblique");
    }

    #[test]
    fn maps_a_numeric_weight_that_is_not_400_or_700_to_its_own_string() {
        let raw = font("Light", "Light", 300, 5, 0);
        let fm = FontMetadata::new(&raw).unwrap();
        assert_eq!(fm.font_weight, "300");
    }

    #[test]
    fn maps_width_class_to_a_css_font_stretch_keyword() {
        let raw = font("Cond", "Regular", 400, 3, 0);
        let fm = FontMetadata::new(&raw).unwrap();
        assert_eq!(fm.font_stretch, "condensed");
    }

    #[test]
    fn rejects_a_font_with_no_name_or_os2_table() {
        let raw = build_sfnt(&[]);
        assert!(FontMetadata::new(&raw).is_err());
    }

    #[test]
    fn rejects_an_os2_width_class_out_of_the_1_to_9_range() {
        let raw = font("Bad", "Width", 400, 0, 0);
        assert!(FontMetadata::new(&raw).is_err());
    }
}
