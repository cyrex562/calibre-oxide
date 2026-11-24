//! Build the native MOBI `INDX` binary index tree (TAGX-tagged entries,
//! `CNCX` string records, trailing byte sequences) for a serialized book.
//!
//! Port of `calibre.ebooks.mobi.writer2.indexer`. This is the inverse of
//! [`crate::mobi::index`], which decodes the same wire format on the
//! reader side -- entry/tag/control-byte layout here is read off that
//! decoder rather than re-derived from the format spec.
//!
//! # Scope gap: recipe-supplied article thumbnails
//!
//! `create_periodical_index` in Python also wires up
//! `art.toc_thumbnail` (an image a news recipe can attach to a TOC
//! article node) into `PeriodicalIndexEntry.image_index`. `TOCNode` has
//! no such field and recipe support is well outside this issue's scope,
//! so periodical articles here never carry a thumbnail image index --
//! narrow, and it doesn't affect the section/article tree structure or
//! TBS encoding, only that one optional per-article image pointer.

use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Result};

use crate::mobi::utils::{
    align_block, encint, encode_number_as_hex, encode_tbs, CNCX, RECORD_SIZE,
};
use crate::mobi::writer2::serializer::{urlnormalize, Serializer};
use crate::oeb::book::OEBBook;
use crate::oeb::toc::TOCNode;

// TAGX / entry-type bit layout {{{

fn bitmask(tag: u8) -> u8 {
    match tag {
        11 => 0b1,
        1 => 1 << 0,
        2 => 1 << 1,
        3 => 1 << 2,
        4 => 1 << 3,
        5 => 1 << 4,
        21 => 1 << 5,
        22 => 1 << 6,
        23 => 1 << 7,
        69 => 1 << 0,
        70 => 1 << 1,
        71 => 1 << 2,
        72 => 1 << 3,
        73 => 1 << 4,
        _ => 0,
    }
}

fn num_values(tag: u8) -> u8 {
    match tag {
        11 => 3,
        0 => 0,
        _ => 1,
    }
}

/// Builds a `TAGX` control-byte-table block. Port of the `TAGX` class in
/// `indexer.py`.
struct TagX {
    byts: Vec<u8>,
}

impl TagX {
    fn new() -> Self {
        TagX { byts: Vec::new() }
    }

    fn add_tag(&mut self, tag: u8) {
        self.byts.push(tag);
        self.byts.push(num_values(tag));
        self.byts.push(if tag != 0 { bitmask(tag) } else { 0 });
        self.byts.push(if tag != 0 { 0 } else { 1 });
    }

    fn header(&self, control_byte_count: u32) -> Vec<u8> {
        let mut h = b"TAGX".to_vec();
        h.extend_from_slice(&((12 + self.byts.len()) as u32).to_be_bytes());
        h.extend_from_slice(&control_byte_count.to_be_bytes());
        h
    }

    /// Primary index header TAGX block for a periodical.
    fn periodical() -> Vec<u8> {
        let mut t = TagX::new();
        for tag in [1u8, 2, 3, 4, 5, 21, 22, 23, 0, 69, 70, 71, 72, 73, 0] {
            t.add_tag(tag);
        }
        let mut out = t.header(2);
        out.extend_from_slice(&t.byts);
        out
    }

    /// Secondary index header TAGX block for a periodical.
    fn secondary() -> Vec<u8> {
        let mut t = TagX::new();
        for tag in [11u8, 0] {
            t.add_tag(tag);
        }
        let mut out = t.header(1);
        out.extend_from_slice(&t.byts);
        out
    }

    /// Primary index header TAGX block for a flat book.
    fn flat_book() -> Vec<u8> {
        let mut t = TagX::new();
        for tag in [1u8, 2, 3, 4, 0] {
            t.add_tag(tag);
        }
        let mut out = t.header(1);
        out.extend_from_slice(&t.byts);
        out
    }
}

// }}}

// Index entries {{{

/// `IndexEntry.index`: an integer for book/periodical entries, or one of
/// the five fixed strings (`author`, `caption`, ...) for
/// `SecondaryIndexEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexValue {
    Int(u64),
    Str(&'static str),
}

