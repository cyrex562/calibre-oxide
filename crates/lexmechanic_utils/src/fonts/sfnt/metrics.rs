//! Port of `calibre.utils.fonts.sfnt.metrics` (`FontMetrics`, plus the
//! real `read_metrics`/`update_metrics_table` free functions), and --
//! per the scope correction recorded when #550 ported `head.py`'s
//! `HeadTable` -- `HorizontalHeader`/`VerticalHeader` (the `hhea`/
//! `vhea` tables real upstream happens to define in `head.py`, but
//! which are metrics tables conceptually) plus `OS2Table`/`PostTable`
//! (real upstream also defines these in `head.py`; #548's own
//! `get_font_characteristics` only covers the narrower fsType-related
//! field set that function needs, not the full OS/2 field set
//! `FontMetrics` needs -- so a proper `Os2Table` is ported here).
//!
//! Issue #552.
//!
//! # Disclosed narrowings
//!
//! - Real `Sfnt.TABLE_MAP`-based automatic table-tag -> typed-object
//!   dispatch isn't wired up here (that's a `container.rs`-level
//!   change, separate scope). [`FontMetrics::parse`] instead pulls
//!   each named table's raw bytes directly via
//!   [`super::container::Sfnt::get`] and constructs each specialized
//!   type itself, matching how every other table type ported so far
//!   in this cluster (#550/#551) already takes raw bytes rather than a
//!   `Sfnt` object.
//! - Real `FontMetrics.__hash__` returns Python's process-randomized
//!   builtin `hash()` of the `name` table's raw bytes -- not a stable
//!   value across runs even in real Python (hash randomization), so
//!   there is nothing meaningful to reproduce. [`FontMetrics::signature`]
//!   instead returns a real deterministic checksum
//!   ([`crate::fonts::utils::checksum_of_block`]) of the same bytes --
//!   same "content identity" purpose, but actually reproducible.
//! - Real `FontMetrics.postscript_name`/`advance_widths` can raise
//!   `KeyError` (family_name missing, or a character with no cmap
//!   entry) -- ported as `Option`/`Result` respectively rather than a
//!   Rust panic.
//! - Real `OS2Table.read_data`'s `hasattr(self, 'char_width')` re-parse
//!   guard checks a field name (`char_width`) that is never actually
//!   set anywhere in the real field list (the real field is named
//!   `average_char_width`) -- the guard is dead code that never
//!   short-circuits. [`Os2Table::parse`] always parses fully, matching
//!   the real *observable* behavior (this guard never fires in
//!   upstream either).

use std::collections::{BTreeMap, HashMap};

use super::container::Sfnt;
use super::errors::UnsupportedFont;
use super::head::HeadTable;
use super::maxp::MaxpTable;
use super::cmap::CmapTable;
use crate::fonts::utils::{checksum_of_block, get_all_font_names_from_table, Cursor};

/// Port of `read_metrics`: `hmtx`/`vmtx`-style `(advance, bearing)`
/// pairs for `num_of_metrics` glyphs, plus any trailing bearing-only
/// entries for glyphs beyond `num_of_metrics` (which all reuse the
/// last advance width). Ported directly from the per-field byte
/// layout rather than replicating Python's own "read the same bytes
/// twice, once as an unsigned array and once as a signed array"
/// approach (which exists only because Python's `array` module needs
/// one element type per array).
pub fn read_metrics(raw: &[u8], num_of_metrics: usize, num_of_glyphs: usize, table_name: &str) -> Result<(Vec<u16>, Vec<i16>), UnsupportedFont> {
    let rawsz = 4 * num_of_metrics;
    if raw.len() < rawsz {
        return Err(UnsupportedFont(format!("The {table_name} table has insufficient data")));
    }
    let mut advances = Vec::with_capacity(num_of_metrics);
    let mut bearings = Vec::with_capacity(num_of_metrics);
    for chunk in raw[..rawsz].chunks_exact(4) {
        advances.push(u16::from_be_bytes([chunk[0], chunk[1]]));
        bearings.push(i16::from_be_bytes([chunk[2], chunk[3]]));
    }
    if num_of_glyphs > num_of_metrics {
        let extra = num_of_glyphs - num_of_metrics;
        let rest = &raw[rawsz..];
        let rawsz2 = 2 * extra;
        if rest.len() < rawsz2 {
            return Err(UnsupportedFont(format!("The {table_name} table has insufficient data for trailing bearings")));
        }
        for chunk in rest[..rawsz2].chunks_exact(2) {
            bearings.push(i16::from_be_bytes([chunk[0], chunk[1]]));
        }
    }
    Ok((advances, bearings))
}

/// Port of `update_metrics_table`: rebuilds an `hmtx`/`vmtx`-style
/// table's raw bytes from `metrics_map` (glyph id -> `(advance,
/// bearing)`, already sorted by glyph id via `BTreeMap`). Returns the
/// per-glyph advance/bearing arrays alongside the new raw bytes --
/// real Python instead mutates the passed-in `mtx_table` object as a
/// side effect; this port returns the bytes so the caller decides
/// where they go, matching this cluster's established "return new
/// bytes rather than mutate a passed table object" convention.
pub fn update_metrics_table(metrics_map: &BTreeMap<usize, (u16, i16)>) -> (Vec<u16>, Vec<i16>, Vec<u8>) {
    let mut aw = Vec::with_capacity(metrics_map.len());
    let mut b = Vec::with_capacity(metrics_map.len());
    let mut raw = Vec::with_capacity(metrics_map.len() * 4);
    for &(adv, bearing) in metrics_map.values() {
        aw.push(adv);
        b.push(bearing);
        raw.extend_from_slice(&adv.to_be_bytes());
        raw.extend_from_slice(&bearing.to_be_bytes());
    }
    (aw, b, raw)
}

