//! KF8's native `SKEL`/`CHUNK`/`GUIDE`/`NCX` `INDX` binary index trees.
//!
//! Port of `calibre.ebooks.mobi.writer8.index`.
//!
//! # Relationship to the other two INDX encoders/decoders in this crate
//!
//! [`crate::mobi::index`] (issue #33) *decodes* the on-disk `INDX`/`TAGX`
//! format on the reader side. [`crate::mobi::writer2::indexer`] (issue
//! #34) *encodes* the same wire format for MOBI 6's flavor of index
//! (`IndexEntry`/`PeriodicalIndexEntry`, fixed Rust structs with a
//! `bytestring()` method). This module encodes KF8's flavor, whose
//! entries are Python `dict`s of `name -> [int, ...]` with a *dynamic*
//! tag set per index type (`Index.tag_types`), not a fixed struct --
//! genuinely a different entry shape (`writer2::indexer::IndexEntry` maps
//! one Rust `enum`-tagged struct straight to bytes, `writer8`'s `Index`
//! walks a `TagMeta` table plus a name-keyed value map). Full type-level
//! sharing between the two isn't practical without either forcing
//! `writer2`'s entries into a dynamic dict shape (losing their
//! `Debug`/type-checked construction, deliberately chosen for the MOBI6
//! encoder) or forcing this module's entries into `writer2`'s fixed enum
//! (which has no room for the very different `SkelIndex`/`ChunkIndex`
//! tag sets). What *is* shared, per the issue's fallback guidance: the
//! low-level wire primitives (`encint`, `align_block`, `CNCX`,
//! `encode_number_as_hex`) all come from [`crate::mobi::utils`], used
//! identically by both encoders and by the reader's [`crate::mobi::index`]
//! decoder -- there is exactly one implementation of each of those, not
//! three. The `INDX` header itself is built on the shared
//! [`crate::mobi::writer8::header::Header`] builder ([`IndexHeader`]),
//! rather than the ad hoc byte-pushing `writer2::indexer::Indexer::create_header`
//! uses (that module predates `header.rs`, which this issue adds).

use anyhow::{bail, Result};
use indexmap::IndexMap;

use crate::mobi::utils::{align_block, encint, CNCX};
use crate::mobi::writer8::header::{zeroes, FieldDef, FieldValue, Header, NULL};

/// `TagMeta_` in `index.py`: `(name, number, values_per_entry, bitmask,
/// end_flag)`.
#[derive(Debug, Clone, Copy)]
pub struct TagMeta {
    pub name: &'static str,
    pub number: u8,
    pub values_per_entry: u32,
    pub bitmask: u8,
    pub end_flag: u8,
}

const fn tag(
    name: &'static str,
    number: u8,
    values_per_entry: u32,
    bitmask: u8,
    end_flag: u8,
) -> TagMeta {
    TagMeta {
        name,
        number,
        values_per_entry,
        bitmask,
        end_flag,
    }
}

/// `EndTagTable` in `index.py`.
const END_TAG_TABLE: TagMeta = tag("eof", 0, 0, 0, 1);

/// `mask_to_bit_shifts` in `index.py`.
fn mask_to_bit_shifts(mask: u8) -> u32 {
    match mask {
        1 => 0,
        2 => 1,
        3 => 0,
        4 => 2,
        8 => 3,
        12 => 2,
        16 => 4,
        32 => 5,
        48 => 4,
        64 => 6,
        128 => 7,
        192 => 6,
        _ => 0,
    }
}

/// A single `INDX` entry's tag values, keyed by tag name. Port of the
/// `dict` half of each `(lead_text, tags)` pair in `Index.entries`.
pub type TagValues = IndexMap<&'static str, Vec<u64>>;

/// `IndexHeader` in `index.py`: the `INDX` record whose payload is a
/// `TAGX` block plus a geometry/IDXT table describing the entry records
/// that follow it.
pub const INDEX_HEADER_LENGTH: usize = 192;