impl IndexValue {
    fn as_u64(&self) -> u64 {
        match self {
            IndexValue::Int(n) => *n,
            IndexValue::Str(_) => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Book,
    Periodical,
    Secondary,
}

/// A single row of the INDX entry table. Port of `IndexEntry` /
/// `PeriodicalIndexEntry` / `SecondaryIndexEntry` in `indexer.py`,
/// collapsed into one type with an internal `EntryKind` rather than a
/// Python-style subclass hierarchy.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    kind: EntryKind,
    pub offset: u64,
    pub label_offset: u64,
    pub depth: u64,
    pub class_offset: Option<u64>,
    pub control_byte_count: u8,
    pub length: u64,
    pub index: IndexValue,
    pub parent_index: Option<u64>,
    pub first_child_index: Option<u64>,
    pub last_child_index: Option<u64>,
    pub image_index: Option<u64>,
    pub author_offset: Option<u64>,
    pub desc_offset: Option<u64>,
    pub secondary: Option<[u64; 3]>,
}

impl IndexEntry {
    fn new_book(offset: u64, label_offset: u64) -> Self {
        IndexEntry {
            kind: EntryKind::Book,
            offset,
            label_offset,
            depth: 0,
            class_offset: None,
            control_byte_count: 1,
            length: 0,
            index: IndexValue::Int(0),
            parent_index: None,
            first_child_index: None,
            last_child_index: None,
            image_index: None,
            author_offset: None,
            desc_offset: None,
            secondary: None,
        }
    }

    fn new_periodical(offset: u64, label_offset: u64, class_offset: u64, depth: u64) -> Self {
        let mut e = IndexEntry::new_book(offset, label_offset);
        e.kind = EntryKind::Periodical;
        e.class_offset = Some(class_offset);
        e.depth = depth;
        e.control_byte_count = 2;
        e
    }

    fn new_secondary(index: &'static str, secondary: [u64; 3]) -> Self {
        let mut e = IndexEntry::new_book(0, 0);
        e.kind = EntryKind::Secondary;
        e.index = IndexValue::Str(index);
        e.secondary = Some(secondary);
        e
    }

    pub fn next_offset(&self) -> u64 {
        self.offset + self.length
    }

    pub fn index_u64(&self) -> u64 {
        self.index.as_u64()
    }

    fn tag_nums(&self) -> Vec<u8> {
        if self.kind == EntryKind::Secondary {
            return vec![11];
        }
        let mut v = vec![1u8, 2, 3, 4];
        if self.class_offset.is_some() {
            v.push(5);
        }
        if self.parent_index.is_some() {
            v.push(21);
        }
        if self.first_child_index.is_some() {
            v.push(22);
        }
        if self.last_child_index.is_some() {
            v.push(23);
        }
        v
    }

    fn entry_type(&self) -> u8 {
        if self.kind == EntryKind::Secondary {
            return 1;
        }
        self.tag_nums().iter().fold(0u8, |acc, &t| acc | bitmask(t))
    }