/// Port of `HorizontalHeader` (the `hhea` table).
#[derive(Debug, Clone)]
pub struct HorizontalHeader {
    pub version_number: i32,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub advance_width_max: u16,
    pub min_left_side_bearing: i16,
    pub min_right_side_bearing: i16,
    pub x_max_extent: i16,
    pub caret_slope_rise: i16,
    pub caret_slope_run: i16,
    pub caret_offset: i16,
    pub r1: i16,
    pub r2: i16,
    pub r3: i16,
    pub r4: i16,
    pub metric_data_format: i16,
    pub number_of_h_metrics: u16,
    pub advance_widths: Vec<u16>,
    pub left_side_bearings: Vec<i16>,
}

impl HorizontalHeader {
    /// Port of `HorizontalHeader.read_data`.
    pub fn read_data(raw: &[u8], hmtx_raw: &[u8], num_glyphs: usize) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        let version_number = c.i32().map_err(UnsupportedFont)?;
        let ascender = c.i16().map_err(UnsupportedFont)?;
        let descender = c.i16().map_err(UnsupportedFont)?;
        let line_gap = c.i16().map_err(UnsupportedFont)?;
        let advance_width_max = c.u16().map_err(UnsupportedFont)?;
        let min_left_side_bearing = c.i16().map_err(UnsupportedFont)?;
        let min_right_side_bearing = c.i16().map_err(UnsupportedFont)?;
        let x_max_extent = c.i16().map_err(UnsupportedFont)?;
        let caret_slope_rise = c.i16().map_err(UnsupportedFont)?;
        let caret_slope_run = c.i16().map_err(UnsupportedFont)?;
        let caret_offset = c.i16().map_err(UnsupportedFont)?;
        let r1 = c.i16().map_err(UnsupportedFont)?;
        let r2 = c.i16().map_err(UnsupportedFont)?;
        let r3 = c.i16().map_err(UnsupportedFont)?;
        let r4 = c.i16().map_err(UnsupportedFont)?;
        let metric_data_format = c.i16().map_err(UnsupportedFont)?;
        let number_of_h_metrics = c.u16().map_err(UnsupportedFont)?;

        let (advance_widths, left_side_bearings) = read_metrics(hmtx_raw, number_of_h_metrics as usize, num_glyphs, "hmtx")?;

        Ok(HorizontalHeader {
            version_number,
            ascender,
            descender,
            line_gap,
            advance_width_max,
            min_left_side_bearing,
            min_right_side_bearing,
            x_max_extent,
            caret_slope_rise,
            caret_slope_run,
            caret_offset,
            r1,
            r2,
            r3,
            r4,
            metric_data_format,
            number_of_h_metrics,
            advance_widths,
            left_side_bearings,
        })
    }

    /// Port of `HorizontalHeader.metrics_for`.
    pub fn metrics_for(&self, glyph_id: usize) -> (u16, i16) {
        let lsb = self.left_side_bearings[glyph_id];
        let idx = if glyph_id >= self.advance_widths.len() { self.advance_widths.len() - 1 } else { glyph_id };
        (self.advance_widths[idx], lsb)
    }

    /// Port of `HorizontalHeader.update`. Returns `(hhea_bytes,
    /// hmtx_bytes)` -- see the module doc for why this returns bytes
    /// rather than mutating a passed `mtx_table` object.
    pub fn update(&mut self, metrics_map: &BTreeMap<usize, (u16, i16)>) -> (Vec<u8>, Vec<u8>) {
        let (aw, b, mtx_raw) = update_metrics_table(metrics_map);
        self.number_of_h_metrics = metrics_map.len() as u16;
        self.advance_width_max = aw.iter().copied().max().unwrap_or(0);
        self.min_left_side_bearing = b.iter().copied().min().unwrap_or(0);
        self.advance_widths = aw;
        self.left_side_bearings = b;
        (self.to_bytes(), mtx_raw)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(36);
        out.extend_from_slice(&self.version_number.to_be_bytes());
        out.extend_from_slice(&self.ascender.to_be_bytes());
        out.extend_from_slice(&self.descender.to_be_bytes());
        out.extend_from_slice(&self.line_gap.to_be_bytes());
        out.extend_from_slice(&self.advance_width_max.to_be_bytes());
        out.extend_from_slice(&self.min_left_side_bearing.to_be_bytes());
        out.extend_from_slice(&self.min_right_side_bearing.to_be_bytes());
        out.extend_from_slice(&self.x_max_extent.to_be_bytes());
        out.extend_from_slice(&self.caret_slope_rise.to_be_bytes());
        out.extend_from_slice(&self.caret_slope_run.to_be_bytes());
        out.extend_from_slice(&self.caret_offset.to_be_bytes());
        out.extend_from_slice(&self.r1.to_be_bytes());
        out.extend_from_slice(&self.r2.to_be_bytes());
        out.extend_from_slice(&self.r3.to_be_bytes());
        out.extend_from_slice(&self.r4.to_be_bytes());
        out.extend_from_slice(&self.metric_data_format.to_be_bytes());
        out.extend_from_slice(&self.number_of_h_metrics.to_be_bytes());
        out
    }
}

