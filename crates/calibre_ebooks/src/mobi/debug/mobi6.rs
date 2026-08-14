//! MOBI6 structural dump: primary/secondary index, records, TBS.
//!
//! Port of `src/calibre/ebooks/mobi/debug/mobi6.py`. `IndexHeader` and
//! `SecondaryIndexHeader` here parse MOBI6's own primary/secondary
//! index record format directly — a different, older layout from the
//! generic `INDX` trees `debug::index` reads for KF8's SKEL/SECT/GUIDE
//! indices, hence the separate implementation rather than shared code.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::mobi::debug::format_bytes;
use crate::mobi::headers::NULL_INDEX;
use crate::mobi::index::{
    parse_index_record, parse_tagx_section, CNCXReader, IndexTable, TagX as ReaderTagX,
};
use crate::mobi::utils::{decode_hex_number, decode_tbs};

use super::headers::{MobiFile as RawMobiFile, Record, TextRecord};

fn be_u32(b: &[u8]) -> Result<u32> {
    b.get(..4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .context("truncated u32")
}
fn be_u16(b: &[u8]) -> Result<u16> {
    b.get(..2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .context("truncated u16")
}

/// One `TAGX` table entry. `TagX` in the Python (distinct shape from
/// `crate::mobi::index::TagX`, which this wraps).
pub struct TagX {
    pub tag: u8,
    pub num_values: u8,
    pub bitmask: u8,
    pub eof: u8,
    pub is_eof: bool,
}

impl From<&ReaderTagX> for TagX {
    fn from(t: &ReaderTagX) -> Self {
        TagX {
            tag: t.tag,
            num_values: t.num_of_values,
            bitmask: t.bitmask,
            eof: t.eof,
            is_eof: t.eof == 1 && t.tag == 0 && t.num_of_values == 0 && t.bitmask == 0,
        }
    }
}

impl fmt::Debug for TagX {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TAGX(tag={:02}, num_values={}, bitmask={:#b}, eof={})",
            self.tag, self.num_values, self.bitmask, self.eof
        )
    }
}

fn index_type_desc(t: u32) -> &'static str {
    match t {
        0 => "normal",
        2 => "inflection",
        6 => "calibre",
        _ => "unknown",
    }
}

fn index_encoding(n: u32) -> Result<&'static str> {
    match n {
        65001 => Ok("utf-8"),
        1252 => Ok("cp1252"),
        _ => bail!("Unknown index encoding: {n}"),
    }
}

/// MOBI6's secondary index header (record `secondary_index_record`).
/// `SecondaryIndexHeader` in the Python.
pub struct SecondaryIndexHeader {
    pub header_length: u32,
    pub unknown1: Vec<u8>,
    pub index_type: u32,
    pub idxt_start: u32,
    pub index_count: u32,
    pub index_encoding_num: u32,
    pub unknown2: Vec<u8>,
    pub num_index_entries: u32,
    pub ordt_start: u32,
    pub ligt_start: u32,
    pub num_of_ligt_entries: u32,
    pub num_of_cncx_blocks: u32,
    pub unknown3: Vec<u8>,
    pub tagx_offset: u32,
    pub unknown4: Vec<u8>,
    pub tagx_header_length: u32,
    pub tagx_control_byte_count: u32,
    pub tagx_entries: Vec<TagX>,
    pub last_entry: Vec<u8>,
    pub ncx_count: u16,
}

impl SecondaryIndexHeader {
    /// `SecondaryIndexHeader.__init__`.
    pub fn parse(record: &Record) -> Result<Self> {
        let raw = &record.raw;
        if !raw.starts_with(b"INDX") {
            bail!("Invalid Secondary Index Record");
        }
        let header_length = be_u32(&raw[4..8])?;
        let unknown1 = raw[8..16].to_vec();
        let index_type = be_u32(&raw[16..20])?;
        let idxt_start = be_u32(&raw[20..24])?;
        let index_count = be_u32(&raw[24..28])?;
        let index_encoding_num = be_u32(&raw[28..32])?;
        index_encoding(index_encoding_num)?;
        let unknown2 = raw[32..36].to_vec();
        let num_index_entries = be_u32(&raw[36..40])?;
        let ordt_start = be_u32(&raw[40..44])?;
        let ligt_start = be_u32(&raw[44..48])?;
        let num_of_ligt_entries = be_u32(&raw[48..52])?;
        let num_of_cncx_blocks = be_u32(&raw[52..56])?;
        let unknown3 = raw[56..180].to_vec();
        let tagx_offset = be_u32(&raw[180..184])?;
        if tagx_offset != header_length {
            bail!("TAGX offset and header length disagree");
        }
        let unknown4 = raw[184..header_length as usize].to_vec();

        let tagx = &raw[header_length as usize..];
        if !tagx.starts_with(b"TAGX") {
            bail!("Invalid TAGX section");
        }
        let tagx_header_length = be_u32(&tagx[4..8])?;
        let tagx_control_byte_count = be_u32(&tagx[8..12])?;
        let (_, reader_tags) = parse_tagx_section(tagx)?;
        let tagx_entries: Vec<TagX> = reader_tags.iter().map(TagX::from).collect();
        if let Some(last) = tagx_entries.last() {
            if !last.is_eof {
                bail!("TAGX last entry is not EOF");
            }
        }

        let idxt0_pos = header_length as usize + tagx_header_length as usize;
        let num = raw[idxt0_pos] as usize;
        let count_pos = idxt0_pos + 1 + num;
        let last_entry = raw[idxt0_pos + 1..count_pos].to_vec();
        let ncx_count = be_u16(&raw[count_pos..count_pos + 2])?;

        let idxt = &raw[idxt_start as usize..];
        if !idxt.starts_with(b"IDXT") {
            bail!("Invalid IDXT header");
        }
        let length_check = be_u16(&idxt[4..6])?;
        if u32::from(length_check) != header_length + tagx_header_length {
            bail!("Length check failed");
        }
        if idxt[6..].iter().any(|&b| b != 0) {
            bail!("Non null trailing bytes after IDXT");
        }

        Ok(SecondaryIndexHeader {
            header_length,
            unknown1,
            index_type,
            idxt_start,
            index_count,
            index_encoding_num,
            unknown2,
            num_index_entries,
            ordt_start,
            ligt_start,
            num_of_ligt_entries,
            num_of_cncx_blocks,
            unknown3,
            tagx_offset,
            unknown4,
            tagx_header_length,
            tagx_control_byte_count,
            tagx_entries,
            last_entry,
            ncx_count,
        })
    }