    /// Port of `IndexEntry.bytestring`: the entry's on-disk encoding.
    pub fn bytestring(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        match &self.index {
            IndexValue::Int(n) => buf.extend_from_slice(&encode_number_as_hex(*n)),
            IndexValue::Str(s) => {
                let raw = s.as_bytes();
                buf.push(raw.len() as u8);
                buf.extend_from_slice(raw);
            }
        }
        buf.push(self.entry_type());

        if self.control_byte_count == 2 {
            let mut flags = 0u8;
            if self.image_index.is_some() {
                flags |= bitmask(69);
            }
            if self.desc_offset.is_some() {
                flags |= bitmask(70);
            }
            if self.author_offset.is_some() {
                flags |= bitmask(71);
            }
            buf.push(flags);
        }

        for tag in self.tag_nums() {
            match tag {
                1 => buf.extend(encint(self.offset, true)),
                2 => buf.extend(encint(self.length, true)),
                3 => buf.extend(encint(self.label_offset, true)),
                4 => buf.extend(encint(self.depth, true)),
                5 => buf.extend(encint(self.class_offset.unwrap_or(0), true)),
                21 => buf.extend(encint(self.parent_index.unwrap_or(0), true)),
                22 => buf.extend(encint(self.first_child_index.unwrap_or(0), true)),
                23 => buf.extend(encint(self.last_child_index.unwrap_or(0), true)),
                11 => {
                    for x in self.secondary.unwrap_or([0, 0, 0]) {
                        buf.extend(encint(x, true));
                    }
                }
                _ => {}
            }
        }

        if self.control_byte_count == 2 {
            for v in [self.image_index, self.desc_offset, self.author_offset]
                .into_iter()
                .flatten()
            {
                buf.extend(encint(v, true));
            }
        }

        buf
    }
}

/// The five fixed secondary-index rows (`author`, `caption`, `credit`,
/// `description`, `mastheadImage`), in descending-tag order. Port of
/// `SecondaryIndexEntry.entries`.
fn secondary_entries() -> Vec<IndexEntry> {
    let map: [(&'static str, u64); 5] = [
        ("author", 73),
        ("caption", 72),
        ("credit", 71),
        ("description", 70),
        ("mastheadImage", 69),
    ];
    let min_tag = map.iter().map(|(_, t)| *t).min().unwrap();
    let mut sorted = map.to_vec();
    sorted.sort_by_key(|&(_, tag)| std::cmp::Reverse(tag));
    sorted
        .into_iter()
        .map(|(name, tag)| {
            let first = if tag == min_tag { 5 } else { 0 };
            IndexEntry::new_secondary(name, [first, 0, tag])
        })
        .collect()
}

// }}}

// Trailing byte sequences {{{

struct RecordData<'a> {
    starts: Vec<&'a IndexEntry>,
    completes: Vec<&'a IndexEntry>,
    ends: Vec<&'a IndexEntry>,
    spans: Option<&'a IndexEntry>,
}

fn hm(pairs: &[(u32, u64)]) -> HashMap<u32, u64> {
    pairs.iter().cloned().collect()
}

/// Port of `TBS.book_tbs`.
fn book_tbs(data: &RecordData) -> Vec<u8> {
    if let Some(spanner) = data.spans {
        encode_tbs(spanner.index_u64(), &hm(&[(0b010, 0), (0b001, 0)]), 3)
    } else {
        let (starts, completes, ends) = (&data.starts, &data.completes, &data.ends);
        if completes.is_empty()
            && ((starts.len() == 1 && ends.is_empty()) || (ends.len() == 1 && starts.is_empty()))
        {
            let node = if !starts.is_empty() {
                starts[0]
            } else {
                ends[0]
            };
            encode_tbs(node.index_u64(), &hm(&[(0b010, 0)]), 3)
        } else {
            let mut nodes: Vec<&IndexEntry> = starts
                .iter()
                .chain(completes.iter())
                .chain(ends.iter())
                .copied()
                .collect();
            nodes.sort_by_key(|n| n.index_u64());
            encode_tbs(
                nodes[0].index_u64(),
                &hm(&[(0b010, 0), (0b100, nodes.len() as u64)]),
                3,
            )
        }
    }
}

/// Port of `TBS.periodical_tbs`.
#[allow(clippy::too_many_lines)]
fn periodical_tbs(data: &RecordData, section_map: &BTreeMap<u64, &IndexEntry>) -> Vec<u8> {
    let type_010 = encode_tbs(0, &hm(&[(0b010, 0)]), 3);
    let type_011 = encode_tbs(0, &hm(&[(0b010, 0), (0b001, 0)]), 3);
    let type_110 = encode_tbs(0, &hm(&[(0b100, 2), (0b010, 0)]), 3);
    let type_111 = encode_tbs(0, &hm(&[(0b100, 2), (0b010, 0), (0b001, 0)]), 3);

    let mut depth_map: HashMap<u64, Vec<&IndexEntry>> = HashMap::new();
    for x in data
        .starts
        .iter()
        .chain(data.ends.iter())
        .chain(data.completes.iter())
    {
        depth_map.entry(x.depth).or_default().push(x);
    }
    for v in depth_map.values_mut() {
        v.sort_by_key(|e| e.offset);
    }
    let empty: Vec<&IndexEntry> = Vec::new();
    let at_depth = |d: u64| -> &Vec<&IndexEntry> { depth_map.get(&d).unwrap_or(&empty) };

    let has_section_start = !at_depth(1).is_empty()
        && at_depth(1)
            .iter()
            .any(|n| data.starts.iter().any(|s| std::ptr::eq(*s, *n)));
    let spanner = data.spans;
    let parent_section_index: i64;

    #[derive(PartialEq, Eq, Clone, Copy)]
    enum Typ {
        T010,
        T011,
        T110,
        T111,
    }
    let typ: Typ;

    if !at_depth(0).is_empty() {
        let mut first_node: Option<&IndexEntry> = None;
        for nodes in [at_depth(1), at_depth(2)] {
            for &node in nodes {
                let better = match first_node {
                    None => true,
                    Some(fnode) => (node.offset, node.depth) < (fnode.offset, fnode.depth),
                };
                if better {
                    first_node = Some(node);
                }
            }
        }
        typ = if has_section_start {
            Typ::T110
        } else {
            Typ::T010
        };
        match first_node {
            Some(fnode) if fnode.depth > 0 => {
                parent_section_index = if fnode.depth == 1 {
                    fnode.index_u64() as i64
                } else {
                    fnode.parent_index.unwrap_or(0) as i64
                };
            }
            _ => {
                parent_section_index = section_map.keys().max().copied().unwrap_or(0) as i64;
            }
        }
    } else if let Some(sp) = spanner {
        parent_section_index = sp.parent_index.unwrap_or(0) as i64;
        typ = if parent_section_index == 1 {
            Typ::T110
        } else {
            Typ::T010
        };
    } else if at_depth(1).is_empty() {
        if at_depth(2).is_empty() {
            // Defensive: Python would IndexError here; we degrade to an
            // empty TBS rather than panicking on malformed input.
            return Vec::new();
        }
        parent_section_index = at_depth(2)[0].parent_index.unwrap_or(0) as i64;
        typ = if parent_section_index == 1 {
            Typ::T111
        } else {
            Typ::T010
        };
    } else {
        parent_section_index = if !at_depth(2).is_empty() {
            at_depth(2)[0].parent_index.unwrap_or(0) as i64
        } else {
            at_depth(1)[0].index_u64() as i64
        };
        typ = Typ::T011;
    }

    let typ_bytes = match typ {
        Typ::T010 => &type_010,
        Typ::T011 => &type_011,
        Typ::T110 => &type_110,
        Typ::T111 => &type_111,
    };
    let mut buf = typ_bytes.clone();

    if typ != Typ::T110 && typ != Typ::T111 && parent_section_index > 0 {
        let mut extra = HashMap::new();
        if spanner.is_none() {
            let num_articles = at_depth(1)
                .iter()
                .filter(|a| a.parent_index == Some(parent_section_index as u64))
                .count();
            if at_depth(1).is_empty() {
                extra = hm(&[(0b0001, 0)]);
            }
            if num_articles > 1 {
                extra = hm(&[(0b0100, num_articles as u64)]);
            }
        }
        buf.extend(encode_tbs(parent_section_index as u64, &extra, 4));
    }

    if spanner.is_none() {
        let articles = at_depth(2);
        let mut section_offsets: Vec<u64> = articles
            .iter()
            .filter_map(|a| a.parent_index)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        section_offsets.sort_by_key(|idx| section_map.get(idx).map(|s| s.offset).unwrap_or(0));
        let sections: Vec<&IndexEntry> = section_offsets
            .iter()
            .filter_map(|idx| section_map.get(idx).copied())
            .collect();

        for (i, &section) in sections.iter().enumerate() {
            let sec_articles: Vec<&&IndexEntry> = articles
                .iter()
                .filter(|a| a.parent_index == Some(section.index_u64()))
                .collect();
            if sec_articles.is_empty() {
                continue;
            }
            let first_article = *sec_articles[0];
            let last_article = *sec_articles[sec_articles.len() - 1];
            let num = sec_articles.len();
            let last_article_ends = data
                .ends
                .iter()
                .chain(data.completes.iter())
                .any(|e| std::ptr::eq(*e, last_article));

            let next_sec = sections.get(i + 1).copied();

            let mut extra = HashMap::new();
            if num > 1 {
                extra.insert(0b0100, num as u64);
            }
            let delta = first_article.index_u64() as i64 - section.index_u64() as i64;
            buf.extend(encode_tbs(delta.max(0) as u64, &extra, 4));

            if let Some(ns) = next_sec {
                let delta = last_article.index_u64() as i64 - ns.index_u64() as i64;
                buf.extend(encode_tbs(delta.unsigned_abs(), &hm(&[(0b1000, 0)]), 4));
            } else if typ == Typ::T011
                && last_article_ends
                && !(last_article.offset + last_article.length).is_multiple_of(RECORD_SIZE as u64)
            {
                let delta = last_article.index_u64() as i64 - section.index_u64() as i64 - 1;
                buf.extend(encode_tbs(delta.max(0) as u64, &hm(&[(0b1000, 0)]), 4));
            }
        }
    } else if let Some(sp) = spanner {
        let delta = sp.index_u64() as i64 - parent_section_index;
        buf.extend(encode_tbs(delta.max(0) as u64, &hm(&[(0b0001, 0)]), 4));
    }

    buf
}

// }}}

// Indexer {{{

/// Builds the primary (and, for periodicals, secondary) `INDX` header +
/// entry records, the `CNCX` string records, and the per-text-record
/// trailing byte sequences.
///
/// Port of `Indexer` in `indexer.py`.
pub struct Indexer {
    pub is_periodical: bool,
    pub is_flat_periodical: bool,
    /// header, index-entries record, CNCX records, and (for
    /// periodicals) the secondary header + secondary index-entries
    /// record, in emission order -- append directly after
    /// `MobiWriter`'s text records.
    pub records: Vec<Vec<u8>>,
    /// Set only for periodicals: offset (within `records`) of the
    /// secondary index header.
    pub secondary_record_offset: Option<usize>,
    cncx: CNCX,
    indices: Vec<IndexEntry>,
    tbs_map: HashMap<usize, Vec<u8>>,
    pub warnings: Vec<String>,
}

impl Indexer {
    /// `masthead_offset`: image record index of the periodical's
    /// masthead (always `0` in practice -- see `resources.rs`).
    pub fn new(
        serializer: &Serializer,
        oeb: &OEBBook,
        number_of_text_records: usize,
        is_periodical: bool,
        masthead_offset: usize,
    ) -> Result<Self> {
        let is_flat_periodical = if is_periodical {
            oeb.toc
                .first()
                .map(|p| p.children.len() == 1)
                .unwrap_or(false)
        } else {
            false
        };

        let cncx_strings = build_cncx_strings(&oeb.toc, is_periodical);
        let cncx = CNCX::new(&cncx_strings);

        let mut indexer = Indexer {
            is_periodical,
            is_flat_periodical,
            records: Vec::new(),
            secondary_record_offset: None,
            cncx,
            indices: Vec::new(),
            tbs_map: HashMap::new(),
            warnings: Vec::new(),
        };

        indexer.indices = if is_periodical {
            indexer.create_periodical_index(oeb, serializer, masthead_offset)?
        } else {
            indexer.create_book_index(oeb, serializer)
        };

        if indexer.indices.is_empty() {
            bail!("No valid entries in TOC, cannot generate index");
        }

        indexer.records.push(indexer.create_index_record(false)?);
        indexer.records.insert(0, indexer.create_header(false));
        indexer.records.extend(indexer.cncx.records.clone());

        if is_periodical {
            indexer.secondary_record_offset = Some(indexer.records.len());
            indexer.records.push(indexer.create_header(true));
            indexer.records.push(indexer.create_index_record(true)?);
        }

        indexer.calculate_trailing_byte_sequences(number_of_text_records);

        Ok(indexer)
    }