/// Port of `VerticalHeader` (the `vhea` table) -- the vertical-layout
/// mirror of [`HorizontalHeader`], pairing with `vmtx` instead of
/// `hmtx`. Not used by [`FontMetrics`] itself (real upstream's
/// `FontMetrics` only reads `hhea`/`hmtx`), ported for completeness
/// since real `head.py` defines it right alongside `HorizontalHeader`.
#[derive(Debug, Clone)]
pub struct VerticalHeader {
    pub version_number: i32,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub advance_height_max: u16,
    pub min_top_side_bearing: i16,
    pub min_bottom_side_bearing: i16,
    pub y_max_extent: i16,
    pub caret_slope_rise: i16,
    pub caret_slope_run: i16,
    pub caret_offset: i16,
    pub r1: i16,
    pub r2: i16,
    pub r3: i16,
    pub r4: i16,
    pub metric_data_format: i16,
    pub number_of_v_metrics: u16,
    pub advance_heights: Vec<u16>,
    pub top_side_bearings: Vec<i16>,
}

impl VerticalHeader {
    /// Port of `VerticalHeader.read_data`.
    pub fn read_data(raw: &[u8], vmtx_raw: &[u8], num_glyphs: usize) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        let version_number = c.i32().map_err(UnsupportedFont)?;
        let ascender = c.i16().map_err(UnsupportedFont)?;
        let descender = c.i16().map_err(UnsupportedFont)?;
        let line_gap = c.i16().map_err(UnsupportedFont)?;
        let advance_height_max = c.u16().map_err(UnsupportedFont)?;
        let min_top_side_bearing = c.i16().map_err(UnsupportedFont)?;
        let min_bottom_side_bearing = c.i16().map_err(UnsupportedFont)?;
        let y_max_extent = c.i16().map_err(UnsupportedFont)?;
        let caret_slope_rise = c.i16().map_err(UnsupportedFont)?;
        let caret_slope_run = c.i16().map_err(UnsupportedFont)?;
        let caret_offset = c.i16().map_err(UnsupportedFont)?;
        let r1 = c.i16().map_err(UnsupportedFont)?;
        let r2 = c.i16().map_err(UnsupportedFont)?;
        let r3 = c.i16().map_err(UnsupportedFont)?;
        let r4 = c.i16().map_err(UnsupportedFont)?;
        let metric_data_format = c.i16().map_err(UnsupportedFont)?;
        let number_of_v_metrics = c.u16().map_err(UnsupportedFont)?;

        let (advance_heights, top_side_bearings) = read_metrics(vmtx_raw, number_of_v_metrics as usize, num_glyphs, "vmtx")?;

        Ok(VerticalHeader {
            version_number,
            ascender,
            descender,
            line_gap,
            advance_height_max,
            min_top_side_bearing,
            min_bottom_side_bearing,
            y_max_extent,
            caret_slope_rise,
            caret_slope_run,
            caret_offset,
            r1,
            r2,
            r3,
            r4,
            metric_data_format,
            number_of_v_metrics,
            advance_heights,
            top_side_bearings,
        })
    }

    /// Port of `VerticalHeader.metrics_for`.
    pub fn metrics_for(&self, glyph_id: usize) -> (u16, i16) {
        let tsb = self.top_side_bearings[glyph_id];
        let idx = if glyph_id >= self.advance_heights.len() { self.advance_heights.len() - 1 } else { glyph_id };
        (self.advance_heights[idx], tsb)
    }

    /// Port of `VerticalHeader.update`. Returns `(vhea_bytes,
    /// vmtx_bytes)`.
    pub fn update(&mut self, metrics_map: &BTreeMap<usize, (u16, i16)>) -> (Vec<u8>, Vec<u8>) {
        let (aw, b, mtx_raw) = update_metrics_table(metrics_map);
        self.number_of_v_metrics = metrics_map.len() as u16;
        self.advance_height_max = aw.iter().copied().max().unwrap_or(0);
        self.min_top_side_bearing = b.iter().copied().min().unwrap_or(0);
        self.advance_heights = aw;
        self.top_side_bearings = b;
        (self.to_bytes(), mtx_raw)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(36);
        out.extend_from_slice(&self.version_number.to_be_bytes());
        out.extend_from_slice(&self.ascender.to_be_bytes());
        out.extend_from_slice(&self.descender.to_be_bytes());
        out.extend_from_slice(&self.line_gap.to_be_bytes());
        out.extend_from_slice(&self.advance_height_max.to_be_bytes());
        out.extend_from_slice(&self.min_top_side_bearing.to_be_bytes());
        out.extend_from_slice(&self.min_bottom_side_bearing.to_be_bytes());
        out.extend_from_slice(&self.y_max_extent.to_be_bytes());
        out.extend_from_slice(&self.caret_slope_rise.to_be_bytes());
        out.extend_from_slice(&self.caret_slope_run.to_be_bytes());
        out.extend_from_slice(&self.caret_offset.to_be_bytes());
        out.extend_from_slice(&self.r1.to_be_bytes());
        out.extend_from_slice(&self.r2.to_be_bytes());
        out.extend_from_slice(&self.r3.to_be_bytes());
        out.extend_from_slice(&self.r4.to_be_bytes());
        out.extend_from_slice(&self.metric_data_format.to_be_bytes());
        out.extend_from_slice(&self.number_of_v_metrics.to_be_bytes());
        out
    }
}

/// The version->1 (`ver > 1`) real `OS/2` table fields.
#[derive(Debug, Clone)]
pub struct Os2TableV1 {
    pub code_page_range: [u8; 8],
    pub x_height: i16,
    pub cap_height: i16,
    pub default_char: u16,
    pub break_char: u16,
    pub max_context: u16,
}

/// Port of `OS2Table` (the real full field set -- see the module doc
/// for why this is separate from #548's narrower
/// `get_font_characteristics`).
#[derive(Debug, Clone)]
pub struct Os2Table {
    pub version: u16,
    pub average_char_width: i16,
    pub weight_class: u16,
    pub width_class: u16,
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
    pub ranges: [u8; 16],
    pub vendor_id: [u8; 4],
    pub selection: u16,
    pub first_char_index: u16,
    pub last_char_index: u16,
    pub typo_ascender: i16,
    pub typo_descender: i16,
    pub typo_line_gap: i16,
    pub win_ascent: u16,
    pub win_descent: u16,
    pub v1: Option<Os2TableV1>,
}