fn index_header_fields() -> Vec<FieldDef> {
    vec![
        FieldDef::new("header_length", FieldValue::Int(INDEX_HEADER_LENGTH as u64)),
        FieldDef::new("unknown1", zeroes(8)),
        FieldDef::new("type", FieldValue::Int(2)),
        FieldDef::new("idxt_offset", FieldValue::Int(0)),
        FieldDef::new("num_of_records", FieldValue::Dyn),
        FieldDef::new("encoding", FieldValue::Int(65001)),
        FieldDef::new("unknown2", FieldValue::Int(NULL)),
        FieldDef::new("num_of_entries", FieldValue::Dyn),
        FieldDef::new("ordt_offset", FieldValue::Int(0)),
        FieldDef::new("ligt_offset", FieldValue::Int(0)),
        FieldDef::new("num_of_ordt_entries", FieldValue::Int(0)),
        FieldDef::new("num_of_cncx", FieldValue::Dyn),
        FieldDef::new("unknown3", zeroes(124)),
        FieldDef::new("tagx_offset", FieldValue::Int(INDEX_HEADER_LENGTH as u64)),
        FieldDef::new("unknown4", zeroes(8)),
        FieldDef::new("tagx", FieldValue::Dyn),
        FieldDef::new("geometry", FieldValue::Dyn),
        FieldDef::new("idxt", FieldValue::Dyn),
    ]
}

fn index_header() -> Header {
    Header::new(
        b"INDX",
        true,
        &index_header_fields(),
        &[("idxt_offset", "idxt")],
    )
}

/// Encodes one `Index`'s entries (a `tag_types` table plus an ordered
/// list of `(key, TagValues)` entries) into the header + entry + CNCX
/// records that get appended after a KF8 book's text records. Port of
/// the `Index` base class's `__call__`/`generate_tagx`/
/// `calculate_control_bytes_for_each_entry`.
pub struct IndexBuilder {
    pub tag_types: &'static [TagMeta],
    pub control_byte_count: usize,
}

impl IndexBuilder {
    /// Port of `Index.generate_tagx`.
    pub fn generate_tagx(&self) -> Vec<u8> {
        let mut byts = Vec::new();
        for t in self.tag_types {
            byts.push(t.number);
            byts.push(t.values_per_entry as u8);
            byts.push(t.bitmask);
            byts.push(t.end_flag);
        }
        let mut header = b"TAGX".to_vec();
        header.extend_from_slice(&((12 + byts.len()) as u32).to_be_bytes());
        header.extend_from_slice(&(self.control_byte_count as u32).to_be_bytes());
        header.extend_from_slice(&byts);
        header
    }