    fn create_index_record(&self, secondary: bool) -> Result<Vec<u8>> {
        let header_length = 192usize;
        let owned;
        let indices: &[IndexEntry] = if secondary {
            owned = secondary_entries();
            &owned
        } else {
            &self.indices
        };

        let mut offsets = Vec::new();
        let mut body_buf = Vec::new();
        for i in indices {
            offsets.push(body_buf.len());
            body_buf.extend(i.bytestring());
        }
        let index_block = align_block(&body_buf, 4, 0);

        let mut idxt_tail = Vec::new();
        for offset in &offsets {
            idxt_tail.extend_from_slice(&((header_length + offset) as u16).to_be_bytes());
        }
        let mut idxt_block = b"IDXT".to_vec();
        idxt_block.extend_from_slice(&idxt_tail);
        let idxt_block = align_block(&idxt_block, 4, 0);

        let body = [index_block.clone(), idxt_block].concat();

        let mut header = b"INDX".to_vec();
        header.extend_from_slice(&(header_length as u32).to_be_bytes());
        header.extend_from_slice(&[0u8; 4]);
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[0u8; 4]);
        header.extend_from_slice(&((header_length + index_block.len()) as u32).to_be_bytes());
        header.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        header.extend_from_slice(&[0xffu8; 8]);
        header.extend_from_slice(&[0u8; 156]);

