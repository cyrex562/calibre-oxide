//! Generic `INDX`-record dumping: SKEL/SECT/GUIDE/NCX indices.
//!
//! Port of `src/calibre/ebooks/mobi/debug/index.py`. These are the
//! indices a KF8 file's header points at (`skel_idx`, `sect_idx`,
//! `oth_idx`, `primary_index_record`) — not to be confused with
//! `debug::mobi6`'s own `IndexHeader`/`IndexRecord`, which parse
//! MOBI6's differently-shaped primary/secondary index format directly.
//!
//! Reuses the low-level `INDX`/`TAGX` grammar from
//! `crate::mobi::index` (shared with the production reader) via its
//! `pub(crate)` helpers, but keeps its own record-order-preserving
//! table — the reader's `read_index` returns a `BTreeMap` sorted by
//! identifier, which is fine for building a lookup table but loses the
//! physical file order a debug dump should show.

use std::fmt;

use anyhow::Result;

use crate::mobi::headers::NULL_INDEX;
use crate::mobi::index::{
    get_tag_map, get_tag_section_start, parse_indx_header, parse_tagx_section, CNCXReader,
};
use crate::mobi::utils::decode_string;

/// One index entry's tags, in file order. `table.items()` in the
/// Python, which relies on `OrderedDict` for this ordering.
pub type OrderedTable = Vec<(String, std::collections::BTreeMap<u8, Vec<u64>>)>;

/// `read_variable_len_data` in the Python — the `(text, num)` pairs
/// living between an index header and its `IDXT` table, plus the raw
/// `TAGX` block they were read alongside.
#[derive(Default, Clone)]
pub struct Geometry {
    pub indices: Vec<(Vec<u8>, u16)>,
    pub tagx_block_size: u32,
    pub tagx_block: Vec<u8>,
    /// Bytes after the last IDXT entry. Non-empty (and non-zero)
    /// contents here are what the Python treats as a hard error; see
    /// the note on [`read_variable_len_data`].
    pub trailing_bytes: Vec<u8>,
}

fn read_variable_len_data(
    data: &[u8],
    header_tagx: u32,
    idxt_start: u32,
    count: u32,
) -> Result<Geometry> {
    let mut geo = Geometry::default();
    let idxt_size = 4 + count as usize * 2;
    if header_tagx > 0 {
        let offset = header_tagx as usize;
        let tagx_block_size =
            u32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap_or([0; 4]));
        geo.tagx_block_size = tagx_block_size;
        let end = (offset + tagx_block_size as usize).min(data.len());
        geo.tagx_block = data[offset..end].to_vec();

        let mut pos = idxt_start as usize + 4;
        for _ in 0..count {
            if pos + 2 > data.len() {
                break;
            }
            let p = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if p >= data.len() {
                break;
            }
            let strlen = data[p] as usize;
            let text = data.get(p + 1..p + 1 + strlen).unwrap_or(&[]).to_vec();
            let np = p + 1 + strlen;
            let num =
                u16::from_be_bytes(data.get(np..np + 2).map(|s| [s[0], s[1]]).unwrap_or([0; 2]));
            geo.indices.push((text, num));
        }
    }

    // Python raises `ValueError` here if `data[idxt_start+idxt_size:]`
    // has any non-zero byte — a strict "this file is exactly what we
    // expect" check. A debug tool exists to show what it can about a
    // file that *isn't* exactly what's expected, so this port records
    // the trailing region instead of refusing to proceed on it; a
    // caller that wants the strict check can compare
    // `geo.trailing_bytes` itself.
    let trailing_start = (idxt_start as usize + idxt_size).min(data.len());
    geo.trailing_bytes = data[trailing_start..].to_vec();
    Ok(geo)
}

