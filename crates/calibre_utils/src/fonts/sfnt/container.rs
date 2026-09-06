//! Port of `calibre.utils.fonts.sfnt.container` (`Sfnt`): the top-level
//! parsed-font object -- a table-tag -> table-bytes map, iterable in
//! sorted tag order, re-serializable back to real sfnt bytes with
//! correct checksums and the `head` table's checksum-adjustment field.
//!
//! # Disclosed narrowings
//!
//! - **The "not `isinstance(raw_or_get_table, bytes)`" constructor
//!   path** (building an `Sfnt` from a `fontTools` `ttLib` font object
//!   via a `raw_or_get_table(tag) -> bytes` getter closure, used only
//!   by `FontMetadata`-adjacent code) is unreachable in this port --
//!   nothing here constructs such an object. Only the real byte-parsing
//!   constructor (`Sfnt(raw: bytes)`) is ported, as [`Sfnt::parse`].
//! - **Per-table structured access** (`TABLE_MAP`'s dispatch to
//!   `HeadTable`/`GlyfTable`/`CmapTable`/etc, one class per real sfnt
//!   table type) isn't ported yet -- those types are separate,
//!   dependency-ordered follow-up issues (#550-555). Until they land,
//!   every table (known or not) is stored and round-tripped as opaque
//!   bytes, exactly matching what real Python's own `UnknownTable`
//!   fallback already does for any tag `TABLE_MAP` doesn't recognize.
//!   This is why [`Sfnt::parse`]/[`Sfnt::to_bytes`] alone are already
//!   enough to support a real round-trip test (parse a font, rebuild
//!   it, confirm the header/checksums match) even before any
//!   specialized table type exists.
//! - **`Sfnt.get_all_font_names`** (a *method* on the real `Sfnt`
//!   class) is not ported: its own body imports `from
//!   calibre.utils.fonts.metadata import FontNames, get_font_names2` --
//!   but `metadata.py` defines no `get_font_names2` function at all
//!   (confirmed by reading the whole file). This is real, unreachable
//!   dead code in upstream itself: every real caller of "get all font
//!   names" anywhere in the codebase (`oeb/polish/check/fonts.py`,
//!   `sfnt/metrics.py`) calls `utils.py`'s own `get_all_font_names`
//!   directly (`calibre_utils::fonts::utils::get_all_font_names`,
//!   already ported in #548), never this container method. Ported
//!   without this specific broken method rather than inventing a fix
//!   for a bug nothing exercises.
//! - **The sfnt-version signature set** real Python checks against
//!   includes a literal 5-byte value, `b'type1'`, compared against a
//!   4-byte slice (`raw[:4]`) -- that comparison can never succeed, so
//!   `type1`-tagged fonts are (probably unintentionally) always
//!   rejected as unsupported in real upstream too. Reproduced as-is:
//!   this port's own signature set only contains the 3 real 4-byte
//!   values that can actually match (`\x00\x01\x00\x00`/`OTTO`/`true`).

use indexmap::IndexMap;

use super::errors::UnsupportedFont;
use crate::fonts::utils::{checksum_of_block, get_tables};

use super::{align_block, max_power_of_two};

/// Real, 4-byte sfnt version signatures `Sfnt::parse` accepts. See the
/// module doc's disclosed narrowing on the dead 5-byte `type1` entry
/// real upstream's own version of this set has.
const VALID_SFNT_VERSIONS: &[[u8; 4]] = &[[0x00, 0x01, 0x00, 0x00], *b"OTTO", *b"true"];

/// Port of `Sfnt`.
#[derive(Debug)]
pub struct Sfnt {
    tables: IndexMap<[u8; 4], Vec<u8>>,
    pub sfnt_version: [u8; 4],
}