        let ans = [header, body].concat();
        if ans.len() > 0x10000 {
            bail!("Too many entries ({}) in the TOC", offsets.len());
        }
        Ok(ans)
    }

    fn create_header(&self, secondary: bool) -> Vec<u8> {
        let tagx_block = if secondary {
            TagX::secondary()
        } else if self.is_periodical {
            TagX::periodical()
        } else {
            TagX::flat_book()
        };
        let header_length = 192usize;
        let mut buf = Vec::new();

        buf.extend_from_slice(b"INDX");
        buf.extend_from_slice(&(header_length as u32).to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        buf.extend_from_slice(&2u32.to_be_bytes()); // index type
        buf.extend_from_slice(&0u32.to_be_bytes()); // IDXT offset, filled below
                                                    // Number of index records: always 1. See the module doc for why
                                                    // this is a hardcoded literal rather than `self.records.len()`:
                                                    // in Python, this reads `len(self.records)` at the exact call
                                                    // site in `__init__` where `self.records` holds only the
                                                    // just-created index-entries record, so it is always 1 in both
                                                    // the primary and (explicitly `1 if secondary`) secondary case.
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&65001u32.to_be_bytes()); // utf-8
        buf.extend_from_slice(&[0xffu8; 4]);

        let owned;
        let indices: &[IndexEntry] = if secondary {
            owned = secondary_entries();
            &owned
        } else {
            &self.indices
        };
        buf.extend_from_slice(&(indices.len() as u32).to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes()); // ORDT offset
        buf.extend_from_slice(&0u32.to_be_bytes()); // LIGT offset
        buf.extend_from_slice(&0u32.to_be_bytes()); // num LIGT entries
        buf.extend_from_slice(
            &(if secondary {
                0
            } else {
                self.cncx.records.len()
            } as u32)
                .to_be_bytes(),
        );
        buf.extend_from_slice(&[0u8; 124]);
        buf.extend_from_slice(&(header_length as u32).to_be_bytes()); // TAGX offset
        buf.extend_from_slice(&[0u8; 8]);
        buf.extend_from_slice(&tagx_block);

        let num = indices.len();
        let last = &indices[indices.len() - 1];
        match &last.index {
            IndexValue::Int(n) => buf.extend(encode_number_as_hex(*n)),
            IndexValue::Str(s) => {
                let raw = s.as_bytes();
                buf.push(raw.len() as u8);
                buf.extend_from_slice(raw);
            }
        }
        buf.extend_from_slice(&(num as u16).to_be_bytes());

        let pad = (4 - (buf.len() % 4)) % 4;
        buf.extend(std::iter::repeat_n(0u8, pad));

        let idxt_offset = buf.len();
        buf.extend_from_slice(b"IDXT");
        buf.extend_from_slice(&((header_length + tagx_block.len()) as u16).to_be_bytes());
        buf.push(0);
        buf[20..24].copy_from_slice(&(idxt_offset as u32).to_be_bytes());

        align_block(&buf, 4, 0)
    }