impl Os2Table {
    /// Port of `OS2Table.read_data` (the always-parses-fully real
    /// behavior; see the module doc's disclosed narrowing about the
    /// dead `hasattr(self, 'char_width')` guard).
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        let version = c.u16().map_err(UnsupportedFont)?;
        let average_char_width = c.i16().map_err(UnsupportedFont)?;
        let weight_class = c.u16().map_err(UnsupportedFont)?;
        let width_class = c.u16().map_err(UnsupportedFont)?;
        let fs_type = c.u16().map_err(UnsupportedFont)?;
        let subscript_x_size = c.i16().map_err(UnsupportedFont)?;
        let subscript_y_size = c.i16().map_err(UnsupportedFont)?;
        let subscript_x_offset = c.i16().map_err(UnsupportedFont)?;
        let subscript_y_offset = c.i16().map_err(UnsupportedFont)?;
        let superscript_x_size = c.i16().map_err(UnsupportedFont)?;
        let superscript_y_size = c.i16().map_err(UnsupportedFont)?;
        let superscript_x_offset = c.i16().map_err(UnsupportedFont)?;
        let superscript_y_offset = c.i16().map_err(UnsupportedFont)?;
        let strikeout_size = c.i16().map_err(UnsupportedFont)?;
        let strikeout_position = c.i16().map_err(UnsupportedFont)?;
        let family_class = c.i16().map_err(UnsupportedFont)?;
        let panose: [u8; 10] = c.take(10).map_err(UnsupportedFont)?.try_into().unwrap();
        let ranges: [u8; 16] = c.take(16).map_err(UnsupportedFont)?.try_into().unwrap();
        let vendor_id: [u8; 4] = c.take(4).map_err(UnsupportedFont)?.try_into().unwrap();
        let selection = c.u16().map_err(UnsupportedFont)?;
        let first_char_index = c.u16().map_err(UnsupportedFont)?;
        let last_char_index = c.u16().map_err(UnsupportedFont)?;
        let typo_ascender = c.i16().map_err(UnsupportedFont)?;
        let typo_descender = c.i16().map_err(UnsupportedFont)?;
        let typo_line_gap = c.i16().map_err(UnsupportedFont)?;
        let win_ascent = c.u16().map_err(UnsupportedFont)?;
        let win_descent = c.u16().map_err(UnsupportedFont)?;

        let v1 = if version > 1 {
            let code_page_range: [u8; 8] = c.take(8).map_err(UnsupportedFont)?.try_into().unwrap();
            let x_height = c.i16().map_err(UnsupportedFont)?;
            let cap_height = c.i16().map_err(UnsupportedFont)?;
            let default_char = c.u16().map_err(UnsupportedFont)?;
            let break_char = c.u16().map_err(UnsupportedFont)?;
            let max_context = c.u16().map_err(UnsupportedFont)?;
            Some(Os2TableV1 { code_page_range, x_height, cap_height, default_char, break_char, max_context })
        } else {
            None
        };

        Ok(Os2Table {
            version,
            average_char_width,
            weight_class,
            width_class,
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
            ranges,
            vendor_id,
            selection,
            first_char_index,
            last_char_index,
            typo_ascender,
            typo_descender,
            typo_line_gap,
            win_ascent,
            win_descent,
            v1,
        })
    }

    /// Port of `getattr(self.os2, 'cap_height', self.os2.typo_ascender)`
    /// (`FontMetrics.__init__`'s own real fallback for pre-version-2
    /// `OS/2` tables, which have no `cap_height` field at all).
    pub fn cap_height(&self) -> i16 {
        self.v1.as_ref().map(|v| v.cap_height).unwrap_or(self.typo_ascender)
    }
}

/// Port of `PostTable`.
#[derive(Debug, Clone)]
pub struct PostTable {
    pub version_number: i32,
    italic_angle_raw: i32,
    pub underline_position: i16,
    pub underline_thickness: i16,
}

impl PostTable {
    /// Port of `PostTable.read_data` (always parses fully -- the real
    /// `hasattr(self, 'underline_position')` guard is likewise dead:
    /// `read_data` is only ever called once per real `FontMetrics`
    /// instance in upstream, so it never gets the chance to matter).
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        let version_number = c.i32().map_err(UnsupportedFont)?;
        let italic_angle_raw = c.i32().map_err(UnsupportedFont)?;
        let underline_position = c.i16().map_err(UnsupportedFont)?;
        let underline_thickness = c.i16().map_err(UnsupportedFont)?;
        Ok(PostTable { version_number, italic_angle_raw, underline_position, underline_thickness })
    }

    /// Port of `PostTable.italic_angle` (a `FixedProperty`).
    pub fn italic_angle(&self) -> f64 {
        self.italic_angle_raw as f64 / 65536.0
    }
}

/// Port of `FontMetrics`.
#[derive(Debug)]
pub struct FontMetrics {
    pub ascent: i16,
    pub descent: i16,
    pub bbox: (i16, i16, i16, i16),
    advance_widths_table: Vec<u16>,
    pub cmap: CmapTable,
    pub units_per_em: u16,
    pub os2: Os2Table,
    pub post: PostTable,
    pub names: HashMap<String, String>,
    pub is_otf: bool,
    signature: u32,
    pub pdf_ascent: i64,
    pub pdf_descent: i64,
    pub pdf_bbox: (i64, i64, i64, i64),
    pub pdf_capheight: i64,
    pub pdf_avg_width: i64,
    pub pdf_stemv: i64,
}