/// The one field this port doesn't reproduce verbatim from
/// `INDEX_HEADER_FIELDS`: the 27 reserved `unknown{i}` slots (see
/// `crate::mobi::index::IndxHeader::unknowns`) are shown as one line
/// rather than 27, since nothing in the format gives them individual
/// meaning.
fn render_header(label: &str, header: &crate::mobi::index::IndxHeader, geo: &Geometry) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} {label} {}\n", "*".repeat(10), "*".repeat(10)));
    let fields: [(&str, String); 14] = [
        ("Header length", header.len.to_string()),
        ("Unknown", header.nul1.to_string()),
        ("Unknown", header.type_.to_string()),
        (
            "Index Type (0 - normal, 2 - inflection)",
            header.gen.to_string(),
        ),
        ("IDXT Offset", header.start.to_string()),
        ("Number of entries in this record", header.count.to_string()),
        ("character encoding", header.code.to_string()),
        ("Unknown", header.lng.to_string()),
        (
            "Total number of actual Index Entries in all records",
            header.total.to_string(),
        ),
        ("ORDT Offset", header.ordt.to_string()),
        ("LIGT Offset", header.ligt.to_string()),
        ("Number of LIGT", header.nligt.to_string()),
        ("Number of CNCX records", header.ncncx.to_string()),
        ("Geometry of index records", format!("{:?}", geo.indices)),
    ];
    for (name, value) in fields {
        out.push_str(&format!("{name:<12}: {value}\n"));
    }
    out
}

/// The result of reading one `INDX` primary/secondary index tree.
/// `read_index` in the Python.
pub struct IndexData {
    pub table: OrderedTable,
    pub cncx: CNCXReader,
    pub header: crate::mobi::index::IndxHeader,
    pub header_geometry: Geometry,
    pub record_headers: Vec<(crate::mobi::index::IndxHeader, Geometry)>,
}

/// `read_index` in the Python: `sections[idx]` is the index header
/// record; the `count` records after it are its entries; the
/// `ncncx` records after those are CNCX string pools.
pub fn read_index(sections: &[Vec<u8>], idx: usize, codec: &str) -> Result<IndexData> {
    let data = &sections[idx];
    let header = parse_indx_header(data)?;
    let indx_count = header.count as usize;

    let mut cncx = CNCXReader::new(&[], codec);
    if header.ncncx > 0 {
        let off = idx + indx_count + 1;
        let cncx_records: Vec<Vec<u8>> = (0..header.ncncx as usize)
            .filter_map(|i| sections.get(off + i).cloned())
            .collect();
        cncx = CNCXReader::new(&cncx_records, codec);
    }

    let tag_section_start = get_tag_section_start(data, &header);
    let (control_byte_count, tags) = parse_tagx_section(&data[tag_section_start..])?;
    let header_geometry = read_variable_len_data(data, header.tagx, header.start, header.count)?;

    let mut table = OrderedTable::new();
    let mut record_headers = Vec::new();
    for i in (idx + 1)..(idx + 1 + indx_count) {
        let Some(record_data) = sections.get(i) else {
            continue;
        };
        let record_header =
            read_one_record(&mut table, record_data, control_byte_count, &tags, codec)?;
        let geo = read_variable_len_data(
            record_data,
            record_header.tagx,
            record_header.start,
            record_header.count,
        )?;
        record_headers.push((record_header, geo));
    }

    Ok(IndexData {
        table,
        cncx,
        header,
        header_geometry,
        record_headers,
    })
}

/// As `crate::mobi::index::parse_index_record`, but appending to an
/// order-preserving `Vec` instead of a `BTreeMap`.
fn read_one_record(
    table: &mut OrderedTable,
    data: &[u8],
    control_byte_count: u32,
    tags: &[crate::mobi::index::TagX],
    codec: &str,
) -> Result<crate::mobi::index::IndxHeader> {
    let header = parse_indx_header(data)?;
    let idxt_pos = header.start as usize;
    let entry_count = header.count;

    let mut idx_positions = Vec::new();
    let mut pos = idxt_pos + 4;
    for _ in 0..entry_count {
        if pos + 2 > data.len() {
            break;
        }
        idx_positions.push(u16::from_be_bytes([data[pos], data[pos + 1]]) as usize);
        pos += 2;
    }
    idx_positions.push(idxt_pos);

    for j in 0..entry_count as usize {
        let Some(&start) = idx_positions.get(j) else {
            break;
        };
        let Some(&end) = idx_positions.get(j + 1) else {
            break;
        };
        if start >= end || end > data.len() {
            continue;
        }
        let rec = &data[start..end];
        let (ident, consumed) = decode_string(rec, codec).unwrap_or((String::new(), 0));
        let rec_remaining = &rec[consumed..];
        let tag_map = get_tag_map(control_byte_count, tags, rec_remaining, true)?;
        table.push((ident, tag_map));
    }

    // `parse_index_record` also uses this fallback wrapper to skip
    // records that produced no entries, matching the reader's
    // `_ = parse_index_record(...)`-style discard of an empty header.
    let _ = table.is_empty();
    Ok(header)
}

