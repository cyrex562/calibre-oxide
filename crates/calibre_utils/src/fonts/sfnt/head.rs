//! Port of `calibre.utils.fonts.sfnt.head`'s real `head`-table piece --
//! `HeadTable` (issue #550, split from #64). `HorizontalHeader`/
//! `VerticalHeader` (the `hhea`/`vhea` tables, paired with `hmtx`/
//! `vmtx`) are ported alongside `metrics.py` in #552, since they're
//! metrics tables conceptually even though real upstream happens to
//! define them in this same file. `OS2Table` isn't ported as a
//! separate structured type: this crate's own
//! [`crate::fonts::utils::get_font_characteristics`]/
//! [`crate::fonts::utils::remove_embed_restriction`] (issue #548)
//! already cover real OS/2-table field access and the exact same
//! `fsType`-zeroing operation `OS2Table::zero_fstype` performs.
//! `PostTable` isn't needed by anything this port's `embed_all_fonts`/
//! `subset_all_fonts` (issue #169) call, so it's deferred indefinitely
//! (not blocking anything named in this cluster's split).

use super::errors::UnsupportedFont;
use crate::fonts::utils::Cursor;

/// Port of `HeadTable`. `version_number`/`font_revision` are kept as
/// raw 16.16 fixed-point `i32`s (matching how the underlying bytes are
/// actually stored) with [`HeadTable::version_number_f64`]/
/// [`HeadTable::font_revision_f64`] real-value accessors, rather than
/// porting Python's `FixedProperty` descriptor pattern (which has no
/// direct Rust equivalent) as its own abstraction. `created`/`modified`
/// are likewise kept as raw seconds-since-1904-01-01 `i64`s -- real
/// upstream's own `DateTimeProperty` is a thin `datetime` conversion
/// wrapper around the exact same raw value, not a different quantity.
#[derive(Debug, Clone)]
pub struct HeadTable {
    pub version_number: i32,
    pub font_revision: i32,
    pub checksum_adjustment: u32,
    pub magic_number: u32,
    pub flags: u16,
    pub units_per_em: u16,
    pub created: i64,
    pub modified: i64,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub mac_style: u16,
    pub lowest_rec_ppem: u16,
    pub font_direction_hint: i16,
    pub index_to_loc_format: i16,
    pub glyph_data_format: i16,
}

impl HeadTable {
    /// Port of `HeadTable.__init__`'s real field-unpacking.
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        Ok(HeadTable {
            version_number: c.i32().map_err(UnsupportedFont)?,
            font_revision: c.i32().map_err(UnsupportedFont)?,
            checksum_adjustment: c.u32().map_err(UnsupportedFont)?,
            magic_number: c.u32().map_err(UnsupportedFont)?,
            flags: c.u16().map_err(UnsupportedFont)?,
            units_per_em: c.u16().map_err(UnsupportedFont)?,
            created: c.i64().map_err(UnsupportedFont)?,
            modified: c.i64().map_err(UnsupportedFont)?,
            x_min: c.i16().map_err(UnsupportedFont)?,
            y_min: c.i16().map_err(UnsupportedFont)?,
            x_max: c.i16().map_err(UnsupportedFont)?,
            y_max: c.i16().map_err(UnsupportedFont)?,
            mac_style: c.u16().map_err(UnsupportedFont)?,
            lowest_rec_ppem: c.u16().map_err(UnsupportedFont)?,
            font_direction_hint: c.i16().map_err(UnsupportedFont)?,
            index_to_loc_format: c.i16().map_err(UnsupportedFont)?,
            glyph_data_format: c.i16().map_err(UnsupportedFont)?,
        })
    }

    /// Port of `HeadTable.update`: reserializes every field back to
    /// bytes (used after mutating a field such as
    /// `index_to_loc_format`, e.g. when subsetting rewrites `loca`
    /// into whichever offset width is now smaller).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(54);
        out.extend_from_slice(&self.version_number.to_be_bytes());
        out.extend_from_slice(&self.font_revision.to_be_bytes());
        out.extend_from_slice(&self.checksum_adjustment.to_be_bytes());
        out.extend_from_slice(&self.magic_number.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&self.units_per_em.to_be_bytes());
        out.extend_from_slice(&self.created.to_be_bytes());
        out.extend_from_slice(&self.modified.to_be_bytes());
        out.extend_from_slice(&self.x_min.to_be_bytes());
        out.extend_from_slice(&self.y_min.to_be_bytes());
        out.extend_from_slice(&self.x_max.to_be_bytes());
        out.extend_from_slice(&self.y_max.to_be_bytes());
        out.extend_from_slice(&self.mac_style.to_be_bytes());
        out.extend_from_slice(&self.lowest_rec_ppem.to_be_bytes());
        out.extend_from_slice(&self.font_direction_hint.to_be_bytes());
        out.extend_from_slice(&self.index_to_loc_format.to_be_bytes());
        out.extend_from_slice(&self.glyph_data_format.to_be_bytes());
        out
    }

    /// Port of `FixedProperty`'s real conversion (`val / 0x10000`)
    /// applied to `version_number`.
    pub fn version_number_f64(&self) -> f64 {
        self.version_number as f64 / 65536.0
    }

    /// Port of `FixedProperty`'s real conversion applied to
    /// `font_revision`.
    pub fn font_revision_f64(&self) -> f64 {
        self.font_revision as f64 / 65536.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bytes() -> Vec<u8> {
        let table = HeadTable {
            version_number: 1 << 16,
            font_revision: (1 << 16) + (5 << 12), // 1.3125 in 16.16 fixed-point (approx.)
            checksum_adjustment: 0xdead_beef,
            magic_number: 0x5f0f3cf5,
            flags: 0b11,
            units_per_em: 2048,
            created: 3_000_000_000,
            modified: 3_000_000_100,
            x_min: -100,
            y_min: -50,
            x_max: 1500,
            y_max: 1800,
            mac_style: 0,
            lowest_rec_ppem: 9,
            font_direction_hint: 2,
            index_to_loc_format: 0,
            glyph_data_format: 0,
        };
        table.to_bytes()
    }

    #[test]
    fn parses_and_round_trips_every_field() {
        let raw = sample_bytes();
        let table = HeadTable::parse(&raw).unwrap();
        assert_eq!(table.units_per_em, 2048);
        assert_eq!(table.x_max, 1500);
        assert_eq!(table.index_to_loc_format, 0);
        assert_eq!(table.version_number_f64(), 1.0);
        assert_eq!(table.to_bytes(), raw, "re-serializing an unmodified table should reproduce the exact same bytes");
    }

    #[test]
    fn mutating_index_to_loc_format_and_re_serializing_works() {
        let raw = sample_bytes();
        let mut table = HeadTable::parse(&raw).unwrap();
        table.index_to_loc_format = 1;
        let rebuilt = table.to_bytes();
        let reparsed = HeadTable::parse(&rebuilt).unwrap();
        assert_eq!(reparsed.index_to_loc_format, 1);
    }

    #[test]
    fn rejects_a_truncated_table() {
        let err = HeadTable::parse(&[0u8; 10]).unwrap_err();
        assert!(err.to_string().contains("truncated"), "{err}");
    }
}