    fn create_book_index(&mut self, oeb: &OEBBook, serializer: &Serializer) -> Vec<IndexEntry> {
        let mut indices: Vec<IndexEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for node in oeb.toc.iter_descendants(false) {
            let Some(href) = node.href.as_deref() else {
                self.warnings.push(format!(
                    "TOC item {:?} has no href",
                    node.title.as_deref().unwrap_or_default()
                ));
                continue;
            };
            let Some(&offset) = serializer.id_offsets.get(&urlnormalize(href)) else {
                self.warnings.push(format!(
                    "TOC item {:?} [{href}] not found in document",
                    node.title.as_deref().unwrap_or_default()
                ));
                continue;
            };
            let title = node.title.clone().unwrap_or_default();
            let Some(&label) = self.cncx.strings.get(&title) else {
                self.warnings
                    .push(format!("TOC item {title:?} [{href}] not found in document"));
                continue;
            };
            if !seen.insert(offset) {
                continue;
            }
            indices.push(IndexEntry::new_book(offset as u64, label as u64));
        }

        indices.sort_by_key(|x| x.offset);
        set_book_lengths(&mut indices, serializer.body_end_offset as u64);
        indices.retain(|x| x.length > 0);
        set_book_lengths(&mut indices, serializer.body_end_offset as u64);
        for (i, x) in indices.iter_mut().enumerate() {
            x.index = IndexValue::Int(i as u64);
        }
        indices
    }