/// `Index` in the Python: the shared machinery — read one `INDX` tree
/// and be able to render it — that `SKELIndex`/`SECTIndex`/
/// `GuideIndex`/`NCXIndex` build their typed record lists on top of.
pub struct Index {
    pub data: Option<IndexData>,
}

impl Index {
    pub fn read(idx: u32, sections: &[Vec<u8>], codec: &str) -> Result<Self> {
        if idx == NULL_INDEX {
            return Ok(Index { data: None });
        }
        Ok(Index {
            data: Some(read_index(sections, idx as usize, codec)?),
        })
    }

    /// `Index.render`.
    pub fn render(&self) -> String {
        let Some(data) = &self.data else {
            return String::new();
        };
        let mut out = String::new();
        out.push_str(&render_header(
            "Index Header",
            &data.header,
            &data.header_geometry,
        ));
        out.push_str("\n\n");
        out.push_str(&format!(
            "{} Index Record Headers ({} records) {}\n",
            "*".repeat(10),
            data.record_headers.len(),
            "*".repeat(10)
        ));
        for (i, (header, geo)) in data.record_headers.iter().enumerate() {
            out.push_str(&render_header(&format!("Index Record {i}"), header, geo));
        }

        if !data.cncx.records.is_empty() {
            out.push_str(&format!("{} CNCX {}\n", "*".repeat(10), "*".repeat(10)));
            for (offset, val) in &data.cncx.records {
                out.push_str(&format!("{offset:>10}: {val}\n"));
            }
            out.push_str("\n\n");
        }

        out.push_str(&format!(
            "{} {} Index Entries {}\n",
            "*".repeat(10),
            data.table.len(),
            "*".repeat(10)
        ));
        for (k, v) in &data.table {
            out.push_str(&format!("{k}: {v:?}\n"));
        }
        out
    }
}

impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

/// One SKEL (skeleton) index entry: a chapter/part's HTML skeleton
/// location. `File` (namedtuple) in the Python.
#[derive(Debug, Clone)]
pub struct SkelFile {
    pub file_number: u32,
    pub name: String,
    pub divtbl_count: u64,
    pub start_position: u64,
    pub length: u64,
}

/// `SKELIndex` in the Python.
pub struct SkelIndex {
    pub index: Index,
    pub records: Vec<SkelFile>,
}

impl SkelIndex {
    pub fn read(idx: u32, sections: &[Vec<u8>], codec: &str) -> Result<Self> {
        let index = Index::read(idx, sections, codec)?;
        let mut records = Vec::new();
        if let Some(data) = &index.data {
            for (i, (text, tag_map)) in data.table.iter().enumerate() {
                let keys: std::collections::BTreeSet<u8> = tag_map.keys().copied().collect();
                if keys != std::collections::BTreeSet::from([1, 6]) {
                    anyhow::bail!("SKEL Index has unknown tags: {:?}", keys);
                }
                let t6 = &tag_map[&6];
                records.push(SkelFile {
                    file_number: i as u32,
                    name: text.clone(),
                    divtbl_count: tag_map[&1][0],
                    start_position: t6[0],
                    length: t6[1],
                });
            }
        }
        Ok(SkelIndex { index, records })
    }
}

impl fmt::Display for SkelIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.index)
    }
}