    pub fn index_encoding(&self) -> &'static str {
        index_encoding(self.index_encoding_num).unwrap_or("unknown")
    }
}

fn unknown_line(w: &[u8]) -> String {
    format!(
        "Unknown: {:?} ({} bytes) (All zeros: {:?})",
        String::from_utf8_lossy(w),
        w.len(),
        w.iter().all(|&b| b == 0)
    )
}

impl fmt::Display for SecondaryIndexHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} Secondary Index Header {}",
            "*".repeat(20),
            "*".repeat(20)
        )?;
        writeln!(f, "Header length: {}", self.header_length)?;
        writeln!(f, "{}", unknown_line(&self.unknown1))?;
        writeln!(
            f,
            "Index Type: {} ({})",
            index_type_desc(self.index_type),
            self.index_type
        )?;
        writeln!(f, "Offset to IDXT start: {}", self.idxt_start)?;
        writeln!(f, "Number of index records: {}", self.index_count)?;
        writeln!(
            f,
            "Index encoding: {} ({})",
            self.index_encoding(),
            self.index_encoding_num
        )?;
        writeln!(f, "{}", unknown_line(&self.unknown2))?;
        writeln!(f, "Number of index entries: {}", self.num_index_entries)?;
        writeln!(f, "ORDT start: {}", self.ordt_start)?;
        writeln!(f, "LIGT start: {}", self.ligt_start)?;
        writeln!(f, "Number of LIGT entries: {}", self.num_of_ligt_entries)?;
        writeln!(f, "Number of cncx blocks: {}", self.num_of_cncx_blocks)?;
        writeln!(f, "{}", unknown_line(&self.unknown3))?;
        writeln!(f, "TAGX offset: {}", self.tagx_offset)?;
        writeln!(f, "{}", unknown_line(&self.unknown4))?;
        writeln!(f, "\n")?;
        writeln!(
            f,
            "{} TAGX Header ({} bytes){}",
            "*".repeat(20),
            self.tagx_header_length,
            "*".repeat(20)
        )?;
        writeln!(f, "Header length: {}", self.tagx_header_length)?;
        writeln!(f, "Control byte count: {}", self.tagx_control_byte_count)?;
        for t in &self.tagx_entries {
            writeln!(f, "\t{t:?}")?;
        }
        writeln!(
            f,
            "Index of last IndexEntry in secondary index record: {:?}",
            self.last_entry
        )?;
        write!(f, "Number of entries in the NCX: {}", self.ncx_count)
    }
}

/// MOBI6's primary index header (record `primary_index_record`).
/// `IndexHeader` in the Python — note the name collision with
/// `crate::mobi::debug::index::Index`; these parse different formats.
pub struct Mobi6IndexHeader {
    pub record_len: usize,
    pub header_length: u32,
    pub unknown1: Vec<u8>,
    pub header_type: u32,
    pub index_type: u32,
    pub idxt_start: u32,
    pub index_count: u32,
    pub index_encoding_num: u32,
    pub possibly_language: Vec<u8>,
    pub num_index_entries: u32,
    pub ordt_start: u32,
    pub ligt_start: u32,
    pub num_of_ligt_entries: u32,
    pub num_of_cncx_blocks: u32,
    pub unknown2: Vec<u8>,
    pub tagx_offset: u32,
    pub unknown3: Vec<u8>,
    pub tagx_header_length: u32,
    pub tagx_control_byte_count: u32,
    pub tagx_entries: Vec<TagX>,
    pub last_entry: u64,
    pub ncx_count: u16,
}