impl Sfnt {
    /// Port of `Sfnt.__init__`'s real (bytes-based) constructor path.
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let mut sfnt_version = [0u8; 4];
        let n = raw.len().min(4);
        sfnt_version[..n].copy_from_slice(&raw[..n]);
        if !VALID_SFNT_VERSIONS.contains(&sfnt_version) {
            return Err(UnsupportedFont(format!("Font has unknown sfnt version: {sfnt_version:?}")));
        }
        let mut tables = IndexMap::new();
        for t in get_tables(raw) {
            tables.insert(t.tag, t.data);
        }
        Ok(Sfnt { tables, sfnt_version })
    }

    pub fn get(&self, tag: &[u8; 4]) -> Option<&Vec<u8>> {
        self.tables.get(tag)
    }

    pub fn contains(&self, tag: &[u8; 4]) -> bool {
        self.tables.contains_key(tag)
    }

    pub fn remove(&mut self, tag: &[u8; 4]) -> Option<Vec<u8>> {
        self.tables.shift_remove(tag)
    }

    pub fn insert(&mut self, tag: [u8; 4], data: Vec<u8>) {
        self.tables.insert(tag, data);
    }

    /// Port of `Sfnt.__iter__`: table tags in sorted (not necessarily
    /// optimal-for-loading) order, matching the real OTF spec
    /// recommendation this class's own comment cites.
    pub fn tags(&self) -> Vec<[u8; 4]> {
        let mut tags: Vec<[u8; 4]> = self.tables.keys().copied().collect();
        tags.sort();
        tags
    }

    /// Port of `Sfnt.sizes`.
    pub fn sizes(&self) -> IndexMap<[u8; 4], usize> {
        self.tags().into_iter().map(|tag| (tag, self.tables[&tag].len())).collect()
    }

    /// Port of `Sfnt.__call__`: reserializes every table (in sorted tag
    /// order) into a complete, real sfnt file, with correct per-table
    /// and whole-file checksums and the `head` table's real checksum-
    /// adjustment field. Returns `(bytes, per-tag sizes)`.
    pub fn to_bytes(&self) -> Result<(Vec<u8>, IndexMap<[u8; 4], usize>), UnsupportedFont> {
        let tags = self.tags();
        let num_tables = tags.len() as u32;
        let ln2 = max_power_of_two(num_tables);
        let srange = (1u32 << ln2) * 16;

        let mut out = Vec::new();
        out.extend_from_slice(&self.sfnt_version);
        out.extend_from_slice(&(num_tables as u16).to_be_bytes());
        out.extend_from_slice(&(srange as u16).to_be_bytes());
        out.extend_from_slice(&(ln2 as u16).to_be_bytes());
        out.extend_from_slice(&((num_tables * 16).wrapping_sub(srange) as u16).to_be_bytes());

        let mut head_offset: Option<usize> = None;
        let mut table_data: Vec<Vec<u8>> = Vec::new();
        let mut directory: Vec<([u8; 4], u32, usize, usize)> = Vec::new(); // (tag, checksum, offset, real_len)
        let mut offset = out.len() + (16 * tags.len());
        let mut sizes = IndexMap::new();

        for tag in &tags {
            let raw = &self.tables[tag];
            let table_len = raw.len();
            let mut raw = raw.clone();
            if tag == b"head" {
                head_offset = Some(offset);
                if raw.len() < 12 {
                    return Err(UnsupportedFont("head table is too short".to_string()));
                }
                raw[8..12].copy_from_slice(&[0, 0, 0, 0]);
            }
            let raw = align_block(&raw);
            let checksum = checksum_of_block(&raw);
            directory.push((*tag, checksum, offset, table_len));
            offset += raw.len();
            table_data.push(raw);
            sizes.insert(*tag, table_len);
        }

        for (tag, checksum, table_offset, table_len) in &directory {
            out.extend_from_slice(tag);
            out.extend_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(&(*table_offset as u32).to_be_bytes());
            out.extend_from_slice(&(*table_len as u32).to_be_bytes());
        }

        for data in &table_data {
            out.extend_from_slice(data);
        }

        let Some(head_offset) = head_offset else {
            return Err(UnsupportedFont("This font has no head table".to_string()));
        };
        let checksum = checksum_of_block(&out);
        let q = 0xB1B0AFBAu32.wrapping_sub(checksum);
        out[head_offset + 8..head_offset + 12].copy_from_slice(&q.to_be_bytes());

        Ok((out, sizes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let num_tables = tables.len() as u32;
        let ln2 = max_power_of_two(num_tables);
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
    fn rejects_an_unrecognized_sfnt_version() {
        let err = Sfnt::parse(b"BAD!").unwrap_err();
        assert!(err.to_string().contains("unknown sfnt version"), "{err}");
    }

    #[test]
    fn parses_every_table_and_iterates_tags_sorted() {
        let head = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let name = vec![0u8; 8];
        let font = build_sfnt(&[(b"name", &name), (b"head", &head)]);
        let sfnt = Sfnt::parse(&font).unwrap();
        assert_eq!(sfnt.tags(), vec![*b"head", *b"name"], "tags should iterate in sorted order");
        assert_eq!(sfnt.get(b"head").unwrap(), &head);
        assert!(sfnt.contains(b"name"));
        assert!(!sfnt.contains(b"glyf"));
    }

    #[test]
    fn round_trips_a_real_font_preserving_size_and_checksums() {
        let head = {
            let mut h = vec![0u8; 12];
            h[0..4].copy_from_slice(&1.0f32.to_be_bytes());
            h
        };
        let name = b"hello world!".to_vec();
        let font = build_sfnt(&[(b"head", &head), (b"name", &name)]);

        let sfnt = Sfnt::parse(&font).unwrap();
        let (rebuilt, sizes) = sfnt.to_bytes().unwrap();

        assert_eq!(&font[..12], &rebuilt[..12], "the sfnt header should round-trip identically");
        assert_eq!(font.len(), rebuilt.len(), "size should be preserved for already-4-byte-aligned tables");
        assert_eq!(sizes[b"name"], name.len());

        crate::fonts::utils::verify_checksums(&rebuilt).expect("the rebuilt font's checksums should be real and internally consistent");
    }

    #[test]
    fn removing_and_inserting_a_table_is_reflected_in_the_rebuilt_font() {
        let head = vec![0u8; 12];
        let name = vec![1u8; 4];
        let font = build_sfnt(&[(b"head", &head), (b"name", &name)]);
        let mut sfnt = Sfnt::parse(&font).unwrap();

        assert_eq!(sfnt.remove(b"name"), Some(vec![1u8; 4]));
        assert!(!sfnt.contains(b"name"));
        sfnt.insert(*b"OS/2", vec![9u8; 4]);

        let (rebuilt, sizes) = sfnt.to_bytes().unwrap();
        assert!(sizes.contains_key(b"OS/2"));
        assert!(!sizes.contains_key(b"name"));
        let rebuilt_sfnt = Sfnt::parse(&rebuilt).unwrap();
        assert_eq!(rebuilt_sfnt.get(b"OS/2").unwrap(), &vec![9u8; 4]);
    }
}