    /// Port of `Index.calculate_control_bytes_for_each_entry`. Errors
    /// (rather than raising `ValueError` uncaught) if an entry doesn't
    /// produce exactly `control_byte_count` control bytes.
    fn calculate_control_bytes(&self, entries: &[(String, TagValues)]) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::with_capacity(entries.len());
        for (lead_text, tags) in entries {
            let mut cbs = Vec::new();
            let mut ans: u8 = 0;
            for t in self.tag_types {
                if t.end_flag == 1 {
                    cbs.push(ans);
                    ans = 0;
                    continue;
                }
                let nvals = tags.get(t.name).map(Vec::len).unwrap_or(0);
                let vpe = t.values_per_entry.max(1) as usize;
                let nentries = nvals / vpe;
                let shifts = mask_to_bit_shifts(t.bitmask);
                ans |= t.bitmask & ((nentries as u32) << shifts) as u8;
            }
            if cbs.len() != self.control_byte_count {
                bail!("The entry {lead_text:?} is invalid");
            }
            out.push(cbs);
        }
        Ok(out)
    }

    /// Build the `[header_record, entry_record, ...]` sequence (CNCX
    /// records are *not* appended here -- callers own the `CNCX` and
    /// append its `.records` themselves, matching how
    /// `Index.__call__`'s caller in `main.py` treats `self.records`).
    /// Port of `Index.__call__`, minus the trailing
    /// `self.records.extend(self.cncx.records)` line.
    pub fn build(
        &self,
        entries: &[(String, TagValues)],
        cncx_num_records: usize,
    ) -> Result<Vec<Vec<u8>>> {
        let control_bytes = self.calculate_control_bytes(entries)?;
        let header_length = INDEX_HEADER_LENGTH;
        // kindlegen uses 1048 bytes of margin because of block alignment.
        let record_limit = 0x10000 - header_length - 1048;

        let mut index_blocks: Vec<Vec<u8>> = vec![Vec::new()];
        let mut idxt_blocks: Vec<Vec<u8>> = vec![Vec::new()];
        let mut record_counts: Vec<usize> = vec![0];
        let mut last_indices: Vec<String> = vec![String::new()];

        for (i, (index_num, tags)) in entries.iter().enumerate() {
            let cbs = &control_bytes[i];
            let mut raw = Vec::new();
            let key_bytes = index_num.as_bytes();
            raw.push(key_bytes.len() as u8);
            raw.extend_from_slice(key_bytes);
            raw.extend_from_slice(cbs);
            for t in self.tag_types {
                if t.end_flag == 1 {
                    continue;
                }
                if let Some(values) = tags.get(t.name) {
                    for &v in values {
                        raw.extend(encint(v, true));
                    }
                }
            }

            let offset = index_blocks.last().unwrap().len();
            let idxt_pos = idxt_blocks.last().unwrap().len();
            if offset + idxt_pos + raw.len() + 2 > record_limit {
                index_blocks.push(Vec::new());
                idxt_blocks.push(Vec::new());
                record_counts.push(0);
                last_indices.push(String::new());
            }
            let cur_offset = index_blocks.last().unwrap().len();
            *record_counts.last_mut().unwrap() += 1;
            idxt_blocks
                .last_mut()
                .unwrap()
                .extend_from_slice(&((header_length + cur_offset) as u16).to_be_bytes());
            index_blocks.last_mut().unwrap().extend_from_slice(&raw);
            *last_indices.last_mut().unwrap() = index_num.clone();
        }

        let mut index_records = Vec::new();
        for i in 0..index_blocks.len() {
            let index_block = align_block(&index_blocks[i], 4, 0);
            let mut idxt_full = b"IDXT".to_vec();
            idxt_full.extend_from_slice(&idxt_blocks[i]);
            let idxt_full = align_block(&idxt_full, 4, 0);

            let mut rec = b"INDX".to_vec();
            rec.extend_from_slice(&(header_length as u32).to_be_bytes());
            rec.extend_from_slice(&[0u8; 4]);
            rec.extend_from_slice(&1u32.to_be_bytes()); // record header type
            rec.extend_from_slice(&[0u8; 4]);
            rec.extend_from_slice(&((header_length + index_block.len()) as u32).to_be_bytes());
            rec.extend_from_slice(&(record_counts[i] as u32).to_be_bytes());
            rec.extend_from_slice(&[0xffu8; 8]);
            rec.extend_from_slice(&[0u8; 156]);
            rec.extend_from_slice(&index_block);
            rec.extend_from_slice(&idxt_full);
            if rec.len() > 0x10000 {
                bail!("Failed to rollover index blocks for very large index.");
            }
            index_records.push(rec);
        }

        let tagx = self.generate_tagx();

        let mut geometry = Vec::new();
        let mut idxt_geo = b"IDXT".to_vec();
        let mut pos = header_length + tagx.len();
        for (last_idx, &num) in last_indices.iter().zip(record_counts.iter()) {
            let start = geometry.len();
            idxt_geo.extend_from_slice(&(pos as u16).to_be_bytes());
            geometry.push(last_idx.len() as u8);
            geometry.extend_from_slice(last_idx.as_bytes());
            geometry.extend_from_slice(&(num as u16).to_be_bytes());
            pos += geometry.len() - start;
        }

        let total_entries: usize = record_counts.iter().sum();
        let mut header = index_header();
        header.set(
            "num_of_records",
            FieldValue::Int(index_records.len() as u64),
        )?;
        header.set("num_of_entries", FieldValue::Int(total_entries as u64))?;
        header.set("num_of_cncx", FieldValue::Int(cncx_num_records as u64))?;
        header.set("tagx", FieldValue::Bytes(align_block(&tagx, 4, 0)))?;
        header.set("geometry", FieldValue::Bytes(align_block(&geometry, 4, 0)))?;
        header.set("idxt", FieldValue::Bytes(align_block(&idxt_geo, 4, 0)))?;
        let header_bytes = header.build()?;

        let mut records = vec![header_bytes];
        records.extend(index_records);
        Ok(records)
    }
}

// SkelIndex {{{

/// One row of the SKEL table (`Skel` namedtuple in `skeleton.py`'s
/// `Chunker.create_tables`).
#[derive(Debug, Clone)]
pub struct SkelTableEntry {
    pub file_number: usize,
    pub name: String,
    pub chunk_count: usize,
    pub start_pos: usize,
    pub length: usize,
}

static SKEL_TAGS: &[TagMeta] = &[
    tag("chunk_count", 1, 1, 3, 0),
    tag("geometry", 6, 2, 12, 0),
    END_TAG_TABLE,
];