impl Mobi6IndexHeader {
    /// `IndexHeader.__init__`.
    pub fn parse(record: &Record) -> Result<Self> {
        let raw = &record.raw;
        if !raw.starts_with(b"INDX") {
            bail!("Invalid Primary Index Record");
        }
        let header_length = be_u32(&raw[4..8])?;
        let unknown1 = raw[8..12].to_vec();
        let header_type = be_u32(&raw[12..16])?;
        let index_type = be_u32(&raw[16..20])?;
        let idxt_start = be_u32(&raw[20..24])?;
        let index_count = be_u32(&raw[24..28])?;
        let index_encoding_num = be_u32(&raw[28..32])?;
        index_encoding(index_encoding_num)?;
        let possibly_language = raw[32..36].to_vec();
        let num_index_entries = be_u32(&raw[36..40])?;
        let ordt_start = be_u32(&raw[40..44])?;
        let ligt_start = be_u32(&raw[44..48])?;
        let num_of_ligt_entries = be_u32(&raw[48..52])?;
        let num_of_cncx_blocks = be_u32(&raw[52..56])?;
        let unknown2 = raw[56..180].to_vec();
        let tagx_offset = be_u32(&raw[180..184])?;
        if tagx_offset != header_length {
            bail!("TAGX offset and header length disagree");
        }
        let unknown3 = raw[184..header_length as usize].to_vec();

        let tagx = &raw[header_length as usize..];
        if !tagx.starts_with(b"TAGX") {
            bail!("Invalid TAGX section");
        }
        let tagx_header_length = be_u32(&tagx[4..8])?;
        let tagx_control_byte_count = be_u32(&tagx[8..12])?;
        let (_, reader_tags) = parse_tagx_section(tagx)?;
        let tagx_entries: Vec<TagX> = reader_tags.iter().map(TagX::from).collect();
        if let Some(last) = tagx_entries.last() {
            if !last.is_eof {
                bail!("TAGX last entry is not EOF");
            }
        }

        let idxt0_pos = header_length as usize + tagx_header_length as usize;
        let (last_num, consumed) = decode_hex_number(&raw[idxt0_pos..])?;
        let count_pos = idxt0_pos + consumed;
        let ncx_count = be_u16(&raw[count_pos..count_pos + 2])?;
        if last_num != u64::from(ncx_count) - 1 {
            bail!("Last id number in the NCX != NCX count - 1");
        }

        let idxt = &raw[idxt_start as usize..];
        if !idxt.starts_with(b"IDXT") {
            bail!("Invalid IDXT header");
        }
        let length_check = be_u16(&idxt[4..6])?;
        if u32::from(length_check) != header_length + tagx_header_length {
            bail!("Length check failed");
        }

        Ok(Mobi6IndexHeader {
            record_len: raw.len(),
            header_length,
            unknown1,
            header_type,
            index_type,
            idxt_start,
            index_count,
            index_encoding_num,
            possibly_language,
            num_index_entries,
            ordt_start,
            ligt_start,
            num_of_ligt_entries,
            num_of_cncx_blocks,
            unknown2,
            tagx_offset,
            unknown3,
            tagx_header_length,
            tagx_control_byte_count,
            tagx_entries,
            last_entry: last_num,
            ncx_count,
        })
    }

    pub fn index_encoding(&self) -> &'static str {
        index_encoding(self.index_encoding_num).unwrap_or("unknown")
    }
}

impl fmt::Display for Mobi6IndexHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} Index Header ({} bytes){}",
            "*".repeat(20),
            self.record_len,
            "*".repeat(20)
        )?;
        writeln!(f, "Header length: {}", self.header_length)?;
        writeln!(f, "{}", unknown_line(&self.unknown1))?;
        writeln!(f, "Header type: {}", self.header_type)?;
        writeln!(
            f,
            "Index Type: {} ({})",
            index_type_desc(self.index_type),
            self.index_type
        )?;
        writeln!(f, "Offset to IDXT start: {}", self.idxt_start)?;
        writeln!(f, "Number of index records: {}", self.index_count)?;
        writeln!(
            f,
            "Index encoding: {} ({})",
            self.index_encoding(),
            self.index_encoding_num
        )?;
        writeln!(
            f,
            "Unknown (possibly language?): {:?}",
            self.possibly_language
        )?;
        writeln!(f, "Number of index entries: {}", self.num_index_entries)?;
        writeln!(f, "ORDT start: {}", self.ordt_start)?;
        writeln!(f, "LIGT start: {}", self.ligt_start)?;
        writeln!(f, "Number of LIGT entries: {}", self.num_of_ligt_entries)?;
        writeln!(f, "Number of cncx blocks: {}", self.num_of_cncx_blocks)?;
        writeln!(f, "{}", unknown_line(&self.unknown2))?;
        writeln!(f, "TAGX offset: {}", self.tagx_offset)?;
        writeln!(f, "{}", unknown_line(&self.unknown3))?;
        writeln!(f, "\n")?;
        writeln!(
            f,
            "{} TAGX Header ({} bytes){}",
            "*".repeat(20),
            self.tagx_header_length,
            "*".repeat(20)
        )?;
        writeln!(f, "Header length: {}", self.tagx_header_length)?;
        writeln!(f, "Control byte count: {}", self.tagx_control_byte_count)?;
        for t in &self.tagx_entries {
            writeln!(f, "\t{t:?}")?;
        }
        writeln!(
            f,
            "Index of last IndexEntry in primary index record: {}",
            self.last_entry
        )?;
        write!(f, "Number of entries in the NCX: {}", self.ncx_count)
    }
}