    fn create_periodical_index(
        &mut self,
        oeb: &OEBBook,
        serializer: &Serializer,
        _masthead_offset: usize,
    ) -> Result<Vec<IndexEntry>> {
        let Some(periodical_node) = oeb.toc.first() else {
            bail!("No periodical root TOC node");
        };
        let periodical_node_offset = serializer.body_start_offset as u64;
        let periodical_node_size = serializer.body_end_offset as u64 - periodical_node_offset;

        let title = periodical_node.title.clone().unwrap_or_default();
        let klass = periodical_node.klass.clone().unwrap_or_default();
        let mut periodical = IndexEntry::new_periodical(
            periodical_node_offset,
            *self.cncx.strings.get(&title).unwrap_or(&0) as u64,
            *self.cncx.strings.get(&klass).unwrap_or(&0) as u64,
            0,
        );
        periodical.length = periodical_node_size;
        periodical.first_child_index = Some(1);
        periodical.image_index = Some(0);

        let mut seen_sec_offsets = std::collections::HashSet::new();
        let mut seen_art_offsets = std::collections::HashSet::new();
        let mut normalized_sections: Vec<(IndexEntry, Vec<IndexEntry>)> = Vec::new();

        for sec in &periodical_node.children {
            let Some(href) = sec.href.as_deref() else {
                continue;
            };
            let Some(&offset) = serializer.id_offsets.get(&urlnormalize(href)) else {
                continue;
            };
            if !seen_sec_offsets.insert(offset) {
                continue;
            }
            let sec_title = sec.title.clone().unwrap_or_default();
            let sec_klass = sec.klass.clone().unwrap_or_default();
            let mut section = IndexEntry::new_periodical(
                offset as u64,
                *self.cncx.strings.get(&sec_title).unwrap_or(&0) as u64,
                *self.cncx.strings.get(&sec_klass).unwrap_or(&0) as u64,
                1,
            );
            section.parent_index = Some(0);

            let mut normalized_articles: Vec<IndexEntry> = Vec::new();
            for art in &sec.children {
                let Some(art_href) = art.href.as_deref() else {
                    continue;
                };
                let Some(&art_offset) = serializer.id_offsets.get(&urlnormalize(art_href)) else {
                    continue;
                };
                if !seen_art_offsets.insert(art_offset) {
                    continue;
                }
                let art_title = art.title.clone().unwrap_or_default();
                let art_klass = art.klass.clone().unwrap_or_default();
                let mut article = IndexEntry::new_periodical(
                    art_offset as u64,
                    *self.cncx.strings.get(&art_title).unwrap_or(&0) as u64,
                    *self.cncx.strings.get(&art_klass).unwrap_or(&0) as u64,
                    2,
                );
                let author = art.author.clone().unwrap_or_default();
                let desc = art.description.clone().unwrap_or_default();
                article.author_offset = Some(*self.cncx.strings.get(&author).unwrap_or(&0) as u64);
                article.desc_offset = Some(*self.cncx.strings.get(&desc).unwrap_or(&0) as u64);
                normalized_articles.push(article);
            }

            if !normalized_articles.is_empty() {
                normalized_articles.sort_by_key(|a| a.offset);
                normalized_sections.push((section, normalized_articles));
            }
        }

        normalized_sections.sort_by_key(|(s, _)| s.offset);

        // Set lengths.
        let body_end = serializer.body_end_offset as u64;
        for s in 0..normalized_sections.len() {
            let sec_len = normalized_sections
                .get(s + 1)
                .map(|(next, _)| next.offset)
                .unwrap_or(body_end)
                - normalized_sections[s].0.offset;
            normalized_sections[s].0.length = sec_len;
            let articles_len = normalized_sections[s].1.len();
            for i in 0..articles_len {
                let next_offset = if i + 1 < articles_len {
                    normalized_sections[s].1[i + 1].offset
                } else {
                    normalized_sections[s].0.offset + normalized_sections[s].0.length
                };
                normalized_sections[s].1[i].length =
                    next_offset - normalized_sections[s].1[i].offset;
            }
        }

        // Filter empty.
        for (_, articles) in normalized_sections.iter_mut() {
            articles.retain(|a| a.length > 0);
        }
        normalized_sections.retain(|(s, arts)| s.length > 0 && !arts.is_empty());

        // Set indices.
        let mut i: u64 = 0;
        for (sec, _) in normalized_sections.iter_mut() {
            i += 1;
            sec.index = IndexValue::Int(i);
            sec.parent_index = Some(0);
        }
        for (sec, articles) in normalized_sections.iter_mut() {
            let sec_index = sec.index_u64();
            for art in articles.iter_mut() {
                i += 1;
                art.index = IndexValue::Int(i);
                art.parent_index = Some(sec_index);
            }
        }
        for (sec, articles) in normalized_sections.iter_mut() {
            sec.first_child_index = Some(articles[0].index_u64());
            sec.last_child_index = Some(articles[articles.len() - 1].index_u64());
        }

        // Close gaps left by filtering.
        for s in 0..normalized_sections.len() {
            let next_offset = normalized_sections
                .get(s + 1)
                .map(|(next, _)| next.offset)
                .unwrap_or(body_end);
            normalized_sections[s].0.length = next_offset - normalized_sections[s].0.offset;
            let sec_next = normalized_sections[s].0.next_offset();
            let articles_len = normalized_sections[s].1.len();
            for a in 0..articles_len {
                let next_offset = if a + 1 < articles_len {
                    normalized_sections[s].1[a + 1].offset
                } else {
                    sec_next
                };
                normalized_sections[s].1[a].length =
                    next_offset - normalized_sections[s].1[a].offset;
            }
        }

        for (s, (sec, articles)) in normalized_sections.iter().enumerate() {
            match normalized_sections.get(s + 1) {
                Some((next_sec, _)) => {
                    if next_sec.offset != sec.next_offset() || sec.length == 0 {
                        bail!("Invalid section layout");
                    }
                }
                None => {
                    if sec.length == 0 || sec.next_offset() != body_end {
                        bail!("Invalid section layout");
                    }
                }
            }
            for (a, art) in articles.iter().enumerate() {
                match articles.get(a + 1) {
                    Some(next_art) => {
                        if art.length == 0 || art.next_offset() != next_art.offset {
                            bail!("Invalid article layout");
                        }
                    }
                    None => {
                        if art.length == 0 || art.next_offset() != sec.next_offset() {
                            bail!("Invalid article layout");
                        }
                    }
                }
            }
        }

        let mut indices = vec![periodical];
        for (sec, _) in &normalized_sections {
            indices.last_mut().unwrap().last_child_index = Some(sec.index_u64());
            indices.push(sec.clone());
        }
        for (_, articles) in &normalized_sections {
            for a in articles {
                indices.push(a.clone());
            }
        }

        Ok(indices)
    }