/// Port of `SkelIndex`: one entry per skeleton part, keyed by
/// `SKEL%010d`.
pub fn skel_index(skel_table: &[SkelTableEntry]) -> Result<Vec<Vec<u8>>> {
    let entries: Vec<(String, TagValues)> = skel_table
        .iter()
        .map(|s| {
            let mut tags = TagValues::new();
            // "Don't ask me why these entries have to be repeated
            // twice" -- comment preserved verbatim from `index.py`.
            tags.insert(
                "chunk_count",
                vec![s.chunk_count as u64, s.chunk_count as u64],
            );
            tags.insert(
                "geometry",
                vec![
                    s.start_pos as u64,
                    s.length as u64,
                    s.start_pos as u64,
                    s.length as u64,
                ],
            );
            (s.name.clone(), tags)
        })
        .collect();
    let builder = IndexBuilder {
        tag_types: SKEL_TAGS,
        control_byte_count: 1,
    };
    builder.build(&entries, 0)
}

// }}}

// ChunkIndex {{{

/// One row of the CHUNK table (`Chunk` namedtuple in `skeleton.py`'s
/// `Chunker.create_tables` -- distinct from [`crate::mobi::writer8::skeleton::Chunk`],
/// the in-memory fragment).
#[derive(Debug, Clone)]
pub struct ChunkTableEntry {
    pub insert_pos: usize,
    /// `"{P|S}-//*[@aid='{aid}']"`, matches the string the KF8 reader's
    /// `build_parts` strips (`idtext[12:-2]`) back down to a bare `aid`.
    pub selector: String,
    pub file_number: usize,
    pub sequence_number: usize,
    pub start_pos: usize,
    pub length: usize,
}

static CHUNK_TAGS: &[TagMeta] = &[
    tag("cncx_offset", 2, 1, 1, 0),
    tag("file_number", 3, 1, 2, 0),
    tag("sequence_number", 4, 1, 4, 0),
    tag("geometry", 6, 2, 8, 0),
    END_TAG_TABLE,
];

/// Port of `ChunkIndex`: one entry per fragment, keyed by the fragment's
/// zero-padded insert position (`f'{c.insert_pos:010}'`), which is
/// exactly how the KF8 reader recovers `insert_pos` back out of the
/// entry's identifier string.
pub fn chunk_index(chunk_table: &[ChunkTableEntry]) -> Result<(Vec<Vec<u8>>, CNCX)> {
    let selectors: Vec<String> = chunk_table.iter().map(|c| c.selector.clone()).collect();
    let cncx = CNCX::new(&selectors);
    let entries: Vec<(String, TagValues)> = chunk_table
        .iter()
        .map(|c| {
            let mut tags = TagValues::new();
            let offset = *cncx.strings.get(&c.selector).unwrap_or(&0) as u64;
            tags.insert("cncx_offset", vec![offset]);
            tags.insert("file_number", vec![c.file_number as u64]);
            tags.insert("sequence_number", vec![c.sequence_number as u64]);
            tags.insert("geometry", vec![c.start_pos as u64, c.length as u64]);
            (format!("{:010}", c.insert_pos), tags)
        })
        .collect();
    let builder = IndexBuilder {
        tag_types: CHUNK_TAGS,
        control_byte_count: 1,
    };
    let mut records = builder.build(&entries, cncx.records.len())?;
    records.extend(cncx.records.clone());
    Ok((records, cncx))
}

// }}}

// GuideIndex {{{

/// One row of the GUIDE table (`GuideRef` namedtuple in `main.py`'s
/// `create_guide`).
#[derive(Debug, Clone)]
pub struct GuideTableEntry {
    pub title: String,
    pub type_: String,
    /// `(pos, fid)`: chunk sequence number and offset-within-chunk of
    /// the referenced location.
    pub pos_fid: (u64, u64),
}

static GUIDE_TAGS: &[TagMeta] = &[
    tag("title", 1, 1, 1, 0),
    tag("pos_fid", 6, 2, 2, 0),
    END_TAG_TABLE,
];

/// Port of `GuideIndex`: one entry per `<guide>` reference, keyed by its
/// `type` attribute.
pub fn guide_index(guide_table: &[GuideTableEntry]) -> Result<Vec<Vec<u8>>> {
    let titles: Vec<String> = guide_table.iter().map(|g| g.title.clone()).collect();
    let cncx = CNCX::new(&titles);
    let entries: Vec<(String, TagValues)> = guide_table
        .iter()
        .map(|g| {
            let mut tags = TagValues::new();
            let offset = *cncx.strings.get(&g.title).unwrap_or(&0) as u64;
            tags.insert("title", vec![offset]);
            tags.insert("pos_fid", vec![g.pos_fid.0, g.pos_fid.1]);
            (g.type_.clone(), tags)
        })
        .collect();
    let builder = IndexBuilder {
        tag_types: GUIDE_TAGS,
        control_byte_count: 1,
    };
    let mut records = builder.build(&entries, cncx.records.len())?;
    records.extend(cncx.records.clone());
    Ok(records)
}