/// What a numeric tag in a MOBI6 index entry means. `Tag.TAG_MAP` in
/// the Python.
fn tag_desc(tag: u8) -> (&'static str, &'static str) {
    match tag {
        1 => ("offset", "Offset in HTML"),
        2 => ("size", "Size in HTML"),
        3 => ("label_offset", "Label offset in CNCX"),
        4 => ("depth", "Depth of this entry in TOC"),
        5 => ("class_offset", "Class offset in CNCX"),
        6 => ("pos_fid", "File Index"),
        11 => (
            "secondary",
            "[unknown, unknown, tag type from TAGX in primary index header]",
        ),
        21 => ("parent_index", "Parent"),
        22 => ("first_child_index", "First child"),
        23 => ("last_child_index", "Last child"),
        69 => (
            "image_index",
            "Offset from first image record to the image record associated with this entry \
             (masthead for periodical or thumbnail for article entry).",
        ),
        70 => ("desc_offset", "Description offset in cncx"),
        71 => ("author_offset", "Author offset in cncx"),
        72 => ("image_caption_offset", "Image caption offset in cncx"),
        73 => ("image_attr_offset", "Image attribution offset in cncx"),
        _ => ("unknown", ""),
    }
}

/// One tag within an [`IndexEntry`]. `Tag` in the Python.
pub struct Tag {
    pub tag_type: u8,
    pub attr: &'static str,
    pub desc: String,
    pub value: Vec<u64>,
    pub cncx_value: Option<String>,
}

impl Tag {
    /// `Tag.__init__`.
    pub fn new(tag_type: u8, vals: &[u64], cncx: &CNCXReader) -> Self {
        let (attr, desc) = tag_desc(tag_type);
        let desc = if attr == "unknown" && desc.is_empty() {
            format!("??Unknown (tag value: {tag_type})")
        } else {
            desc.to_string()
        };
        let cncx_value = if attr.ends_with("_offset") {
            vals.first().and_then(|&v| cncx.get(v as usize).cloned())
        } else {
            None
        };
        Tag {
            tag_type,
            attr,
            desc,
            value: vals.to_vec(),
            cncx_value,
        }
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.cncx_value {
            Some(v) => write!(f, "{} : {:?} [{:?}]", self.desc, self.value, v),
            None => write!(f, "{} : {:?}", self.desc, self.value),
        }
    }
}

/// One entry in a MOBI6 index. `IndexEntry` in the Python.
pub struct IndexEntry {
    /// The entry's identifier, parsed as hex when possible (matching
    /// `int(ident, 16)`), else kept as the original string.
    pub index: String,
    pub tags: Vec<Tag>,
}

impl IndexEntry {
    pub fn new(
        ident: &str,
        entry: &std::collections::BTreeMap<u8, Vec<u64>>,
        cncx: &CNCXReader,
    ) -> Self {
        let index = u64::from_str_radix(ident, 16)
            .map(|_| ident.to_string())
            .unwrap_or_else(|_| ident.to_string());
        let tags = entry.iter().map(|(&t, v)| Tag::new(t, v, cncx)).collect();
        IndexEntry { index, tags }
    }

    fn tag_value(&self, attr: &str) -> Option<&[u64]> {
        self.tags
            .iter()
            .find(|t| t.attr == attr)
            .map(|t| t.value.as_slice())
    }

    pub fn label(&self) -> String {
        self.tags
            .iter()
            .find(|t| t.attr == "label_offset")
            .and_then(|t| t.cncx_value.clone())
            .unwrap_or_default()
    }

    pub fn offset(&self) -> u64 {
        self.tag_value("offset")
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(0)
    }

    pub fn size(&self) -> u64 {
        self.tag_value("size")
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(0)
    }

    pub fn depth(&self) -> i64 {
        self.tag_value("depth")
            .and_then(|v| v.first())
            .map(|&v| v as i64)
            .unwrap_or(0)
    }

    pub fn parent_index(&self) -> i64 {
        self.tag_value("parent_index")
            .and_then(|v| v.first())
            .map(|&v| v as i64)
            .unwrap_or(-1)
    }

    pub fn first_child_index(&self) -> i64 {
        self.tag_value("first_child_index")
            .and_then(|v| v.first())
            .map(|&v| v as i64)
            .unwrap_or(-1)
    }

    pub fn last_child_index(&self) -> i64 {
        self.tag_value("last_child_index")
            .and_then(|v| v.first())
            .map(|&v| v as i64)
            .unwrap_or(-1)
    }
}

impl fmt::Display for IndexEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Index Entry(index={}, length={})",
            self.index,
            self.tags.len()
        )?;
        for t in &self.tags {
            writeln!(f, "\t{t}")?;
        }
        if self.first_child_index() != -1 {
            write!(
                f,
                "\tNumber of children: {}",
                self.last_child_index() - self.first_child_index() + 1
            )?;
        }
        Ok(())
    }
}

/// MOBI6's own indexing information (excluding trailing-data TBS
/// bytes). `IndexRecord` in the Python.
pub struct IndexRecord {
    pub indices: Vec<IndexEntry>,
    pub alltext: Option<Vec<u8>>,
}

impl IndexRecord {
    /// `IndexRecord.__init__`.
    pub fn new(records: &[&Record], header: &Mobi6IndexHeader, cncx: &CNCXReader) -> Result<Self> {
        let tags: Vec<ReaderTagX> = header
            .tagx_entries
            .iter()
            .map(|t| ReaderTagX {
                tag: t.tag,
                num_of_values: t.num_values,
                bitmask: t.bitmask,
                eof: t.eof,
            })
            .collect();
        let mut table: IndexTable = IndexTable::new();
        for record in records {
            if !record.raw.starts_with(b"INDX") {
                bail!("Invalid Primary Index Record");
            }
            parse_index_record(
                &mut table,
                &record.raw,
                header.tagx_control_byte_count,
                &tags,
                header.index_encoding(),
                "",
                true,
            )?;
        }
        let indices = table
            .iter()
            .map(|(ident, entry)| IndexEntry::new(ident, entry, cncx))
            .collect();
        Ok(IndexRecord {
            indices,
            alltext: None,
        })
    }
}