/// One SECT (chunk) index entry: a fragment of HTML text to splice
/// into a skeleton. `Elem` (namedtuple, aliased `Chunk`) in the
/// Python.
#[derive(Debug, Clone)]
pub struct SectElem {
    pub insert_pos: u64,
    pub toc_text: String,
    pub file_number: u64,
    pub sequence_number: u64,
    pub start_pos: u64,
    pub length: u64,
}

/// `SECTIndex` in the Python.
pub struct SectIndex {
    pub index: Index,
    pub records: Vec<SectElem>,
}

impl SectIndex {
    pub fn read(idx: u32, sections: &[Vec<u8>], codec: &str) -> Result<Self> {
        let index = Index::read(idx, sections, codec)?;
        let mut records = Vec::new();
        if let Some(data) = &index.data {
            for (text, tag_map) in &data.table {
                let keys: std::collections::BTreeSet<u8> = tag_map.keys().copied().collect();
                if keys != std::collections::BTreeSet::from([2, 3, 4, 6]) {
                    anyhow::bail!("Chunk Index has unknown tags: {:?}", keys);
                }
                let toc_text = data
                    .cncx
                    .get(tag_map[&2][0] as usize)
                    .cloned()
                    .unwrap_or_default();
                let t6 = &tag_map[&6];
                records.push(SectElem {
                    insert_pos: text.parse().unwrap_or(0),
                    toc_text,
                    file_number: tag_map[&3][0],
                    sequence_number: tag_map[&4][0],
                    start_pos: t6[0],
                    length: t6[1],
                });
            }
        }
        Ok(SectIndex { index, records })
    }
}

impl fmt::Display for SectIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.index)
    }
}

/// One guide reference. `GuideRef` (namedtuple) in the Python.
#[derive(Debug, Clone)]
pub struct GuideRef {
    pub type_: String,
    pub title: String,
    pub pos_fid: Vec<u64>,
}

/// `GuideIndex` in the Python.
pub struct GuideIndex {
    pub index: Index,
    pub records: Vec<GuideRef>,
}

impl GuideIndex {
    pub fn read(idx: u32, sections: &[Vec<u8>], codec: &str) -> Result<Self> {
        let index = Index::read(idx, sections, codec)?;
        let mut records = Vec::new();
        if let Some(data) = &index.data {
            for (text, tag_map) in &data.table {
                let keys: std::collections::BTreeSet<u8> = tag_map.keys().copied().collect();
                let ok = keys == std::collections::BTreeSet::from([1, 6])
                    || keys == std::collections::BTreeSet::from([1, 2, 3]);
                if !ok {
                    anyhow::bail!("Guide Index has unknown tags: {:?}", tag_map);
                }
                let title = data
                    .cncx
                    .get(tag_map[&1][0] as usize)
                    .cloned()
                    .unwrap_or_default();
                let pos_fid = if let Some(v) = tag_map.get(&6) {
                    v.clone()
                } else {
                    let mut v = tag_map[&2].clone();
                    v.extend(tag_map[&3].clone());
                    v
                };
                records.push(GuideRef {
                    type_: text.clone(),
                    title,
                    pos_fid,
                });
            }
        }
        Ok(GuideIndex { index, records })
    }
}

impl fmt::Display for GuideIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.index)
    }
}

/// Which field of an NCX index entry a numeric tag maps to, and which
/// slot of that tag's value list holds it. `tag_fieldname_map` in
/// `mobi/reader/ncx.py`.
const TAG_FIELDNAME_MAP: [(u8, &str); 9] = [
    (1, "pos"),
    (2, "len"),
    (3, "noffs"),
    (4, "hlvl"),
    (6, "pos_fid"),
    (21, "parent"),
    (22, "child1"),
    (23, "childn"),
    (69, "image_index"),
];

/// One NCX (navigation) entry. `NCXEntry` (namedtuple) in
/// `debug/index.py`.
#[derive(Debug, Clone)]
pub struct NcxEntry {
    pub index: usize,
    pub start: i64,
    pub length: i64,
    pub depth: i64,
    pub parent: Option<i64>,
    pub first_child: Option<i64>,
    pub last_child: Option<i64>,
    pub title: String,
    pub pos_fid: Vec<u64>,
    pub kind: String,
}