// }}}

// NCXIndex {{{

/// One row of the NCX table: the flattened, depth-first, linearized
/// (by `(depth, offset)`) TOC entry list `main.py`'s `create_indices`
/// builds before handing it to `NCXIndex`/`apply_trailing_byte_sequences`.
#[derive(Debug, Clone, Default)]
pub struct NcxTableEntry {
    pub index: u64,
    pub offset: u64,
    pub length: u64,
    pub label: String,
    pub depth: u64,
    pub pos_fid: (u64, u64),
    pub parent: Option<u64>,
    pub first_child: Option<u64>,
    pub last_child: Option<u64>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
}

static NCX_TAGS: &[TagMeta] = &[
    tag("offset", 1, 1, 1, 0),
    tag("length", 2, 1, 2, 0),
    tag("label", 3, 1, 4, 0),
    tag("depth", 4, 1, 8, 0),
    tag("parent", 21, 1, 16, 0),
    tag("first_child", 22, 1, 32, 0),
    tag("last_child", 23, 1, 64, 0),
    tag("pos_fid", 6, 2, 128, 0),
    END_TAG_TABLE,
];

static NON_LINEAR_NCX_TAGS: &[TagMeta] = &[
    tag("offset", 1, 1, 1, 0),
    tag("length", 2, 1, 2, 0),
    tag("label", 3, 1, 4, 0),
    tag("depth", 4, 1, 8, 0),
    tag("kind", 5, 1, 16, 0),
    tag("parent", 21, 1, 32, 0),
    tag("first_child", 22, 1, 64, 0),
    tag("last_child", 23, 1, 128, 0),
    END_TAG_TABLE,
    tag("pos_fid", 6, 2, 1, 0),
    END_TAG_TABLE,
];

fn ncx_cncx(toc_table: &[NcxTableEntry]) -> CNCX {
    let mut strings = Vec::new();
    for entry in toc_table {
        strings.push(entry.label.clone());
        if let Some(a) = &entry.author {
            if !a.is_empty() {
                strings.push(a.clone());
            }
        }
        if let Some(d) = &entry.description {
            if !d.is_empty() {
                strings.push(d.clone());
            }
        }
        if let Some(k) = &entry.kind {
            if !k.is_empty() {
                strings.push(k.clone());
            }
        }
    }
    CNCX::new(&strings)
}

fn ncx_entries(toc_table: &[NcxTableEntry], cncx: &CNCX) -> Vec<(String, TagValues)> {
    let largest = toc_table.iter().map(|x| x.index).max().unwrap_or(0);
    let width = format!("{largest:X}").len().max(2);

    toc_table
        .iter()
        .map(|x| {
            let mut tags = TagValues::new();
            tags.insert("offset", vec![x.offset]);
            tags.insert("length", vec![x.length]);
            tags.insert("depth", vec![x.depth]);
            tags.insert("pos_fid", vec![x.pos_fid.0, x.pos_fid.1]);
            if let Some(p) = x.parent {
                tags.insert("parent", vec![p]);
            }
            if let Some(c) = x.first_child {
                tags.insert("first_child", vec![c]);
            }
            if let Some(c) = x.last_child {
                tags.insert("last_child", vec![c]);
            }
            let label_off = *cncx.strings.get(&x.label).unwrap_or(&0) as u64;
            tags.insert("label", vec![label_off]);
            if let Some(d) = &x.description {
                if !d.is_empty() {
                    tags.insert(
                        "description",
                        vec![*cncx.strings.get(d).unwrap_or(&0) as u64],
                    );
                }
            }
            if let Some(a) = &x.author {
                if !a.is_empty() {
                    tags.insert("author", vec![*cncx.strings.get(a).unwrap_or(&0) as u64]);
                }
            }
            if let Some(k) = &x.kind {
                if !k.is_empty() {
                    tags.insert("kind", vec![*cncx.strings.get(k).unwrap_or(&0) as u64]);
                }
            }
            (format!("{:0width$X}", x.index, width = width), tags)
        })
        .collect()
}