impl fmt::Display for IndexRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} Index Entries ({} entries) {}",
            "*".repeat(20),
            self.indices.len(),
            "*".repeat(20)
        )?;
        for entry in &self.indices {
            let offset = entry.offset();
            writeln!(f, "{entry}")?;
            if let Some(text) = &self.alltext {
                let before = slice_bytes(text, offset.saturating_sub(50), offset);
                let after = slice_bytes(text, offset, offset + 50);
                let end = offset + entry.size();
                let before_end = slice_bytes(text, end.saturating_sub(50), end);
                let after_end = slice_bytes(text, end, end + 50);
                writeln!(
                    f,
                    "\tHTML before offset: {:?}",
                    String::from_utf8_lossy(&before)
                )?;
                writeln!(
                    f,
                    "\tHTML after offset: {:?}",
                    String::from_utf8_lossy(&after)
                )?;
                writeln!(
                    f,
                    "\tHTML before end: {:?}",
                    String::from_utf8_lossy(&before_end)
                )?;
                writeln!(
                    f,
                    "\tHTML after end: {:?}",
                    String::from_utf8_lossy(&after_end)
                )?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

fn slice_bytes(data: &[u8], start: u64, end: u64) -> Vec<u8> {
    let start = (start as usize).min(data.len());
    let end = (end as usize).min(data.len());
    if start >= end {
        Vec::new()
    } else {
        data[start..end].to_vec()
    }
}

/// A record holding an embedded image. `ImageRecord` in the Python.
pub struct ImageRecord {
    pub idx: u32,
    pub raw: Vec<u8>,
    pub fmt: &'static str,
}

impl ImageRecord {
    pub fn dump(&self, folder: &Path) -> Result<()> {
        std::fs::write(
            folder.join(format!("{:06}.{}", self.idx, self.fmt)),
            &self.raw,
        )?;
        Ok(())
    }
}

const KNOWN_BINARY_SIGS: [&[u8; 4]; 12] = [
    b"FCIS", b"FLIS", b"SRCS", b"DATP", b"RESC", b"BOUN", b"FDST", b"AUDI", b"VIDE", b"CRES",
    b"CONT", b"CMET",
];

/// A record of unknown type, dumped verbatim. `BinaryRecord` in the
/// Python.
pub struct BinaryRecord {
    pub raw: Vec<u8>,
    pub name: String,
}

impl BinaryRecord {
    pub fn new(idx: u32, raw: Vec<u8>) -> Self {
        let sig = &raw[..raw.len().min(4)];
        let mut name = format!("{idx:06}");
        if sig.len() == 4 && KNOWN_BINARY_SIGS.iter().any(|s| s.as_slice() == sig) {
            name.push('-');
            name.push_str(&String::from_utf8_lossy(sig));
        } else if sig == b"\xe9\x8e\r\n" {
            name.push_str("-EOF");
        }
        BinaryRecord { raw, name }
    }

    pub fn dump(&self, folder: &Path) -> Result<()> {
        std::fs::write(folder.join(format!("{}.bin", self.name)), &self.raw)?;
        Ok(())
    }
}

/// A record holding an embedded (possibly obfuscated) font.
/// `FontRecord` in the Python.
pub struct FontRecord {
    pub payload: Vec<u8>,
    pub name: String,
}

impl FontRecord {
    pub fn new(idx: u32, raw: &[u8]) -> Result<Self> {
        let info =
            crate::mobi::utils::read_font_record(raw, crate::mobi::utils::DEFAULT_FONT_XOR_EXTENT);
        if let Some(err) = &info.err {
            bail!("Failed to read font record: {err}");
        }
        let payload = info.font_data.unwrap_or(info.raw_data);
        Ok(FontRecord {
            payload,
            name: format!("{idx:06}.{}", info.ext),
        })
    }

    pub fn dump(&self, folder: &Path) -> Result<()> {
        std::fs::write(folder.join(&self.name), &self.payload)?;
        Ok(())
    }
}

/// Per-text-record TBS (Trailing Byte Sequence) navigation indexing.
/// `TBSIndexing` in the Python.
///
/// `interpret_periodical`'s section/article-transition narration is
/// not reproduced: it only fires for periodical (newspaper/magazine)
/// MOBI files, and the Python itself wraps it in a try/except that
/// falls back to "Failed to decode" on any surprise. This port keeps
/// the same fallback for every periodical file rather than attempting
/// the narration, since regular books never reach that code path.
pub struct TbsIndexing<'a> {
    pub records: Vec<(&'a TextRecord, RecordTbs)>,
    pub doc_type: u32,
}

pub struct RecordTbs {
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
    pub complete: Vec<usize>,
    pub geom: (usize, usize),
}

impl<'a> TbsIndexing<'a> {
    /// `TBSIndexing.__init__`.
    pub fn new(text_records: &'a [TextRecord], indices: &[IndexEntry], doc_type: u32) -> Self {
        let mut pos = 0usize;
        let mut records = Vec::new();
        for r in text_records {
            let start = pos;
            pos += r.len();
            let end = pos - 1;
            let mut rt = RecordTbs {
                starts: Vec::new(),
                ends: Vec::new(),
                complete: Vec::new(),
                geom: (start, end),
            };
            for (i, entry) in indices.iter().enumerate() {
                let istart = entry.offset() as usize;
                let iend = istart + entry.size() as usize - 1;
                let has_start = istart >= start && istart <= end;
                let has_end = iend >= start && iend <= end;
                if has_start && has_end {
                    rt.complete.push(i);
                } else if has_start {
                    rt.starts.push(i);
                } else if has_end {
                    rt.ends.push(i);
                }
            }
            records.push((r, rt));
        }
        TbsIndexing { records, doc_type }
    }

    /// `TBSIndexing.dump_record` — returns `(tbs_type, lines)`.
    fn dump_record(
        &self,
        r: &TextRecord,
        dat: &RecordTbs,
        indices: &[IndexEntry],
    ) -> (u64, Vec<String>) {
        let mut ans = Vec::new();
        ans.push(format!(
            "\nRecord #{}: Starts at: {} Ends at: {}",
            r.idx, dat.geom.0, dat.geom.1
        ));
        let total = dat.starts.len() + dat.ends.len() + dat.complete.len();
        ans.push(format!(
            "\tContains: {total} index entries ({} ends, {} complete, {} starts)",
            dat.ends.len(),
            dat.complete.len(),
            dat.starts.len()
        ));
        let byts = r.trailing_data.get("indexing").cloned().unwrap_or_default();
        ans.push(format!("TBS bytes: {}", format_bytes(&byts)));
        for (label, list) in [
            ("Ends", &dat.ends),
            ("Complete", &dat.complete),
            ("Starts", &dat.starts),
        ] {
            if !list.is_empty() {
                ans.push(format!("\t{label}:"));
                for &i in list {
                    let x = &indices[i];
                    ans.push(format!(
                        "\t\tIndex Entry: {} (Parent index: {}, Depth: {}, Offset: {}, Size: {}) [{}]",
                        x.index,
                        x.parent_index(),
                        x.depth(),
                        x.offset(),
                        x.size(),
                        x.label()
                    ));
                }
            }
        }

        let mut tbs_type: u64 = 0;
        let is_periodical = matches!(self.doc_type, 257 | 258 | 259);
        let mut byts = byts;
        if !byts.is_empty() {
            match decode_tbs(&byts, 3) {
                Ok((outermost_index, extra, consumed)) => {
                    byts = byts[consumed..].to_vec();
                    for k in extra.keys() {
                        tbs_type |= *k as u64;
                    }
                    ans.push(format!("\nTBS: {tbs_type} ({tbs_type:04b})"));
                    ans.push(format!("Outermost index: {outermost_index}"));
                    ans.push(format!(
                        "Unknown extra start bytes: {:?}",
                        format_extra(&extra)
                    ));
                    if is_periodical {
                        ans.push(
                            "Periodical section/article transition decoding is not \
                             implemented in this port; showing raw bytes only."
                                .to_string(),
                        );
                    }
                    if !byts.is_empty() {
                        let hex: Vec<String> = byts.iter().map(|b| format!("{b:x}")).collect();
                        ans.push(format!("Remaining bytes: {}", hex.join(" ")));
                    }
                }
                Err(_) => {
                    ans.push("Failed to decode TBS bytes for this record".to_string());
                }
            }
        }
        ans.push(String::new());
        (tbs_type, ans)
    }

    /// `TBSIndexing.dump`.
    pub fn dump(&self, bdir: &Path, indices: &[IndexEntry]) -> Result<()> {
        let mut by_type: HashMap<u64, Vec<String>> = HashMap::new();
        for (r, dat) in &self.records {
            let (tbs_type, lines) = self.dump_record(r, dat, indices);
            if tbs_type == 0 {
                continue;
            }
            by_type.entry(tbs_type).or_default().extend(lines);
        }
        for (typ, lines) in by_type {
            std::fs::write(bdir.join(format!("tbs_type_{typ}.txt")), lines.join("\n"))?;
        }
        Ok(())
    }

    /// `TBSIndexing.__str__`.
    pub fn render(&self, indices: &[IndexEntry]) -> String {
        let mut out = format!(
            "{} TBS Indexing ({} records) {}\n",
            "*".repeat(20),
            self.records.len(),
            "*".repeat(20)
        );
        for (r, dat) in &self.records {
            let (_, lines) = self.dump_record(r, dat, indices);
            out.push_str(&lines.join("\n"));
            out.push('\n');
        }
        out
    }
}

fn format_extra(extra: &HashMap<u32, u64>) -> String {
    let mut parts: Vec<String> = extra
        .iter()
        .map(|(k, v)| format!("'{k:04b}': {v}"))
        .collect();
    parts.sort();
    format!("{{{}}}", parts.join(", "))
}

/// The whole parsed MOBI6 structure the debug tool dumps.
/// `MOBIFile` in `debug/mobi6.py`.
pub struct MobiFile {
    pub inner: RawMobiFile,
    pub index_header: Option<Mobi6IndexHeader>,
    pub cncx: CNCXReader,
    pub index_record: Option<IndexRecord>,
    pub secondary_index_header: Option<SecondaryIndexHeader>,
    pub secondary_index_record: Option<IndexRecord>,
    pub text_records: Vec<TextRecord>,
    pub image_records: Vec<ImageRecord>,
    pub binary_records: Vec<BinaryRecord>,
    pub font_records: Vec<FontRecord>,
}

impl MobiFile {
    /// `MOBIFile.__init__`.
    pub fn new(mut mf: RawMobiFile) -> Result<Self> {
        let indexing_record_nums: std::collections::HashSet<usize>;
        let mut index_header = None;
        let mut cncx = CNCXReader::new(&[], "utf-8");
        let mut index_record = None;

        let pir = mf.mobi_header.primary_index_record;
        let mut nums = std::collections::HashSet::new();
        if pir != NULL_INDEX {
            let header = Mobi6IndexHeader::parse(&mf.records[pir as usize])?;
            let numi = header.index_count;
            let cncx_start = pir as usize + 1 + numi as usize;
            let cncx_end = cncx_start + header.num_of_cncx_blocks as usize;
            let cncx_records: Vec<Vec<u8>> = mf.records
                [cncx_start.min(mf.records.len())..cncx_end.min(mf.records.len())]
                .iter()
                .map(|r| r.raw.clone())
                .collect();
            cncx = CNCXReader::new(&cncx_records, header.index_encoding());
            let entry_refs: Vec<&Record> = mf.records
                [(pir as usize + 1)..(pir as usize + 1 + numi as usize).min(mf.records.len())]
                .iter()
                .collect();
            index_record = Some(IndexRecord::new(&entry_refs, &header, &cncx)?);
            for i in pir as usize..cncx_end {
                nums.insert(i);
            }
            index_header = Some(header);
        }

        let mut secondary_index_header = None;
        let mut secondary_index_record = None;
        let sir = mf.mobi_header.secondary_index_record;
        if sir != NULL_INDEX {
            let header = SecondaryIndexHeader::parse(&mf.records[sir as usize])?;
            let numi = header.index_count;
            nums.insert(sir as usize);
            let entry_refs: Vec<&Record> = mf.records
                [(sir as usize + 1)..(sir as usize + 1 + numi as usize).min(mf.records.len())]
                .iter()
                .collect();
            // The secondary index shares field names via a small
            // adapter, since `IndexRecord::new` wants a
            // `Mobi6IndexHeader`; secondary and primary headers carry
            // the same tagx/encoding shape, so we build an equivalent.
            let adapted = Mobi6IndexHeader {
                record_len: 0,
                header_length: header.header_length,
                unknown1: Vec::new(),
                header_type: 0,
                index_type: header.index_type,
                idxt_start: header.idxt_start,
                index_count: header.index_count,
                index_encoding_num: header.index_encoding_num,
                possibly_language: Vec::new(),
                num_index_entries: header.num_index_entries,
                ordt_start: header.ordt_start,
                ligt_start: header.ligt_start,
                num_of_ligt_entries: header.num_of_ligt_entries,
                num_of_cncx_blocks: header.num_of_cncx_blocks,
                unknown2: Vec::new(),
                tagx_offset: header.tagx_offset,
                unknown3: Vec::new(),
                tagx_header_length: header.tagx_header_length,
                tagx_control_byte_count: header.tagx_control_byte_count,
                tagx_entries: header
                    .tagx_entries
                    .iter()
                    .map(|t| TagX {
                        tag: t.tag,
                        num_values: t.num_values,
                        bitmask: t.bitmask,
                        eof: t.eof,
                        is_eof: t.is_eof,
                    })
                    .collect(),
                last_entry: 0,
                ncx_count: header.ncx_count,
            };
            secondary_index_record = Some(IndexRecord::new(&entry_refs, &adapted, &cncx)?);
            for i in (sir as usize + 1)..(sir as usize + 1 + numi as usize) {
                nums.insert(i);
            }
            secondary_index_header = Some(header);
        }
        indexing_record_nums = nums;

        let ntr = mf.mobi_header.number_of_text_records as usize;
        let mut text_records = Vec::new();
        for r in 1..(mf.records.len().min(ntr + 1)) {
            let extra = mf.mobi_header.extra_data_flags;
            let raw = mf.records[r].raw.clone();
            let decompressed = mf.decompress_text6(&raw)?;
            text_records.push(TextRecord::new(r as u32, &raw, extra, decompressed)?);
        }

        let mut image_records = Vec::new();
        let mut binary_records = Vec::new();
        let mut font_records = Vec::new();
        let huffman_nums: std::collections::HashSet<u32> =
            mf.huffman_record_nums.iter().copied().collect();
        let fii = mf.mobi_header.first_image_index;
        let mut image_index = 0u32;
        let last = mf
            .mobi_header
            .last_resource_record
            .min(mf.records.len() as u32);
        for i in mf.mobi_header.first_resource_record..last {
            if indexing_record_nums.contains(&(i as usize)) || huffman_nums.contains(&i) {
                continue;
            }
            image_index += 1;
            let r = &mf.records[i as usize];
            let sig4 = &r.raw[..r.raw.len().min(4)];
            let is_container_sig = matches!(
                sig4,
                b"FLIS"
                    | b"FCIS"
                    | b"SRCS"
                    | b"RESC"
                    | b"BOUN"
                    | b"FDST"
                    | b"DATP"
                    | b"AUDI"
                    | b"VIDE"
                    | b"FONT"
                    | b"CRES"
                    | b"CONT"
                    | b"CMET"
            ) || sig4 == b"\xe9\x8e\r\n";
            let fmt = if i >= fii && !is_container_sig {
                calibre_utils::imghdr::what(&r.raw)
            } else {
                None
            };
            if let Some(fmt) = fmt {
                image_records.push(ImageRecord {
                    idx: image_index,
                    raw: r.raw.clone(),
                    fmt,
                });
            } else if sig4 == b"FONT" {
                font_records.push(FontRecord::new(i, &r.raw)?);
            } else {
                binary_records.push(BinaryRecord::new(i, r.raw.clone()));
            }
        }

        Ok(MobiFile {
            inner: mf,
            index_header,
            cncx,
            index_record,
            secondary_index_header,
            secondary_index_record,
            text_records,
            image_records,
            binary_records,
            font_records,
        })
    }

    /// `MOBIFile.print_header`.
    pub fn header_dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.inner.palmdb.to_string());
        out.push_str("\n\nRecord headers:\n");
        for (i, r) in self.inner.records.iter().enumerate() {
            out.push_str(&format!("{i:>6}. {}\n", r.header_line()));
        }
        out.push('\n');
        out.push_str(&self.inner.mobi_header.to_string());
        out
    }
}