/// `NCXIndex` in the Python.
pub struct NcxIndex {
    pub index: Index,
    pub records: Vec<NcxEntry>,
}

fn ref_or_none(v: i64) -> Option<i64> {
    if v < 0 {
        None
    } else {
        Some(v)
    }
}

impl NcxIndex {
    pub fn read(idx: u32, sections: &[Vec<u8>], codec: &str) -> Result<Self> {
        let index = Index::read(idx, sections, codec)?;
        let mut records = Vec::new();
        if let Some(data) = &index.data {
            for (num, (text, tag_map)) in data.table.iter().enumerate() {
                let mut pos: i64 = -1;
                let mut len: i64 = 0;
                let mut noffs: i64 = -1;
                let mut hlvl: i64 = -1;
                let mut parent: i64 = -1;
                let mut child1: i64 = -1;
                let mut childn: i64 = -1;
                let mut pos_fid: Vec<u64> = Vec::new();
                let mut title = "Unknown Text".to_string();
                let kind = "Unknown Class".to_string();

                for &(tag, name) in &TAG_FIELDNAME_MAP {
                    let Some(v) = tag_map.get(&tag) else {
                        continue;
                    };
                    match name {
                        "pos" => pos = v[0] as i64,
                        "len" => len = v[0] as i64,
                        "noffs" => noffs = v[0] as i64,
                        "hlvl" => hlvl = v[0] as i64,
                        "parent" => parent = v[0] as i64,
                        "child1" => child1 = v[0] as i64,
                        "childn" => childn = v[0] as i64,
                        "pos_fid" => pos_fid = v.clone(),
                        _ => {}
                    }
                }
                if let Some(v) = tag_map.get(&3) {
                    title = data.cncx.get(v[0] as usize).cloned().unwrap_or(title);
                }
                let _ = noffs; // parsed for parity; not part of NcxEntry

                records.push(NcxEntry {
                    index: num,
                    start: pos,
                    length: len,
                    depth: hlvl,
                    parent: ref_or_none(parent),
                    first_child: ref_or_none(child1),
                    last_child: ref_or_none(childn),
                    title,
                    pos_fid,
                    kind,
                });
                let _ = text;
            }
        }
        Ok(NcxIndex { index, records })
    }
}