const REQUIRED_TABLES: [[u8; 4]; 8] = [*b"head", *b"hhea", *b"hmtx", *b"cmap", *b"OS/2", *b"post", *b"name", *b"maxp"];

impl FontMetrics {
    /// Port of `FontMetrics.__init__`. See the module doc for why this
    /// takes a [`Sfnt`] and pulls raw table bytes directly rather than
    /// relying on a `TABLE_MAP`-style automatic dispatch.
    pub fn parse(sfnt: &Sfnt) -> Result<Self, UnsupportedFont> {
        for tag in REQUIRED_TABLES {
            if !sfnt.contains(&tag) {
                return Err(UnsupportedFont(format!("This font has no {} table", String::from_utf8_lossy(&tag))));
            }
        }

        let head = HeadTable::parse(sfnt.get(b"head").unwrap())?;
        let maxp = MaxpTable::parse(sfnt.get(b"maxp").unwrap())?;
        let hmtx_raw = sfnt.get(b"hmtx").unwrap();
        let hhea = HorizontalHeader::read_data(sfnt.get(b"hhea").unwrap(), hmtx_raw, maxp.num_glyphs as usize)?;
        let ascent = hhea.ascender;
        let descent = hhea.descender;
        let bbox = (head.x_min, head.y_min, head.x_max, head.y_max);
        let advance_widths_table = hhea.advance_widths;
        let cmap = CmapTable::parse(sfnt.get(b"cmap").unwrap().clone())?;
        let units_per_em = head.units_per_em;
        let os2 = Os2Table::parse(sfnt.get(b"OS/2").unwrap())?;
        let post = PostTable::parse(sfnt.get(b"post").unwrap())?;
        let name_raw = sfnt.get(b"name").unwrap();
        let names = get_all_font_names_from_table(name_raw).map_err(UnsupportedFont)?;
        let is_otf = sfnt.contains(b"CFF ");
        let signature = checksum_of_block(name_raw);

        let pdf_scale = |x: i32| -> i64 { (x as f64 * 1000.0 / units_per_em as f64).round() as i64 };
        let pdf_ascent = pdf_scale(os2.typo_ascender as i32);
        let pdf_descent = pdf_scale(os2.typo_descender as i32);
        let pdf_bbox = (pdf_scale(bbox.0 as i32), pdf_scale(bbox.1 as i32), pdf_scale(bbox.2 as i32), pdf_scale(bbox.3 as i32));
        let pdf_capheight = pdf_scale(os2.cap_height() as i32);
        let pdf_avg_width = pdf_scale(os2.average_char_width as i32);
        let pdf_stemv = 50 + (os2.weight_class as f64 / 65.0).powi(2) as i64;

        Ok(FontMetrics {
            ascent,
            descent,
            bbox,
            advance_widths_table,
            cmap,
            units_per_em,
            os2,
            post,
            names,
            is_otf,
            signature,
            pdf_ascent,
            pdf_descent,
            pdf_bbox,
            pdf_capheight,
            pdf_avg_width,
            pdf_stemv,
        })
    }

    /// Port of `FontMetrics.__hash__` -- see the module doc for why
    /// this is a real checksum rather than Python's process-randomized
    /// `hash()`.
    pub fn signature(&self) -> u32 {
        self.signature
    }

    /// Port of `FontMetrics.postscript_name`.
    pub fn postscript_name(&self) -> Option<String> {
        if let Some(v) = self.names.get("postscript_name") {
            return Some(v.replace(' ', "-"));
        }
        if let Some(v) = self.names.get("full_name") {
            return Some(v.replace(' ', "-"));
        }
        self.names.get("family_name").map(|v| v.replace(' ', "-"))
    }

    /// Port of `FontMetrics.underline_thickness`.
    pub fn underline_thickness(&self, pixel_size: f64) -> f64 {
        let yscale = pixel_size / self.units_per_em as f64;
        self.post.underline_thickness as f64 * yscale
    }

    /// Port of `FontMetrics.underline_position`.
    pub fn underline_position(&self, pixel_size: f64) -> f64 {
        let yscale = pixel_size / self.units_per_em as f64;
        self.post.underline_position as f64 * yscale
    }

    /// Port of `FontMetrics.overline_position`.
    pub fn overline_position(&self, pixel_size: f64) -> f64 {
        let yscale = pixel_size / self.units_per_em as f64;
        (self.ascent as f64 + 2.0) * yscale
    }

    /// Port of `FontMetrics.strikeout_size`.
    pub fn strikeout_size(&self, pixel_size: f64) -> f64 {
        let yscale = pixel_size / self.units_per_em as f64;
        yscale * self.os2.strikeout_size as f64
    }

    /// Port of `FontMetrics.strikeout_position`.
    pub fn strikeout_position(&self, pixel_size: f64) -> f64 {
        let yscale = pixel_size / self.units_per_em as f64;
        yscale * self.os2.strikeout_position as f64
    }

    /// Port of `FontMetrics.glyph_widths`.
    pub fn glyph_widths(&self, glyph_ids: impl Iterator<Item = u32>) -> Vec<u16> {
        let last = self.advance_widths_table.len();
        glyph_ids
            .map(|i| {
                let idx = if (i as usize) < last { i as usize } else { last - 1 };
                self.advance_widths_table[idx]
            })
            .collect()
    }