/// `inspect_mobi` in `debug/mobi6.py`.
pub fn inspect_mobi(mf: RawMobiFile, ddir: &Path) -> Result<()> {
    let mut f = MobiFile::new(mf)?;
    std::fs::write(ddir.join("header.txt"), f.header_dump())?;

    let mut alltext = Vec::new();
    for rec in &f.text_records {
        alltext.extend_from_slice(&rec.raw);
    }
    std::fs::write(ddir.join("text.html"), &alltext)?;
    // `pretty.html` (lxml pretty-printed HTML) is not reproduced: this
    // port has no HTML pretty-printer wired up for the raw MOBI6
    // stream encoding, and it's a convenience view, not a source of
    // structural information the plain `text.html` dump lacks.

    if let Some(index_header) = &f.index_header {
        if let Some(index_record) = f.index_record.as_mut() {
            index_record.alltext = Some(alltext.clone());
        }
        let mut out = String::new();
        out.push_str(&index_header.to_string());
        out.push_str("\n\n\n");
        if let Some(sih) = &f.secondary_index_header {
            out.push_str(&sih.to_string());
            out.push_str("\n\n\n");
        }
        if let Some(sir) = &f.secondary_index_record {
            out.push_str(&sir.to_string());
            out.push_str("\n\n\n");
        }
        out.push_str(&f.cncx_display());
        out.push_str("\n\n\n");
        if let Some(ir) = &f.index_record {
            out.push_str(&ir.to_string());
        }
        std::fs::write(ddir.join("index.txt"), out)?;

        if let Some(ir) = &f.index_record {
            let tbs = TbsIndexing::new(
                &f.text_records,
                &ir.indices,
                f.inner.mobi_header.type_.parse().unwrap_or(0),
            );
            std::fs::write(ddir.join("tbs_indexing.txt"), tbs.render(&ir.indices))?;
            tbs.dump(ddir, &ir.indices)?;
        }
    }

    for (subdir, count) in [
        ("text", f.text_records.len()),
        ("images", f.image_records.len()),
        ("binary", f.binary_records.len()),
        ("font", f.font_records.len()),
    ] {
        let d = ddir.join(subdir);
        std::fs::create_dir_all(&d)?;
        let _ = count;
    }
    for rec in &f.text_records {
        rec.dump(&ddir.join("text"))?;
    }
    for rec in &f.image_records {
        rec.dump(&ddir.join("images"))?;
    }
    for rec in &f.binary_records {
        rec.dump(&ddir.join("binary"))?;
    }
    for rec in &f.font_records {
        rec.dump(&ddir.join("font"))?;
    }

    Ok(())
}