/// Port of `NCXIndex`.
pub fn ncx_index(toc_table: &[NcxTableEntry]) -> Result<(Vec<Vec<u8>>, CNCX)> {
    let cncx = ncx_cncx(toc_table);
    let entries = ncx_entries(toc_table, &cncx);
    let builder = IndexBuilder {
        tag_types: NCX_TAGS,
        control_byte_count: 1,
    };
    let mut records = builder.build(&entries, cncx.records.len())?;
    records.extend(cncx.records.clone());
    Ok((records, cncx))
}

/// Port of `NonLinearNCXIndex`. `main.py` always sets `is_non_linear =
/// False` right after computing it ("False as we are using the
/// linearized entries" -- the branch exists for a heuristic Python
/// itself disabled), so this is dead code on the real write path; kept
/// for API parity, matching the Python module it ports.
pub fn non_linear_ncx_index(toc_table: &[NcxTableEntry]) -> Result<(Vec<Vec<u8>>, CNCX)> {
    let cncx = ncx_cncx(toc_table);
    let entries = ncx_entries(toc_table, &cncx);
    let builder = IndexBuilder {
        tag_types: NON_LINEAR_NCX_TAGS,
        control_byte_count: 2,
    };
    let mut records = builder.build(&entries, cncx.records.len())?;
    records.extend(cncx.records.clone());
    Ok((records, cncx))
}

// }}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skel_index_round_trips_through_the_reader() {
        let table = vec![
            SkelTableEntry {
                file_number: 0,
                name: "SKEL0000000000".to_string(),
                chunk_count: 2,
                start_pos: 0,
                length: 40,
            },
            SkelTableEntry {
                file_number: 1,
                name: "SKEL0000000001".to_string(),
                chunk_count: 1,
                start_pos: 100,
                length: 20,
            },
        ];
        let records = skel_index(&table).unwrap();
        assert!(records.len() >= 2);
        assert_eq!(&records[0][0..4], b"INDX");
        let (decoded, _cncx) = crate::mobi::index::read_index(&records, 0, "utf-8").unwrap();
        assert_eq!(decoded.len(), 2);
        let e0 = decoded.get("SKEL0000000000").unwrap();
        assert_eq!(e0.get(&1).unwrap()[0], 2); // chunk_count
                                               // geometry is written twice ("Don't ask me why" -- see
                                               // `skel_index`), so the reader decodes all 4 raw values.
        assert_eq!(e0.get(&6).unwrap(), &vec![0, 40, 0, 40]); // geometry
    }

    #[test]
    fn chunk_index_round_trips_and_selector_decodes_back_to_the_aid() {
        let table = vec![ChunkTableEntry {
            insert_pos: 12,
            selector: "P-//*[@aid='abc']".to_string(),
            file_number: 0,
            sequence_number: 0,
            start_pos: 0,
            length: 5,
        }];
        let (records, _cncx) = chunk_index(&table).unwrap();
        let (decoded, cncx) = crate::mobi::index::read_index(&records, 0, "utf-8").unwrap();
        let entry = decoded.get("0000000012").unwrap();
        let off = entry.get(&2).unwrap()[0] as usize;
        let selector = cncx.get(off).unwrap();
        assert_eq!(selector, "P-//*[@aid='abc']");
        assert_eq!(&selector[12..selector.len() - 2], "abc");
    }

    #[test]
    fn ncx_index_round_trips_hierarchy_fields() {
        let table = vec![
            NcxTableEntry {
                index: 0,
                offset: 0,
                length: 10,
                label: "Chapter 1".to_string(),
                depth: 0,
                pos_fid: (0, 0),
                ..Default::default()
            },
            NcxTableEntry {
                index: 1,
                offset: 10,
                length: 10,
                label: "Section 1.1".to_string(),
                depth: 1,
                pos_fid: (0, 10),
                parent: Some(0),
                ..Default::default()
            },
        ];
        let (records, _cncx) = ncx_index(&table).unwrap();
        let (decoded, cncx) = crate::mobi::index::read_index(&records, 0, "utf-8").unwrap();
        assert_eq!(decoded.len(), 2);
        let e1 = decoded.get("01").unwrap();
        assert_eq!(e1.get(&21).unwrap()[0], 0); // parent
        let label_off = e1.get(&3).unwrap()[0] as usize;
        assert_eq!(cncx.get(label_off).unwrap(), "Section 1.1");
    }
}