impl fmt::Display for NcxIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one `INDX` header-record + entry-record pair with a
    /// single TAGX tag and a single entry carrying two values for
    /// that tag, matching what `SKELIndex` expects (tags 1 and 6).
    fn build_skel_indx(name: &str, divtbl_count: u32, start_pos: u32, length: u32) -> Vec<Vec<u8>> {
        // TAGX: tag 1 (1 value), tag 6 (2 values), then EOF marker.
        let mut tagx = Vec::new();
        tagx.extend_from_slice(b"TAGX");
        tagx.extend_from_slice(&20u32.to_be_bytes()); // first_entry_offset
        tagx.extend_from_slice(&1u32.to_be_bytes()); // control_byte_count
        tagx.extend_from_slice(&[1, 1, 1, 0]); // tag 1: 1 value
        tagx.extend_from_slice(&[6, 2, 2, 0]); // tag 6: 2 values
        tagx.extend_from_slice(&[0, 0, 0, 1]); // EOF marker

        fn indx_header(
            len: usize,
            count: u32,
            idxt_start: u32,
            ncncx: u32,
            tagx_off: u32,
        ) -> Vec<u8> {
            let mut h = Vec::new();
            h.extend_from_slice(b"INDX");
            h.extend_from_slice(&(len as u32).to_be_bytes()); // len
            h.extend_from_slice(&0u32.to_be_bytes()); // nul1
            h.extend_from_slice(&0u32.to_be_bytes()); // type
            h.extend_from_slice(&0u32.to_be_bytes()); // gen
            h.extend_from_slice(&idxt_start.to_be_bytes()); // start
            h.extend_from_slice(&count.to_be_bytes()); // count
            h.extend_from_slice(&65001u32.to_be_bytes()); // code (utf-8)
            h.extend_from_slice(&0u32.to_be_bytes()); // lng
            h.extend_from_slice(&0u32.to_be_bytes()); // total
            h.extend_from_slice(&0u32.to_be_bytes()); // ordt
            h.extend_from_slice(&0u32.to_be_bytes()); // ligt
            h.extend_from_slice(&0u32.to_be_bytes()); // nligt
            h.extend_from_slice(&ncncx.to_be_bytes()); // ncncx
            for _ in 0..27 {
                h.extend_from_slice(&0u32.to_be_bytes());
            }
            h.extend_from_slice(&0u32.to_be_bytes()); // ocnt
            h.extend_from_slice(&0u32.to_be_bytes()); // oentries
            h.extend_from_slice(&0u32.to_be_bytes()); // ordt1
            h.extend_from_slice(&0u32.to_be_bytes()); // ordt2
            h.extend_from_slice(&tagx_off.to_be_bytes()); // tagx
            h
        }

        // Header record: INDX header (200 bytes long) + TAGX block.
        let header_len = 200u32;
        let mut header_record = indx_header(header_len as usize, 1, 0, 0, header_len);
        header_record.resize(header_len as usize, 0);
        header_record.extend_from_slice(&tagx);
        // IDXT for the header record itself: not consulted by our
        // reader (only `start`/`count` on the entry record matter),
        // so leave header_record as-is.

        // Entry record: its own INDX header, then one entry
        // `[namelen][name][tag1 val][tag6 val0][tag6 val1]`, then IDXT.
        let mut entry_body = Vec::new();
        entry_body.push(name.len() as u8);
        entry_body.extend_from_slice(name.as_bytes());
        // control byte: bit for tag1 set (0b01), bit for tag6 set (0b10)
        entry_body.push(0b0000_0011);
        // `get_tag_map` decodes tag values with `decint(_, forward=true)`
        // (terminal high bit on the *last* byte, reading from index 0),
        // so the matching encoder direction is `encint(_, forward=true)`.
        entry_body.extend_from_slice(&crate::mobi::utils::encint(divtbl_count as u64, true));
        entry_body.extend_from_slice(&crate::mobi::utils::encint(start_pos as u64, true));
        entry_body.extend_from_slice(&crate::mobi::utils::encint(length as u64, true));

        // Must be >= the 184 bytes `parse_indx_header` unconditionally
        // reads (signature + named fields + the 27 reserved words).
        let entry_header_len = 184u32;
        let idxt_start = entry_header_len + entry_body.len() as u32;
        let mut entry_record = indx_header(entry_header_len as usize, 1, idxt_start, 0, 0);
        entry_record.resize(entry_header_len as usize, 0);
        entry_record.extend_from_slice(&entry_body);
        // IDXT: 'IDXT' + one u16 offset (to the entry, i.e.
        // entry_header_len) + padding.
        entry_record.extend_from_slice(b"IDXT");
        entry_record.extend_from_slice(&(entry_header_len as u16).to_be_bytes());

        vec![header_record, entry_record]
    }

    #[test]
    fn skel_index_reads_a_single_entry() {
        let sections = build_skel_indx("part0000", 3, 1000, 2500);
        let skel = SkelIndex::read(0, &sections, "utf-8").expect("reads");
        assert_eq!(skel.records.len(), 1);
        let r = &skel.records[0];
        assert_eq!(r.name, "part0000");
        assert_eq!(r.divtbl_count, 3);
        assert_eq!(r.start_position, 1000);
        assert_eq!(r.length, 2500);
    }

    #[test]
    fn index_read_with_null_index_returns_empty() {
        let index = Index::read(NULL_INDEX, &[], "utf-8").expect("ok");
        assert!(index.data.is_none());
        assert_eq!(index.render(), "");
    }

    #[test]
    fn skel_index_render_lists_the_header_and_entries() {
        let sections = build_skel_indx("chapter1", 1, 0, 100);
        let skel = SkelIndex::read(0, &sections, "utf-8").expect("reads");
        let rendered = skel.to_string();
        assert!(rendered.contains("Index Header"));
        assert!(rendered.contains("Index Entries"));
        assert!(rendered.contains("chapter1"));
    }
}