impl MobiFile {
    /// `str(f.cncx)` — the `CNCX.__str__` the Python renders inline;
    /// [`CNCXReader`] doesn't implement `Display` itself since it's
    /// shared with the production reader.
    fn cncx_display(&self) -> String {
        let mut out = format!(
            "{} cncx ({} strings) {}\n",
            "*".repeat(20),
            self.cncx.records.len(),
            "*".repeat(20)
        );
        for (k, v) in &self.cncx.records {
            out.push_str(&format!("{k:>10} : {v}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_desc_matches_known_and_unknown_types() {
        assert_eq!(tag_desc(1).0, "offset");
        assert_eq!(tag_desc(255).0, "unknown");
    }

    #[test]
    fn binary_record_names_include_known_signatures() {
        let r = BinaryRecord::new(5, b"FDSTdata".to_vec());
        assert_eq!(r.name, "000005-FDST");
        let r = BinaryRecord::new(6, b"\xff\xff\xff\xffplain".to_vec());
        assert_eq!(r.name, "000006");
    }

    #[test]
    fn index_type_desc_matches_known_and_unknown_types() {
        assert_eq!(index_type_desc(0), "normal");
        assert_eq!(index_type_desc(2), "inflection");
        assert_eq!(index_type_desc(6), "calibre");
        assert_eq!(index_type_desc(99), "unknown");
    }
}