    /// Port of `FontMetrics.advance_widths`. Real Python raises
    /// `KeyError` for a character with no cmap entry (`get_character_map`
    /// only includes mapped characters); ported as an `Err` here rather
    /// than silently substituting a width of zero.
    pub fn advance_widths(&self, string: &str, pixel_size: f64, stretch: f64) -> Result<Vec<f64>, UnsupportedFont> {
        let chars: Vec<u32> = string.chars().map(|c| c as u32).collect();
        let cmap = self.cmap.get_character_map(&chars)?;
        let mut glyph_ids = Vec::with_capacity(chars.len());
        for c in &chars {
            let gid = cmap.get(c).ok_or_else(|| UnsupportedFont(format!("character U+{c:04X} has no glyph in this font's cmap")))?;
            glyph_ids.push(*gid);
        }
        let pixel_size_x = stretch * pixel_size;
        let xscale = pixel_size_x / self.units_per_em as f64;
        Ok(self.glyph_widths(glyph_ids.into_iter()).into_iter().map(|w| w as f64 * xscale).collect())
    }

    /// Port of `FontMetrics.width`.
    pub fn width(&self, string: &str, pixel_size: f64, stretch: f64) -> Result<f64, UnsupportedFont> {
        Ok(self.advance_widths(string, pixel_size, stretch)?.into_iter().sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(1i32 << 16).to_be_bytes()); // version_number 1.0
        out.extend_from_slice(&(0i32).to_be_bytes()); // font_revision
        out.extend_from_slice(&0u32.to_be_bytes()); // checksum_adjustment
        out.extend_from_slice(&0x5f0f3cf5u32.to_be_bytes()); // magic_number
        out.extend_from_slice(&0u16.to_be_bytes()); // flags
        out.extend_from_slice(&1000u16.to_be_bytes()); // units_per_em
        out.extend_from_slice(&0i64.to_be_bytes()); // created
        out.extend_from_slice(&0i64.to_be_bytes()); // modified
        out.extend_from_slice(&(-10i16).to_be_bytes()); // x_min
        out.extend_from_slice(&(-20i16).to_be_bytes()); // y_min
        out.extend_from_slice(&1000i16.to_be_bytes()); // x_max
        out.extend_from_slice(&900i16.to_be_bytes()); // y_max
        out.extend_from_slice(&0u16.to_be_bytes()); // mac_style
        out.extend_from_slice(&9u16.to_be_bytes()); // lowest_rec_ppem
        out.extend_from_slice(&2i16.to_be_bytes()); // font_direction_hint
        out.extend_from_slice(&0i16.to_be_bytes()); // index_to_loc_format
        out.extend_from_slice(&0i16.to_be_bytes()); // glyph_data_format
        out
    }