    fn calculate_trailing_byte_sequences(&mut self, number_of_text_records: usize) {
        let mut tbs_map = HashMap::new();
        let mut found_node = false;

        let sections: Vec<&IndexEntry> = self.indices.iter().filter(|i| i.depth == 1).collect();
        let mut sorted_sections = sections.clone();
        sorted_sections.sort_by_key(|s| s.offset);
        let section_map: BTreeMap<u64, &IndexEntry> = sorted_sections
            .into_iter()
            .map(|s| (s.index_u64(), s))
            .collect();

        let deepest = self.indices.iter().map(|i| i.depth).max().unwrap_or(0);

        for i in 0..number_of_text_records {
            let offset = (i * RECORD_SIZE) as u64;
            let next_offset = offset + RECORD_SIZE as u64;

            let mut data = RecordData {
                starts: Vec::new(),
                completes: Vec::new(),
                ends: Vec::new(),
                spans: None,
            };

            for index in &self.indices {
                if index.offset >= next_offset {
                    continue;
                }
                if index.next_offset() <= offset {
                    continue;
                }
                if index.offset >= offset {
                    if index.next_offset() <= next_offset {
                        data.completes.push(index);
                    } else {
                        data.starts.push(index);
                    }
                } else if index.next_offset() <= next_offset {
                    data.ends.push(index);
                } else if index.depth == deepest {
                    data.spans = Some(index);
                }
            }

            let has_data = !data.ends.is_empty()
                || !data.completes.is_empty()
                || !data.starts.is_empty()
                || data.spans.is_some();

            let bytes = if has_data {
                found_node = true;
                if self.is_periodical {
                    periodical_tbs(&data, &section_map)
                } else {
                    book_tbs(&data)
                }
            } else if self.is_periodical && found_node {
                encode_tbs(0, &hm(&[(0b010, 0), (0b001, 0)]), 3)
            } else {
                Vec::new()
            };

            tbs_map.insert(i + 1, bytes);
        }

        self.tbs_map = tbs_map;
    }

    pub fn get_trailing_byte_sequence(&self, num: usize) -> &[u8] {
        self.tbs_map
            .get(&num)
            .map(|v| v.as_slice())
            .unwrap_or_default()
    }
}

fn set_book_lengths(indices: &mut [IndexEntry], body_end_offset: u64) {
    let n = indices.len();
    for i in 0..n {
        let next_offset = if i + 1 < n {
            indices[i + 1].offset
        } else {
            body_end_offset
        };
        indices[i].length = next_offset.saturating_sub(indices[i].offset);
    }
}

/// Collect the strings a `CNCX` needs from a TOC: every node's title
/// plus, for periodicals, its class/author/description. Port of the
/// Python `CNCX(toc, is_periodical)` subclass constructor.
fn build_cncx_strings(toc: &crate::oeb::toc::TOC, is_periodical: bool) -> Vec<String> {
    let mut strings = Vec::new();
    let collect = |node: &TOCNode, strings: &mut Vec<String>| {
        strings.push(node.title.clone().unwrap_or_default());
        if is_periodical {
            strings.push(node.klass.clone().unwrap_or_default());
            if let Some(a) = &node.author {
                if !a.is_empty() {
                    strings.push(a.clone());
                }
            }
            if let Some(d) = &node.description {
                if !d.is_empty() {
                    strings.push(d.clone());
                }
            }
        }
    };
    for node in toc.iter_descendants(true) {
        collect(node, &mut strings);
    }
    strings
}

// }}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagx_blocks_have_the_expected_shape() {
        let flat = TagX::flat_book();
        assert_eq!(&flat[0..4], b"TAGX");
        // header: 4(ident)+4(len)+4(ctrl) + 5 tags * 4 bytes = 32
        assert_eq!(flat.len(), 32);
        let periodical = TagX::periodical();
        assert_eq!(&periodical[0..4], b"TAGX");
        let secondary = TagX::secondary();
        assert_eq!(&secondary[0..4], b"TAGX");
    }

    #[test]
    fn secondary_entries_are_in_descending_tag_order_with_masthead_first_flag() {
        let entries = secondary_entries();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].index, IndexValue::Str("author"));
        assert_eq!(entries[4].index, IndexValue::Str("mastheadImage"));
        assert_eq!(entries[4].secondary, Some([5, 0, 69]));
        assert_eq!(entries[0].secondary, Some([0, 0, 73]));
    }

    #[test]
    fn book_index_entry_bytestring_has_the_expected_minimum_shape() {
        let mut e = IndexEntry::new_book(100, 5);
        e.length = 50;
        e.index = IndexValue::Int(0);
        let bs = e.bytestring();
        // First byte(s): hex-encoded index "00" -> len=1,'0','0' = 3
        // bytes, then entry_type byte, then 4 encints (offset, size,
        // label_offset, depth).
        assert!(bs.len() > 5);
    }
}