    fn hhea_bytes(number_of_h_metrics: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(1i32 << 16).to_be_bytes());
        out.extend_from_slice(&800i16.to_be_bytes()); // ascender
        out.extend_from_slice(&(-200i16).to_be_bytes()); // descender
        out.extend_from_slice(&0i16.to_be_bytes()); // line_gap
        out.extend_from_slice(&1000u16.to_be_bytes()); // advance_width_max
        out.extend_from_slice(&0i16.to_be_bytes()); // min_left_side_bearing
        out.extend_from_slice(&0i16.to_be_bytes()); // min_right_side_bearing
        out.extend_from_slice(&0i16.to_be_bytes()); // x_max_extent
        out.extend_from_slice(&1i16.to_be_bytes()); // caret_slope_rise
        out.extend_from_slice(&0i16.to_be_bytes()); // caret_slope_run
        out.extend_from_slice(&0i16.to_be_bytes()); // caret_offset
        out.extend_from_slice(&0i16.to_be_bytes()); // r1
        out.extend_from_slice(&0i16.to_be_bytes()); // r2
        out.extend_from_slice(&0i16.to_be_bytes()); // r3
        out.extend_from_slice(&0i16.to_be_bytes()); // r4
        out.extend_from_slice(&0i16.to_be_bytes()); // metric_data_format
        out.extend_from_slice(&number_of_h_metrics.to_be_bytes());
        out
    }

    fn hmtx_bytes(entries: &[(u16, i16)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(adv, bearing) in entries {
            out.extend_from_slice(&adv.to_be_bytes());
            out.extend_from_slice(&bearing.to_be_bytes());
        }
        out
    }

    #[test]
    fn read_metrics_reads_full_pairs_and_trailing_bearings() {
        let mut raw = hmtx_bytes(&[(500, 10), (600, -5)]);
        raw.extend_from_slice(&(3i16).to_be_bytes()); // trailing bearing-only entry
        let (advances, bearings) = read_metrics(&raw, 2, 3, "hmtx").unwrap();
        assert_eq!(advances, vec![500, 600]);
        assert_eq!(bearings, vec![10, -5, 3]);
    }

    #[test]
    fn read_metrics_rejects_insufficient_data() {
        let raw = hmtx_bytes(&[(500, 10)]);
        let err = read_metrics(&raw, 2, 2, "hmtx").unwrap_err();
        assert!(err.to_string().contains("hmtx"), "{err}");
    }

    #[test]
    fn horizontal_header_read_data_recovers_advance_widths_and_bearings() {
        let raw = hhea_bytes(2);
        let mut hmtx = hmtx_bytes(&[(500, 10), (600, -5)]);
        // 4 trailing bearing-only entries, for glyphs 2..=5 (beyond
        // number_of_h_metrics=2, which is the real reason
        // left_side_bearings can be longer than advance_widths).
        for b in [1i16, 2, 3, 4] {
            hmtx.extend_from_slice(&b.to_be_bytes());
        }
        let hhea = HorizontalHeader::read_data(&raw, &hmtx, 6).unwrap();
        assert_eq!(hhea.ascender, 800);
        assert_eq!(hhea.advance_widths, vec![500, 600]);
        assert_eq!(hhea.left_side_bearings, vec![10, -5, 1, 2, 3, 4]);
        assert_eq!(hhea.metrics_for(1), (600, -5));
        assert_eq!(hhea.metrics_for(5), (600, 4), "a glyph beyond the metrics table should reuse the last advance width, with its own trailing left_side_bearings entry");
    }

    #[test]
    fn horizontal_header_update_rebuilds_metrics_and_recomputes_extrema() {
        let raw = hhea_bytes(2);
        let hmtx = hmtx_bytes(&[(500, 10), (600, -5)]);
        let mut hhea = HorizontalHeader::read_data(&raw, &hmtx, 2).unwrap();
        let mut map = BTreeMap::new();
        map.insert(0usize, (300u16, 2i16));
        map.insert(1usize, (900u16, -1i16));
        let (hhea_bytes, mtx_bytes) = hhea.update(&map);
        assert_eq!(hhea.advance_width_max, 900);
        assert_eq!(hhea.min_left_side_bearing, -1);
        assert_eq!(hhea.number_of_h_metrics, 2);
        let reparsed = HorizontalHeader::read_data(&hhea_bytes, &mtx_bytes, 2).unwrap();
        assert_eq!(reparsed.advance_widths, vec![300, 900]);
    }

    fn os2_bytes_v0() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version 0
        out.extend_from_slice(&550i16.to_be_bytes()); // average_char_width
        out.extend_from_slice(&700u16.to_be_bytes()); // weight_class
        out.extend_from_slice(&5u16.to_be_bytes()); // width_class
        out.extend_from_slice(&0u16.to_be_bytes()); // fs_type
        for _ in 0..8 {
            out.extend_from_slice(&0i16.to_be_bytes()); // subscript/superscript x/y size/offset
        }
        out.extend_from_slice(&50i16.to_be_bytes()); // strikeout_size
        out.extend_from_slice(&300i16.to_be_bytes()); // strikeout_position
        out.extend_from_slice(&0i16.to_be_bytes()); // family_class
        out.extend_from_slice(&[0u8; 10]); // panose
        out.extend_from_slice(&[0u8; 16]); // ranges
        out.extend_from_slice(b"ABCD"); // vendor_id
        out.extend_from_slice(&0u16.to_be_bytes()); // selection
        out.extend_from_slice(&0u16.to_be_bytes()); // first_char_index
        out.extend_from_slice(&0u16.to_be_bytes()); // last_char_index
        out.extend_from_slice(&750i16.to_be_bytes()); // typo_ascender
        out.extend_from_slice(&(-250i16).to_be_bytes()); // typo_descender
        out.extend_from_slice(&0i16.to_be_bytes()); // typo_line_gap
        out.extend_from_slice(&800u16.to_be_bytes()); // win_ascent
        out.extend_from_slice(&200u16.to_be_bytes()); // win_descent
        out
    }

    #[test]
    fn os2_table_v0_has_no_v1_fields_and_falls_back_for_cap_height() {
        let raw = os2_bytes_v0();
        let os2 = Os2Table::parse(&raw).unwrap();
        assert_eq!(os2.weight_class, 700);
        assert!(os2.v1.is_none());
        assert_eq!(os2.cap_height(), os2.typo_ascender, "pre-version-2 OS/2 has no cap_height, should fall back to typo_ascender");
    }

    #[test]
    fn os2_table_v2_reads_the_extra_fields_including_cap_height() {
        let mut raw = os2_bytes_v0();
        raw[0..2].copy_from_slice(&2u16.to_be_bytes()); // bump version to 2
        raw.extend_from_slice(&[0u8; 8]); // code_page_range
        raw.extend_from_slice(&500i16.to_be_bytes()); // x_height
        raw.extend_from_slice(&700i16.to_be_bytes()); // cap_height
        raw.extend_from_slice(&0u16.to_be_bytes()); // default_char
        raw.extend_from_slice(&0u16.to_be_bytes()); // break_char
        raw.extend_from_slice(&1u16.to_be_bytes()); // max_context
        let os2 = Os2Table::parse(&raw).unwrap();
        assert_eq!(os2.cap_height(), 700);
    }

    #[test]
    fn post_table_recovers_italic_angle_and_underline_metrics() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(2i32 << 16).to_be_bytes()); // version 2.0
        raw.extend_from_slice(&(-(5i32) << 16).to_be_bytes()); // italic_angle -5.0
        raw.extend_from_slice(&(-100i16).to_be_bytes()); // underline_position
        raw.extend_from_slice(&50i16.to_be_bytes()); // underline_thickness
        let post = PostTable::parse(&raw).unwrap();
        assert_eq!(post.italic_angle(), -5.0);
        assert_eq!(post.underline_position, -100);
        assert_eq!(post.underline_thickness, 50);
    }

    fn utf16_be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    fn maxp_bytes(num_glyphs: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(0x0000_5000i32).to_be_bytes()); // version 0.5, no v1 fields
        out.extend_from_slice(&num_glyphs.to_be_bytes());
        out
    }

    fn build_name_table(records: &[(u16, u16, u16, u16, &[u8])]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0u16.to_be_bytes());
        header.extend_from_slice(&(records.len() as u16).to_be_bytes());
        let string_storage_offset = 6 + records.len() * 12;
        header.extend_from_slice(&(string_storage_offset as u16).to_be_bytes());

        let mut string_storage = Vec::new();
        let mut record_entries = Vec::new();
        for &(platform_id, encoding_id, language_id, name_id, text) in records {
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

    /// A single-segment format-4 cmap subtable mapping `start..=end`
    /// (Windows BMP, platform 3 encoding 1) to glyph ids offset by
    /// `id_delta`, wrapped in a full `cmap` table directory.
    fn build_cmap_table(start_code: u16, end_code: u16, id_delta: i16) -> Vec<u8> {
        let end_codes = [end_code, 0xffffu16];
        let start_codes = [start_code, 0xffffu16];
        let id_deltas = [id_delta, 1i16];
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
        data.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[0]
        data.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[1]

        let seg_count = 2u16; // start..=end, plus the 0xffff terminator segment
        let length = 14 + data.len();
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        sub.extend_from_slice(&(length as u16).to_be_bytes()); // length
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        sub.extend_from_slice(&(2 * seg_count).to_be_bytes()); // segCountX2
        sub.extend_from_slice(&0u16.to_be_bytes()); // searchRange (unused by the reader)
        sub.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        sub.extend_from_slice(&data);

        let mut raw = Vec::new();
        raw.extend_from_slice(&0u16.to_be_bytes()); // version
        raw.extend_from_slice(&1u16.to_be_bytes()); // num_tables
        raw.extend_from_slice(&3u16.to_be_bytes()); // platform: Windows
        raw.extend_from_slice(&1u16.to_be_bytes()); // encoding: Unicode BMP
        raw.extend_from_slice(&12u32.to_be_bytes()); // offset
        raw.extend_from_slice(&sub);
        raw
    }

    /// Builds a minimal, real, parseable sfnt file wrapping the given
    /// tables -- same technique used throughout this cluster's tests
    /// (see e.g. `container.rs`'s own `build_sfnt`).
    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let num_tables = tables.len() as u32;
        let ln2 = super::super::max_power_of_two(num_tables);
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

    #[test]
    fn font_metrics_parse_assembles_every_sub_table_into_real_pdf_metrics() {
        let head = head_bytes();
        let hhea = hhea_bytes(1);
        let hmtx = hmtx_bytes(&[(500, 12)]);
        let maxp = maxp_bytes(1);
        let cmap = build_cmap_table(0x41, 0x5a, 0); // 'A'..='Z' -> glyph id == code point offset by 0
        let os2 = os2_bytes_v0();
        let mut post = Vec::new();
        post.extend_from_slice(&(2i32 << 16).to_be_bytes());
        post.extend_from_slice(&0i32.to_be_bytes());
        post.extend_from_slice(&(-100i16).to_be_bytes());
        post.extend_from_slice(&50i16.to_be_bytes());
        let family_name_utf16 = utf16_be("Test Family");
        let name = build_name_table(&[(3, 1, 1033, 1, &family_name_utf16)]);

        let font = build_sfnt(&[
            (b"head", &head),
            (b"hhea", &hhea),
            (b"hmtx", &hmtx),
            (b"maxp", &maxp),
            (b"cmap", &cmap),
            (b"OS/2", &os2),
            (b"post", &post),
            (b"name", &name),
        ]);
        let sfnt = Sfnt::parse(&font).unwrap();
        let metrics = FontMetrics::parse(&sfnt).unwrap();

        assert_eq!(metrics.units_per_em, 1000);
        assert_eq!(metrics.ascent, 800);
        assert_eq!(metrics.bbox, (-10, -20, 1000, 900));
        assert!(!metrics.is_otf);
        assert_eq!(metrics.postscript_name().as_deref(), Some("Test-Family"));

        // A single glyph ('A' -> glyph id 0x41) whose only hmtx entry
        // is (advance=500, bearing=12), at units_per_em=1000 -> a
        // 12pt string of just 'A' should advance by 12 * 500/1000 = 6.0.
        let widths = metrics.advance_widths("A", 12.0, 1.0).unwrap();
        assert_eq!(widths, vec![6.0]);
        assert_eq!(metrics.width("A", 12.0, 1.0).unwrap(), 6.0);

        // pdf_scale(x) = round(x * 1000 / units_per_em); units_per_em
        // here is already 1000, so pdf metrics equal the raw OS/2/head
        // values exactly.
        assert_eq!(metrics.pdf_ascent, 750); // os2.typo_ascender
        assert_eq!(metrics.pdf_descent, -250); // os2.typo_descender
        assert_eq!(metrics.pdf_bbox, (-10, -20, 1000, 900));
    }

    #[test]
    fn font_metrics_advance_widths_errors_on_an_unmapped_character() {
        let head = head_bytes();
        let hhea = hhea_bytes(1);
        let hmtx = hmtx_bytes(&[(500, 12)]);
        let maxp = maxp_bytes(1);
        let cmap = build_cmap_table(0x41, 0x5a, 0);
        let os2 = os2_bytes_v0();
        let mut post = Vec::new();
        post.extend_from_slice(&(2i32 << 16).to_be_bytes());
        post.extend_from_slice(&0i32.to_be_bytes());
        post.extend_from_slice(&(-100i16).to_be_bytes());
        post.extend_from_slice(&50i16.to_be_bytes());
        let family_name_utf16 = utf16_be("Test Family");
        let name = build_name_table(&[(3, 1, 1033, 1, &family_name_utf16)]);
        let font = build_sfnt(&[
            (b"head", &head),
            (b"hhea", &hhea),
            (b"hmtx", &hmtx),
            (b"maxp", &maxp),
            (b"cmap", &cmap),
            (b"OS/2", &os2),
            (b"post", &post),
            (b"name", &name),
        ]);
        let sfnt = Sfnt::parse(&font).unwrap();
        let metrics = FontMetrics::parse(&sfnt).unwrap();
        let err = metrics.advance_widths("z", 12.0, 1.0).unwrap_err(); // 'z' (0x7a) is outside 'A'..='Z'
        assert!(err.to_string().contains("cmap"), "{err}");
    }

    #[test]
    fn font_metrics_parse_rejects_a_font_missing_a_required_table() {
        let head = head_bytes();
        let font = build_sfnt(&[(b"head", &head)]);
        let sfnt = Sfnt::parse(&font).unwrap();
        let err = FontMetrics::parse(&sfnt).unwrap_err();
        assert!(err.to_string().contains("table"), "{err}");
    }
}
