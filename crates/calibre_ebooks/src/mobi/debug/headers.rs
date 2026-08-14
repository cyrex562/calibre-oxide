//! Low-level PalmDB/MOBI/EXTH header dumping.
//!
//! Port of `src/calibre/ebooks/mobi/debug/headers.py`. Everything here
//! parses raw bytes directly rather than going through
//! `crate::mobi::headers` (the production reader): the two exist for
//! different purposes, and this one needs fields — DRM data, extra
//! data flags, the FCIS/FLIS/SRCS bookkeeping — the reader has no use
//! for and doesn't keep.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, TimeZone, Utc};

use crate::mobi::debug::format_bytes;
use crate::mobi::headers::NULL_INDEX;
use crate::mobi::langcodes::{main_language, sub_language};
use crate::mobi::utils::get_trailing_data;

fn be_u16(b: &[u8]) -> Result<u16> {
    b.get(..2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .context("truncated u16")
}

fn be_u32(b: &[u8]) -> Result<u32> {
    b.get(..4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .context("truncated u32")
}

// PalmDB. {{{

/// One flag in [`PalmDocAttributes`]. `PalmDOCAttributes.Attr` in the
/// Python.
pub struct Attr {
    pub name: &'static str,
    pub val: bool,
}

impl fmt::Display for Attr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.val)
    }
}

/// The PalmDB attributes bitfield. `PalmDOCAttributes` in the Python.
pub struct PalmDocAttributes {
    pub val: u16,
    pub attributes: Vec<Attr>,
}

impl PalmDocAttributes {
    /// `PalmDOCAttributes.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let val = be_u16(raw)?;
        const FIELDS: [(&str, u16); 6] = [
            ("Read Only", 0x02),
            ("Dirty AppInfoArea", 0x04),
            ("Backup this database", 0x08),
            (
                "Okay to install newer over existing copy, if present on PalmPilot",
                0x10,
            ),
            (
                "Force the PalmPilot to reset after this database is installed",
                0x12,
            ),
            ("Don't allow copy of file to be beamed to other Pilot", 0x14),
        ];
        let attributes = FIELDS
            .iter()
            .map(|(name, field)| Attr {
                name,
                val: (val & field) != 0,
            })
            .collect();
        Ok(PalmDocAttributes { val, attributes })
    }
}

impl fmt::Display for PalmDocAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "PalmDOC Attributes: {:#b}", self.val)?;
        let lines: Vec<String> = self.attributes.iter().map(|a| a.to_string()).collect();
        write!(f, "\t{}", lines.join("\n\t"))
    }
}

/// The PalmDB (`.pdb`) container header every MOBI file starts with.
/// `PalmDB` in the Python.
pub struct PalmDb {
    pub name: String,
    pub attributes: PalmDocAttributes,
    pub version: u16,
    pub creation_date_raw: u32,
    pub creation_date: DateTime<Utc>,
    pub modification_date_raw: u32,
    pub modification_date: DateTime<Utc>,
    pub last_backup_date_raw: u32,
    pub last_backup_date: DateTime<Utc>,
    pub modification_number: u32,
    pub app_info_id: Vec<u8>,
    pub sort_info_id: Vec<u8>,
    pub type_: Vec<u8>,
    pub creator: Vec<u8>,
    pub ident: Vec<u8>,
    pub last_record_uid: u32,
    pub next_rec_list_id: Vec<u8>,
    pub number_of_records: u16,
}

/// Palm epoch: 1904-01-01 UTC, seconds since which every PalmDB
/// timestamp is counted.
fn palm_epoch_plus(seconds: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1904, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::seconds(seconds as i64)
}

impl PalmDb {
    /// `PalmDB.__init__`. `raw` is the first 78 bytes of the file.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if raw.len() < 78 {
            bail!("Truncated PalmDB header");
        }
        if raw.starts_with(b"TPZ") {
            bail!("This is a Topaz file");
        }
        let name = String::from_utf8_lossy(raw[..32].split(|&b| b == 0).next().unwrap_or(&[]))
            .into_owned();
        let attributes = PalmDocAttributes::parse(&raw[32..34])?;
        let version = be_u16(&raw[34..36])?;

        let creation_date_raw = be_u32(&raw[36..40])?;
        let modification_date_raw = be_u32(&raw[40..44])?;
        let last_backup_date_raw = be_u32(&raw[44..48])?;
        let modification_number = be_u32(&raw[48..52])?;
        let app_info_id = raw[52..56].to_vec();
        let sort_info_id = raw[56..60].to_vec();
        let type_ = raw[60..64].to_vec();
        let creator = raw[64..68].to_vec();
        let mut ident = type_.clone();
        ident.extend_from_slice(&creator);
        if ident != b"BOOKMOBI" && ident != b"TEXTREAD" {
            bail!("Unknown book ident: {:?}", String::from_utf8_lossy(&ident));
        }
        let last_record_uid = be_u32(&raw[68..72])?;
        let next_rec_list_id = raw[72..76].to_vec();
        let number_of_records = be_u16(&raw[76..78])?;

        Ok(PalmDb {
            name,
            attributes,
            version,
            creation_date_raw,
            creation_date: palm_epoch_plus(creation_date_raw),
            modification_date_raw,
            modification_date: palm_epoch_plus(modification_date_raw),
            last_backup_date_raw,
            last_backup_date: palm_epoch_plus(last_backup_date_raw),
            modification_number,
            app_info_id,
            sort_info_id,
            type_,
            creator,
            ident,
            last_record_uid,
            next_rec_list_id,
            number_of_records,
        })
    }
}

impl fmt::Display for PalmDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} PalmDB Header {}", "*".repeat(20), "*".repeat(20))?;
        writeln!(f, "Name: {:?}", self.name)?;
        writeln!(f, "{}", self.attributes)?;
        writeln!(f, "Version: {}", self.version)?;
        writeln!(
            f,
            "Creation date: {} ({})",
            self.creation_date.to_rfc3339(),
            self.creation_date_raw
        )?;
        writeln!(
            f,
            "Modification date: {} ({})",
            self.modification_date.to_rfc3339(),
            self.modification_date_raw
        )?;
        writeln!(
            f,
            "Backup date: {} ({})",
            self.last_backup_date.to_rfc3339(),
            self.last_backup_date_raw
        )?;
        writeln!(f, "Modification number: {}", self.modification_number)?;
        writeln!(f, "App Info ID: {:?}", self.app_info_id)?;
        writeln!(f, "Sort Info ID: {:?}", self.sort_info_id)?;
        writeln!(f, "Type: {:?}", self.type_)?;
        writeln!(f, "Creator: {:?}", self.creator)?;
        writeln!(f, "Last record UID +1: {:?}", self.last_record_uid)?;
        writeln!(f, "Next record list id: {:?}", self.next_rec_list_id)?;
        write!(f, "Number of records: {}", self.number_of_records)
    }
}
// }}}

/// One PalmDB record: its header triple (offset, flags, uid) plus its
/// raw bytes. `Record` in the Python.
pub struct Record {
    pub offset: u32,
    pub flags: u8,
    pub uid: u32,
    pub raw: Vec<u8>,
}

impl Record {
    pub fn new(raw: Vec<u8>, header: (u32, u8, u32)) -> Self {
        Record {
            offset: header.0,
            flags: header.1,
            uid: header.2,
            raw,
        }
    }

    /// `Record.header` — a one-line summary, used for the record table
    /// dump.
    pub fn header_line(&self) -> String {
        let first4 = &self.raw[..self.raw.len().min(4)];
        format!(
            "Offset: {} Flags: {} UID: {} First 4 bytes: {:?} Size: {}",
            self.offset,
            self.flags,
            self.uid,
            String::from_utf8_lossy(first4),
            self.raw.len()
        )
    }
}

// EXTH. {{{

/// The name calibre uses for each known EXTH record type.
/// `EXTHRecord.name`'s lookup table in the Python.
fn exth_name(type_: u32) -> String {
    let name: Option<&str> = match type_ {
        1 => Some("Drm Server Id"),
        2 => Some("Drm Commerce Id"),
        3 => Some("Drm Ebookbase Book Id"),
        100 => Some("Creator"),
        101 => Some("Publisher"),
        102 => Some("Imprint"),
        103 => Some("Description"),
        104 => Some("ISBN"),
        105 => Some("Subject"),
        106 => Some("Published"),
        107 => Some("Review"),
        108 => Some("Contributor"),
        109 => Some("Rights"),
        110 => Some("SubjectCode"),
        111 => Some("Type"),
        112 => Some("Source"),
        113 => Some("ASIN"),
        114 => Some("versionNumber"),
        115 => Some("sample"),
        116 => Some("StartOffset"),
        117 => Some("Adult"),
        118 => Some("Price"),
        119 => Some("Currency"),
        121 => Some("KF8_Boundary_Section"),
        122 => Some("fixed-layout"),
        123 => Some("book-type"),
        124 => Some("orientation-lock"),
        125 => Some("KF8_Count_of_Resources_Fonts_Images"),
        126 => Some("original-resolution"),
        127 => Some("zero-gutter"),
        128 => Some("zero-margin"),
        129 => Some("KF8_Masthead/Cover_Image"),
        131 => Some("KF8_Unidentified_Count"),
        132 => Some("RegionMagnification"),
        200 => Some("DictShortName"),
        201 => Some("CoverOffset"),
        202 => Some("ThumbOffset"),
        203 => Some("Fake Cover"),
        204 => Some("Creator Software"),
        205 => Some("Creator Major Version"),
        206 => Some("Creator Minor Version"),
        207 => Some("Creator Build Number"),
        208 => Some("Watermark"),
        209 => Some("Tamper Proof Keys [hex]"),
        300 => Some("Font Signature [hex]"),
        301 => Some("Clipping Limit [3xx]"),
        401 => Some("Clipping Limit"),
        402 => Some("Publisher Limit"),
        404 => Some("Text to Speech Disabled"),
        501 => Some("CDE Type"),
        502 => Some("last_update_time"),
        503 => Some("Updated Title"),
        504 => Some("ASIN [5xx]"),
        508 => Some("Unknown Title Furigana?"),
        517 => Some("Unknown Creator Furigana?"),
        522 => Some("Unknown Publisher Furigana?"),
        524 => Some("Language"),
        525 => Some("primary-writing-mode"),
        527 => Some("page-progression-direction"),
        528 => Some("Override Kindle fonts"),
        534 => Some("Input Source Type"),
        535 => Some("Kindlegen Build-Rev Number"),
        536 => Some("Container Info"),
        538 => Some("Container Resolution"),
        539 => Some("Container Mimetype"),
        543 => Some("Container id"),
        _ => None,
    };
    name.map(str::to_string)
        .unwrap_or_else(|| type_.to_string())
}

/// A single EXTH metadata record. `EXTHRecord` in the Python.
///
/// `data` holds whichever representation the type calls for: the raw
/// bytes for most records, or the decoded integer/hex text for the
/// numeric and hex-dump types, matching `EXTHRecord.__init__`'s
/// in-place reinterpretation of `self.data`.
pub struct ExthRecord {
    pub type_: u32,
    pub name: String,
    pub length: u32,
    pub data: ExthValue,
}

/// The decoded value of an EXTH record, after `EXTHRecord.__init__`'s
/// type-specific reinterpretation.
pub enum ExthValue {
    Bytes(Vec<u8>),
    Number(u64),
    Hex(String),
}

impl fmt::Debug for ExthValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExthValue::Bytes(b) => write!(f, "{:?}", String::from_utf8_lossy(b)),
            ExthValue::Number(n) => write!(f, "{n}"),
            ExthValue::Hex(h) => write!(f, "b'{h}'"),
        }
    }
}

/// Record types whose payload is a big-endian integer, not text.
/// `EXTHRecord.__init__`'s first `if` branch in the Python.
const NUMERIC_EXTH_TYPES: [u32; 21] = [
    115, 116, 201, 202, 203, 204, 205, 206, 207, 401, 402, 404, 121, 125, 131, 117, 118, 119, 114,
    112, 111,
];

/// Record types whose payload is hex-dumped rather than shown as text.
const HEX_EXTH_TYPES: [u32; 2] = [209, 300];

impl ExthRecord {
    /// `EXTHRecord.__init__`.
    pub fn new(type_: u32, data: &[u8], length: u32) -> Self {
        let name = exth_name(type_);
        let value = if NUMERIC_EXTH_TYPES.contains(&type_) {
            let n = match length {
                9 => data.first().map(|b| *b as u64).unwrap_or(0),
                10 => data
                    .get(..2)
                    .map(|s| u16::from_be_bytes([s[0], s[1]]) as u64)
                    .unwrap_or(0),
                _ => data
                    .get(..4)
                    .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as u64)
                    .unwrap_or(0),
            };
            ExthValue::Number(n)
        } else if HEX_EXTH_TYPES.contains(&type_) {
            ExthValue::Hex(format_bytes(data).replace(' ', ""))
        } else {
            ExthValue::Bytes(data.to_vec())
        };
        ExthRecord {
            type_,
            name,
            length,
            data: value,
        }
    }
}

impl fmt::Display for ExthRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {:?}", self.name, self.type_, self.data)
    }
}

/// The EXTH extended-metadata header. `EXTHHeader` in the Python.
pub struct ExthHeader {
    pub length: u32,
    pub count: u32,
    pub records: Vec<ExthRecord>,
}

impl ExthHeader {
    /// `EXTHHeader.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if !raw.starts_with(b"EXTH") {
            bail!("EXTH header does not start with EXTH");
        }
        let length = be_u32(&raw[4..8])?;
        let count = be_u32(&raw[8..12])?;
        let mut pos = 12usize;
        let mut records = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let type_ = be_u32(&raw[pos..pos + 4])?;
            let rec_len = be_u32(&raw[pos + 4..pos + 8])?;
            let data = &raw[pos + 8..pos + rec_len as usize];
            records.push(ExthRecord::new(type_, data, rec_len));
            pos += rec_len as usize;
        }
        records.sort_by_key(|r| r.type_);
        Ok(ExthHeader {
            length,
            count,
            records,
        })
    }

    /// `EXTHHeader.get` — the first record of a type, if any.
    pub fn get(&self, type_: u32) -> Option<&ExthRecord> {
        self.records.iter().find(|r| r.type_ == type_)
    }

    /// `EXTHHeader.kf8_header_index`.
    pub fn kf8_header_index(&self) -> Option<u32> {
        match self.get(121)?.data {
            ExthValue::Number(n) if n as u32 != NULL_INDEX => Some(n as u32),
            _ => None,
        }
    }
}

impl fmt::Display for ExthHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} EXTH Header {}", "*".repeat(20), "*".repeat(20))?;
        writeln!(f, "EXTH header length: {}", self.length)?;
        writeln!(f, "Number of EXTH records: {}", self.count)?;
        writeln!(f, "EXTH records...")?;
        for (i, r) in self.records.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{r}")?;
        }
        Ok(())
    }
}
// }}}

/// A relative-to-absolute record pointer in [`MobiHeader`], along with
/// the offset it was made relative to. `relative_records` handling in
/// the Python's `__init__`.
fn make_absolute(offset: u32, header_offset: u32) -> u32 {
    if offset == NULL_INDEX {
        offset
    } else {
        header_offset + offset
    }
}

/// A parsed MOBI (record 0, or the KF8 boundary record) header.
/// `MOBIHeader` in the Python.
///
/// Every field that the Python's `relative_records` set adjusts is
/// already stored absolute here; [`MobiHeader::render_relative`] shows
/// both forms, as `MOBIHeader.__str__`'s `r()` helper does.
pub struct MobiHeader {
    pub raw: Vec<u8>,
    pub header_offset: u32,
    pub compression: String,
    pub unused: Vec<u8>,
    pub text_length: u32,
    pub number_of_text_records: u16,
    pub text_record_size: u16,
    pub encryption_type: String,
    pub unknown: Vec<u8>,
    pub identifier: Vec<u8>,
    pub length: u32,
    pub type_: String,
    pub encoding: String,
    pub uid: Vec<u8>,
    pub file_version: u32,
    pub meta_orth_indx: u32,
    pub meta_infl_indx: u32,
    pub secondary_index_record: u32,
    pub reserved: Vec<u8>,
    pub first_non_book_record: u32,
    pub fullname_offset: u32,
    pub fullname_length: u32,
    pub locale_raw: u32,
    pub language: &'static str,
    pub sublanguage: &'static str,
    pub input_language: Vec<u8>,
    pub output_language: Vec<u8>,
    pub min_version: u32,
    pub first_image_index: u32,
    pub huffman_record_offset: u32,
    pub huffman_record_count: u32,
    pub datp_record_offset: u32,
    pub datp_record_count: u32,
    pub exth_flags: u32,
    pub has_exth: bool,
    pub has_drm_data: bool,
    pub unknown3: Vec<u8>,
    pub drm_offset: u32,
    pub drm_count: u32,
    pub drm_size: u32,
    pub drm_flags: u32,
    pub has_extra_data_flags: bool,
    pub unknown4: Vec<u8>,
    pub first_text_record: Option<u16>,
    pub last_text_record: Option<u16>,
    pub fdst_idx: u32,
    pub fdst_count: u32,
    pub fcis_number: u32,
    pub fcis_count: u32,
    pub flis_number: u32,
    pub flis_count: u32,
    pub unknown6: Vec<u8>,
    pub srcs_record_index: u32,
    pub num_srcs_records: u32,
    pub unknown7: Vec<u8>,
    pub extra_data_flags: u32,
    pub has_multibytes: bool,
    pub has_indexing_bytes: bool,
    pub has_uncrossable_breaks: bool,
    pub primary_index_record: u32,
    pub sect_idx: u32,
    pub skel_idx: u32,
    pub datp_idx: u32,
    pub oth_idx: u32,
    pub unknown9: Vec<u8>,
    pub first_resource_record: u32,
    pub last_resource_record: u32,
    pub exth_offset: usize,
    pub exth: Option<ExthHeader>,
    pub end_of_exth: usize,
    pub bytes_after_exth: Vec<u8>,
}

impl MobiHeader {
    /// `MOBIHeader.__init__`. `raw` is `record0.raw`; `offset` is that
    /// record's index (0 for the MOBI6 header, or the KF8 boundary
    /// record's index for a joint file's MOBI8 header).
    pub fn parse(raw: &[u8], offset: u32) -> Result<Self> {
        let compression_code = be_u16(&raw[0..2])?;
        let compression = match compression_code {
            1 => "No compression".to_string(),
            2 => "PalmDoc compression".to_string(),
            17480 => "HUFF/CDIC compression".to_string(),
            _ => format!("{:?}", &raw[0..2]),
        };
        let unused = raw[2..4].to_vec();
        let text_length = be_u32(&raw[4..8])?;
        let number_of_text_records = be_u16(&raw[8..10])?;
        let text_record_size = be_u16(&raw[10..12])?;
        let encryption_type_raw = be_u16(&raw[12..14])?;
        let encryption_type = match encryption_type_raw {
            0 => "No encryption".to_string(),
            1 => "Old mobipocket encryption".to_string(),
            2 => "Mobipocket encryption".to_string(),
            other => other.to_string(),
        };
        let unknown = raw[14..16].to_vec();
        let identifier = raw[16..20].to_vec();
        if identifier != b"MOBI" {
            bail!(
                "Identifier {:?} unknown",
                String::from_utf8_lossy(&identifier)
            );
        }
        let length = be_u32(&raw[20..24])?;
        let type_raw = be_u32(&raw[24..28])?;
        let type_ = match type_raw {
            2 => "Mobipocket book".to_string(),
            3 => "PalmDOC book".to_string(),
            4 => "Audio".to_string(),
            257 => "News".to_string(),
            258 => "News Feed".to_string(),
            259 => "News magazine".to_string(),
            513 => "PICS".to_string(),
            514 => "Word".to_string(),
            515 => "XLS".to_string(),
            516 => "PPT".to_string(),
            517 => "TEXT".to_string(),
            518 => "HTML".to_string(),
            other => other.to_string(),
        };
        let encoding_raw = be_u32(&raw[28..32])?;
        let encoding = match encoding_raw {
            1252 => "cp1252".to_string(),
            65001 => "utf-8".to_string(),
            other => other.to_string(),
        };
        let uid = raw[32..36].to_vec();
        let file_version = be_u32(&raw[36..40])?;
        let meta_orth_indx = be_u32(&raw[40..44])?;
        let meta_infl_indx = be_u32(&raw[44..48])?;
        let secondary_index_record = be_u32(&raw[48..52])?;
        let reserved = raw[52..80].to_vec();
        let first_non_book_record = be_u32(&raw[80..84])?;
        let fullname_offset = be_u32(&raw[84..88])?;
        let fullname_length = be_u32(&raw[88..92])?;
        let locale_raw = be_u32(&raw[92..96])?;
        let langid = (locale_raw & 0xFF) as u8;
        let sublangid = ((locale_raw >> 10) & 0xFF) as u8;
        let language = main_language(langid);
        let sublanguage = sub_language(sublangid);
        let input_language = raw[96..100].to_vec();
        let output_language = raw[100..104].to_vec();
        let min_version = be_u32(&raw[104..108])?;
        let first_image_index = be_u32(&raw[108..112])?;
        let huffman_record_offset = be_u32(&raw[112..116])?;
        let huffman_record_count = be_u32(&raw[116..120])?;
        let datp_record_offset = be_u32(&raw[120..124])?;
        let datp_record_count = be_u32(&raw[124..128])?;
        let exth_flags = be_u32(&raw[128..132])?;
        let has_exth = exth_flags & 0x40 != 0;
        let has_drm_data = length >= 174 && raw.len() >= 184;

        let (mut unknown3, mut drm_offset, mut drm_count, mut drm_size, mut drm_flags) =
            (Vec::new(), 0, 0, 0, 0);
        if has_drm_data {
            unknown3 = raw[132..168].to_vec();
            drm_offset = be_u32(&raw[168..172])?;
            drm_count = be_u32(&raw[172..176])?;
            drm_size = be_u32(&raw[176..180])?;
            drm_flags = be_u32(&raw[180..184])?;
        }

        let has_extra_data_flags = length >= 232 && raw.len() >= 232 + 16;
        let mut unknown4 = Vec::new();
        let (mut first_text_record, mut last_text_record) = (None, None);
        let (mut fdst_idx, mut fdst_count) = (NULL_INDEX, 0);
        let (mut fcis_number, mut fcis_count, mut flis_number, mut flis_count) = (0, 0, 0, 0);
        let mut unknown6 = Vec::new();
        let (mut srcs_record_index, mut num_srcs_records) = (0, 0);
        let mut unknown7 = Vec::new();
        let mut extra_data_flags = 0u32;
        let (mut has_multibytes, mut has_indexing_bytes, mut has_uncrossable_breaks) =
            (false, false, false);
        let mut primary_index_record = NULL_INDEX;

        if has_extra_data_flags {
            unknown4 = raw[184..192].to_vec();
            if file_version < 8 {
                first_text_record = Some(be_u16(&raw[192..194])?);
                last_text_record = Some(be_u16(&raw[194..196])?);
                fdst_count = be_u32(&raw[196..200])?;
            } else {
                fdst_idx = be_u32(&raw[192..196])?;
                fdst_count = be_u32(&raw[196..200])?;
                if fdst_count <= 1 {
                    fdst_idx = NULL_INDEX;
                }
            }
            fcis_number = be_u32(&raw[200..204])?;
            fcis_count = be_u32(&raw[204..208])?;
            flis_number = be_u32(&raw[208..212])?;
            flis_count = be_u32(&raw[212..216])?;
            unknown6 = raw[216..224].to_vec();
            srcs_record_index = be_u32(&raw[224..228])?;
            num_srcs_records = be_u32(&raw[228..232])?;
            unknown7 = raw[232..240].to_vec();
            extra_data_flags = be_u32(&raw[240..244])?;
            has_multibytes = extra_data_flags & 0b1 != 0;
            has_indexing_bytes = extra_data_flags & 0b10 != 0;
            has_uncrossable_breaks = extra_data_flags & 0b100 != 0;
            primary_index_record = be_u32(&raw[244..248])?;
        }

        let (mut sect_idx, mut skel_idx, mut datp_idx, mut oth_idx) =
            (NULL_INDEX, NULL_INDEX, NULL_INDEX, NULL_INDEX);
        let mut unknown9 = Vec::new();
        if length >= 248 {
            sect_idx = be_u32(&raw[248..252])?;
            skel_idx = be_u32(&raw[252..256])?;
            datp_idx = be_u32(&raw[256..260])?;
            oth_idx = be_u32(&raw[260..264])?;
            let end = (length as usize + 16).min(raw.len());
            if end > 264 {
                unknown9 = raw[264..end].to_vec();
            }
            if meta_orth_indx != NULL_INDEX && meta_orth_indx != sect_idx {
                bail!("KF8 header has different Meta orth and section indices");
            }
        }

        // Relative-to-absolute, as `relative_records` does in the
        // Python.
        let meta_orth_indx = make_absolute(meta_orth_indx, offset);
        let huffman_record_offset = make_absolute(huffman_record_offset, offset);
        let first_non_book_record = make_absolute(first_non_book_record, offset);
        let datp_record_offset = make_absolute(datp_record_offset, offset);
        let fcis_number = make_absolute(fcis_number, offset);
        let flis_number = make_absolute(flis_number, offset);
        let primary_index_record = make_absolute(primary_index_record, offset);
        let fdst_idx = make_absolute(fdst_idx, offset);
        let first_image_index = make_absolute(first_image_index, offset);
        let sect_idx = make_absolute(sect_idx, offset);
        let skel_idx = make_absolute(skel_idx, offset);
        let datp_idx = make_absolute(datp_idx, offset);
        let oth_idx = make_absolute(oth_idx, offset);

        // First non-text record: default to the record after all text
        // records, then widen to cover any resource pointer we found.
        let mut first_resource_record = offset + 1 + u32::from(number_of_text_records);
        let pointer = first_non_book_record.min(first_image_index);
        if pointer != NULL_INDEX {
            first_resource_record = pointer.max(first_resource_record);
        }
        let mut last_resource_record = NULL_INDEX;

        let (mut exth_offset, mut exth, mut end_of_exth, mut bytes_after_exth) =
            (0usize, None, 0usize, Vec::new());
        if has_exth {
            let eoff = 16 + length as usize;
            exth_offset = eoff;
            let parsed = ExthHeader::parse(&raw[eoff..])?;
            end_of_exth = eoff + parsed.length as usize;
            bytes_after_exth = raw
                .get(end_of_exth..fullname_offset as usize)
                .unwrap_or(&[])
                .to_vec();
            if let Some(kf8i) = parsed.kf8_header_index() {
                if offset == 0 {
                    // MOBI6 header in a joint file: adjust
                    // last_resource_record.
                    last_resource_record = kf8i.wrapping_sub(2);
                }
            }
            exth = Some(parsed);
        }

        Ok(MobiHeader {
            raw: raw.to_vec(),
            header_offset: offset,
            compression,
            unused,
            text_length,
            number_of_text_records,
            text_record_size,
            encryption_type,
            unknown,
            identifier,
            length,
            type_,
            encoding,
            uid,
            file_version,
            meta_orth_indx,
            meta_infl_indx: make_absolute(meta_infl_indx, offset),
            secondary_index_record,
            reserved,
            first_non_book_record,
            fullname_offset,
            fullname_length,
            locale_raw,
            language,
            sublanguage,
            input_language,
            output_language,
            min_version,
            first_image_index,
            huffman_record_offset,
            huffman_record_count,
            datp_record_offset,
            datp_record_count,
            exth_flags,
            has_exth,
            has_drm_data,
            unknown3,
            drm_offset,
            drm_count,
            drm_size,
            drm_flags,
            has_extra_data_flags,
            unknown4,
            first_text_record,
            last_text_record,
            fdst_idx,
            fdst_count,
            fcis_number,
            fcis_count,
            flis_number,
            flis_count,
            unknown6,
            srcs_record_index,
            num_srcs_records,
            unknown7,
            extra_data_flags,
            has_multibytes,
            has_indexing_bytes,
            has_uncrossable_breaks,
            primary_index_record,
            sect_idx,
            skel_idx,
            datp_idx,
            oth_idx,
            unknown9,
            first_resource_record,
            last_resource_record,
            exth_offset,
            exth,
            end_of_exth,
            bytes_after_exth,
        })
    }

    /// One "Absolute: X Relative: X - header_offset" or "NULL" line,
    /// matching `MOBIHeader.__str__`'s nested `r()`/`i()` helpers.
    fn render_relative(&self, label: &str, value: u32) -> String {
        if value == NULL_INDEX {
            format!("{label}: NULL")
        } else {
            format!(
                "{label}: Absolute: {value} Relative: {}",
                value as i64 - self.header_offset as i64
            )
        }
    }

    fn render_plain(label: &str, value: u32) -> String {
        if value == NULL_INDEX {
            format!("{label}: NULL")
        } else {
            format!("{label}: {value}")
        }
    }
}

impl fmt::Display for MobiHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} MOBI {} Header {}",
            "*".repeat(20),
            self.file_version,
            "*".repeat(20)
        )?;
        writeln!(f, "Compression: {}", self.compression)?;
        writeln!(f, "Unused: {:?}", self.unused)?;
        writeln!(f, "Text length: {}", self.text_length)?;
        writeln!(f, "Number of text records: {}", self.number_of_text_records)?;
        writeln!(f, "Text record size: {}", self.text_record_size)?;
        writeln!(f, "Encryption: {}", self.encryption_type)?;
        writeln!(f, "Unknown: {:?}", self.unknown)?;
        writeln!(f, "Identifier: {:?}", self.identifier)?;
        writeln!(f, "Header length: {}", self.length)?;
        writeln!(f, "Type: {}", self.type_)?;
        writeln!(f, "Encoding: {}", self.encoding)?;
        writeln!(f, "UID: {:?}", self.uid)?;
        writeln!(f, "File version: {}", self.file_version)?;
        writeln!(
            f,
            "{}",
            self.render_relative("Meta Orth Index", self.meta_orth_indx)
        )?;
        writeln!(
            f,
            "{}",
            self.render_relative("Meta Infl Index", self.meta_infl_indx)
        )?;
        writeln!(
            f,
            "{}",
            Self::render_plain("Secondary index record", self.secondary_index_record)
        )?;
        writeln!(f, "Reserved: {:?}", self.reserved)?;
        writeln!(
            f,
            "{}",
            self.render_relative("First non-book record", self.first_non_book_record)
        )?;
        writeln!(f, "Full name offset: {}", self.fullname_offset)?;
        writeln!(f, "Full name length: {} bytes", self.fullname_length)?;
        writeln!(f, "Langcode: {:?}", self.locale_raw)?;
        writeln!(f, "Language: {}", self.language)?;
        writeln!(f, "Sub language: {}", self.sublanguage)?;
        writeln!(f, "Input language: {:?}", self.input_language)?;
        writeln!(f, "Output language: {:?}", self.output_language)?;
        writeln!(f, "Min version: {}", self.min_version)?;
        writeln!(
            f,
            "{}",
            self.render_relative("First Image index", self.first_image_index)
        )?;
        writeln!(
            f,
            "{}",
            self.render_relative("Huffman record offset", self.huffman_record_offset)
        )?;
        writeln!(f, "Huffman record count: {}", self.huffman_record_count)?;
        writeln!(
            f,
            "{}",
            self.render_relative("Huffman table offset", self.datp_record_offset)
        )?;
        writeln!(f, "Huffman table length: {}", self.datp_record_count)?;
        writeln!(f, "EXTH flags: {:b} ({})", self.exth_flags, self.has_exth)?;
        if self.has_drm_data {
            writeln!(f, "Unknown3: {:?}", self.unknown3)?;
            writeln!(f, "{}", self.render_relative("DRM Offset", self.drm_offset))?;
            writeln!(f, "DRM Count: {}", self.drm_count)?;
            writeln!(f, "DRM Size: {}", self.drm_size)?;
            writeln!(f, "DRM Flags: {:?}", self.drm_flags)?;
        }
        if self.has_extra_data_flags {
            writeln!(f, "Unknown4: {:?}", self.unknown4)?;
            if let (Some(first), Some(last)) = (self.first_text_record, self.last_text_record) {
                writeln!(f, "First content record: {first}")?;
                writeln!(f, "Last content record: {last}")?;
            } else {
                writeln!(f, "{}", self.render_relative("FDST Index", self.fdst_idx))?;
            }
            writeln!(f, "FDST Count: {}", self.fdst_count)?;
            writeln!(
                f,
                "{}",
                self.render_relative("FCIS number", self.fcis_number)
            )?;
            writeln!(f, "FCIS count: {}", self.fcis_count)?;
            writeln!(
                f,
                "{}",
                self.render_relative("FLIS number", self.flis_number)
            )?;
            writeln!(f, "FLIS count: {}", self.flis_count)?;
            writeln!(f, "Unknown6: {:?}", self.unknown6)?;
            writeln!(
                f,
                "{}",
                Self::render_plain("SRCS record index", self.srcs_record_index)
            )?;
            writeln!(f, "Number of SRCS records?: {}", self.num_srcs_records)?;
            writeln!(f, "Unknown7: {:?}", self.unknown7)?;
            writeln!(
                f,
                "Extra data flags: {:b} (has multibyte: {}) (has indexing: {}) (has uncrossable breaks: {})",
                self.extra_data_flags, self.has_multibytes, self.has_indexing_bytes, self.has_uncrossable_breaks
            )?;
            writeln!(
                f,
                "{}",
                self.render_relative("NCX index", self.primary_index_record)
            )?;
        }
        if self.length >= 248 {
            writeln!(
                f,
                "{}",
                self.render_relative("Sections Index", self.sect_idx)
            )?;
            writeln!(f, "{}", self.render_relative("SKEL Index", self.skel_idx))?;
            writeln!(f, "{}", self.render_relative("DATP Index", self.datp_idx))?;
            writeln!(f, "{}", self.render_relative("Other Index", self.oth_idx))?;
            if !self.unknown9.is_empty() {
                writeln!(f, "Unknown9: {:?}", self.unknown9)?;
            }
        }

        if let Some(exth) = &self.exth {
            writeln!(f)?;
            writeln!(f, "{exth}")?;
            write!(
                f,
                "\nBytes after EXTH ({} bytes): {}",
                self.bytes_after_exth.len(),
                format_bytes(&self.bytes_after_exth)
            )?;
        }

        write!(
            f,
            "\nNumber of bytes after full name: {}\nRecord 0 length: {}",
            self.raw.len() as i64 - (self.fullname_offset as i64 + self.fullname_length as i64),
            self.raw.len()
        )
    }
}

/// A parsed, whole PalmDB+MOBI file, at the level `debug::headers`
/// needs. `MOBIFile` in the Python.
///
/// This is the shared base both `debug::mobi6::MOBIFile` and
/// `debug::mobi8::MOBIFile` wrap.
pub struct MobiFile {
    pub raw: Vec<u8>,
    pub palmdb: PalmDb,
    pub records: Vec<Record>,
    pub mobi_header: MobiHeader,
    pub mobi8_header: MobiHeader,
    pub huffman_record_nums: Vec<u32>,
    /// `None`, `"standalone"`, or `"joint"`. `kf8_type` in the Python.
    pub kf8_type: Option<&'static str>,
    /// A function decompressing one MOBI6 text record.
    decompress6: fn(&[u8]) -> Result<Vec<u8>>,
    /// A function decompressing one MOBI8 text record. Equal to
    /// `decompress6` except for a joint HUFF/CDIC file, where the two
    /// halves have independent huffman tables.
    decompress8: fn(&[u8]) -> Result<Vec<u8>>,
    huff6: Option<crate::mobi::huffcdic::HuffReader>,
    huff8: Option<crate::mobi::huffcdic::HuffReader>,
}

fn identity_decompress(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

impl MobiFile {
    /// `MOBIFile.__init__`.
    pub fn parse(raw: Vec<u8>) -> Result<Self> {
        let palmdb = PalmDb::parse(&raw[..78.min(raw.len())])?;

        let mut record_headers = Vec::with_capacity(palmdb.number_of_records as usize);
        for i in 0..palmdb.number_of_records as usize {
            let pos = 78 + i * 8;
            let offset = be_u32(&raw[pos..pos + 4])?;
            let a1 = raw[pos + 4];
            let a2 = raw[pos + 5];
            let a3 = raw[pos + 6];
            let a4 = raw[pos + 7];
            let val = (u32::from(a2) << 16) | (u32::from(a3) << 8) | u32::from(a4);
            record_headers.push((offset, a1, val));
        }

        let mut records = Vec::with_capacity(record_headers.len());
        for i in 0..record_headers.len() {
            let start = record_headers[i].0 as usize;
            let end = if i + 1 == record_headers.len() {
                raw.len()
            } else {
                record_headers[i + 1].0 as usize
            };
            // Python's `raw[off:end_off]` never raises even when
            // `end_off < off` or either is past the end of the
            // buffer — it just yields `b''`. A debug tool exists to
            // show whatever it can about a possibly-malformed file,
            // so this mirrors that leniency instead of panicking on
            // out-of-range slice bounds.
            let slice = if start <= end && end <= raw.len() {
                raw[start..end].to_vec()
            } else {
                Vec::new()
            };
            records.push(Record::new(slice, record_headers[i]));
        }

        let mobi_header = MobiHeader::parse(&records[0].raw, 0)?;

        let mut kf8_type = None;
        let mut mobi8_header_idx: Option<usize> = None;
        if mobi_header.file_version >= 8 {
            kf8_type = Some("standalone");
        } else if mobi_header.has_exth {
            if let Some(kf8i) = mobi_header.exth.as_ref().and_then(|e| e.kf8_header_index()) {
                if let Some(rec) = records.get(kf8i as usize - 1) {
                    if rec.raw == b"BOUNDARY" {
                        kf8_type = Some("joint");
                        mobi8_header_idx = Some(kf8i as usize);
                    }
                }
            }
        }
        let mobi8_header = match mobi8_header_idx {
            Some(idx) => MobiHeader::parse(&records[idx].raw, idx as u32)?,
            None => MobiHeader::parse(&records[0].raw, 0)?,
        };

        let mut huffman_record_nums = Vec::new();
        let mut huff6 = None;
        let mut huff8 = None;
        let (decompress6, decompress8): (
            fn(&[u8]) -> Result<Vec<u8>>,
            fn(&[u8]) -> Result<Vec<u8>>,
        );
        if mobi_header.compression.to_lowercase().contains("huff") {
            let huffit =
                |off: u32, cnt: u32| -> Result<(Vec<u32>, crate::mobi::huffcdic::HuffReader)> {
                    let nums: Vec<u32> = (off..off + cnt).collect();
                    let huffrecs: Vec<Vec<u8>> = nums
                        .iter()
                        .map(|&r| records[r as usize].raw.clone())
                        .collect();
                    let huffs = crate::mobi::huffcdic::HuffReader::new(&huffrecs)?;
                    Ok((nums, huffs))
                };
            if kf8_type == Some("joint") {
                let (nums6, h6) = huffit(
                    mobi_header.huffman_record_offset,
                    mobi_header.huffman_record_count,
                )?;
                let (nums8, h8) = huffit(
                    mobi8_header.huffman_record_offset,
                    mobi8_header.huffman_record_count,
                )?;
                huffman_record_nums = nums6.into_iter().chain(nums8).collect();
                huff6 = Some(h6);
                huff8 = Some(h8);
            } else {
                let (nums6, h6) = huffit(
                    mobi_header.huffman_record_offset,
                    mobi_header.huffman_record_count,
                )?;
                huffman_record_nums = nums6;
                huff6 = Some(h6);
            }
            decompress6 = identity_decompress; // overridden per-call via huff6, see decompress_text6/8
            decompress8 = identity_decompress;
        } else if mobi_header.compression.to_lowercase().contains("palmdoc") {
            decompress6 = crate::compression::palmdoc::decompress;
            decompress8 = crate::compression::palmdoc::decompress;
        } else {
            decompress6 = identity_decompress;
            decompress8 = identity_decompress;
        }

        Ok(MobiFile {
            raw,
            palmdb,
            records,
            mobi_header,
            mobi8_header,
            huffman_record_nums,
            kf8_type,
            decompress6,
            decompress8,
            huff6,
            huff8,
        })
    }

    /// Decompress one MOBI6 text record. Threads through the
    /// stateful `HuffReader` when the file is huffman-compressed,
    /// since `fn` pointers can't close over it.
    pub fn decompress_text6(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if let Some(h) = self.huff6.as_mut() {
            h.unpack(data)
        } else {
            (self.decompress6)(data)
        }
    }

    /// As [`MobiFile::decompress_text6`], for the MOBI8 half of the
    /// file.
    pub fn decompress_text8(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if let Some(h) = self.huff8.as_mut() {
            h.unpack(data)
        } else if self.huff6.is_some() {
            // Non-joint huffman file: both halves share one table.
            self.huff6.as_mut().unwrap().unpack(data)
        } else {
            (self.decompress8)(data)
        }
    }
}

/// One decompressed text record, with its trailing per-record data
/// (multibyte-overlap, indexing, uncrossable-break bytes) split out.
/// `TextRecord` in the Python.
pub struct TextRecord {
    pub idx: u32,
    pub raw: Vec<u8>,
    /// Keyed by the Python's post-`pop` names where recognized
    /// (`"multibyte_overlap"`, `"indexing"`, `"uncrossable_breaks"`),
    /// else the raw numeric trailing-data type as a string.
    pub trailing_data: BTreeMap<String, Vec<u8>>,
    /// Bytes belonging to the record but outside its decompressed
    /// text and any known trailing-data section — `raw_bytes` in the
    /// Python's `trailing_data` dict.
    pub raw_trailing_bytes: Vec<u8>,
}

impl TextRecord {
    /// `TextRecord.__init__`. `decompress` is `MobiFile::decompress_text6`
    /// or `::decompress_text8`, applied by the caller since Rust can't
    /// pass a `&mut self` method as a plain closure argument here.
    pub fn new(
        idx: u32,
        record_raw: &[u8],
        extra_data_flags: u32,
        decompressed: Vec<u8>,
    ) -> Result<Self> {
        let (numeric_trailing, remainder) = get_trailing_data(record_raw, extra_data_flags)?;
        let raw_trailing_bytes = record_raw[remainder.len()..].to_vec();

        let mut trailing_data = BTreeMap::new();
        for (k, v) in numeric_trailing {
            let name = match k {
                0 => "multibyte_overlap".to_string(),
                1 => "indexing".to_string(),
                2 => "uncrossable_breaks".to_string(),
                other => other.to_string(),
            };
            trailing_data.insert(name, v);
        }

        Ok(TextRecord {
            idx,
            raw: decompressed,
            trailing_data,
            raw_trailing_bytes,
        })
    }

    /// `TextRecord.dump`.
    pub fn dump(&self, folder: &std::path::Path) -> Result<()> {
        let name = format!("{:06}", self.idx);
        std::fs::write(folder.join(format!("{name}.txt")), &self.raw)?;
        let mut trailing = String::new();
        for (k, v) in &self.trailing_data {
            trailing.push_str(&format!("{k} : {:?}\n\n", String::from_utf8_lossy(v)));
        }
        trailing.push_str(&format!(
            "raw_bytes : {:?}\n\n",
            String::from_utf8_lossy(&self.raw_trailing_bytes)
        ));
        std::fs::write(folder.join(format!("{name}.trailing_data")), trailing)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_palmdb_bytes() -> Vec<u8> {
        let mut raw = vec![0u8; 78];
        raw[..8].copy_from_slice(b"MyBook\0\0");
        raw[60..64].copy_from_slice(b"BOOK");
        raw[64..68].copy_from_slice(b"MOBI");
        raw[76..78].copy_from_slice(&1u16.to_be_bytes());
        raw
    }

    #[test]
    fn palmdb_parses_the_ident_and_name() {
        let raw = sample_palmdb_bytes();
        let db = PalmDb::parse(&raw).expect("parses");
        assert_eq!(db.name, "MyBook");
        assert_eq!(db.ident, b"BOOKMOBI");
        assert_eq!(db.number_of_records, 1);
    }

    #[test]
    fn palmdb_rejects_an_unknown_ident() {
        let mut raw = sample_palmdb_bytes();
        raw[60..68].copy_from_slice(b"XXXXXXXX");
        assert!(PalmDb::parse(&raw).is_err());
    }

    #[test]
    fn palmdb_rejects_a_topaz_file() {
        let mut raw = sample_palmdb_bytes();
        raw[..3].copy_from_slice(b"TPZ");
        assert!(PalmDb::parse(&raw).is_err());
    }

    #[test]
    fn palm_epoch_matches_a_known_date() {
        // 1 day after the Palm epoch (1904-01-01) is 1904-01-02.
        let d = palm_epoch_plus(86400);
        assert_eq!(d.to_rfc3339(), "1904-01-02T00:00:00+00:00");
    }

    fn sample_mobi_header_bytes(text_length: u32, exth: Option<&[u8]>) -> Vec<u8> {
        let mut raw = vec![0u8; 232 + 16];
        raw[0..2].copy_from_slice(&1u16.to_be_bytes()); // no compression
        raw[4..8].copy_from_slice(&text_length.to_be_bytes());
        raw[8..10].copy_from_slice(&1u16.to_be_bytes()); // 1 text record
        raw[16..20].copy_from_slice(b"MOBI");
        raw[20..24].copy_from_slice(&232u32.to_be_bytes()); // header length
        raw[24..28].copy_from_slice(&2u32.to_be_bytes()); // Mobipocket book
        raw[28..32].copy_from_slice(&65001u32.to_be_bytes()); // utf-8
        raw[36..40].copy_from_slice(&6u32.to_be_bytes()); // file_version
        raw[80..84].copy_from_slice(&NULL_INDEX.to_be_bytes());
        let fullname = b"Test Book";
        let fullname_offset = raw.len() as u32;
        raw[84..88].copy_from_slice(&fullname_offset.to_be_bytes());
        raw[88..92].copy_from_slice(&(fullname.len() as u32).to_be_bytes());
        if let Some(exth) = exth {
            raw[128..132].copy_from_slice(&0x40u32.to_be_bytes()); // has_exth
            raw.extend_from_slice(exth);
        }
        raw.extend_from_slice(fullname);
        raw
    }

    #[test]
    fn mobi_header_parses_basic_fields() {
        let raw = sample_mobi_header_bytes(1000, None);
        let h = MobiHeader::parse(&raw, 0).expect("parses");
        assert_eq!(h.compression, "No compression");
        assert_eq!(h.text_length, 1000);
        assert_eq!(h.type_, "Mobipocket book");
        assert_eq!(h.encoding, "utf-8");
        assert!(!h.has_exth);
    }

    #[test]
    fn mobi_header_rejects_a_bad_identifier() {
        let mut raw = sample_mobi_header_bytes(10, None);
        raw[16..20].copy_from_slice(b"NOPE");
        assert!(MobiHeader::parse(&raw, 0).is_err());
    }

    fn build_exth(records: &[(u32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, data) in records {
            body.extend_from_slice(&t.to_be_bytes());
            body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"EXTH");
        out.extend_from_slice(&((body.len() + 12) as u32).to_be_bytes());
        out.extend_from_slice(&(records.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn mobi_header_parses_an_exth_block() {
        let exth = build_exth(&[(101, b"A Publisher")]);
        let raw = sample_mobi_header_bytes(10, Some(&exth));
        let h = MobiHeader::parse(&raw, 0).expect("parses");
        assert!(h.has_exth);
        let publisher = h.exth.as_ref().unwrap().get(101).unwrap();
        assert!(matches!(&publisher.data, ExthValue::Bytes(b) if b == b"A Publisher"));
        assert_eq!(publisher.name, "Publisher");
    }

    #[test]
    fn exth_numeric_fields_decode_as_integers() {
        let exth = build_exth(&[(115, &[0u8, 0, 0, 42])]); // sample, 4-byte -> u32
        let h = ExthHeader::parse(&exth).expect("parses");
        let r = h.get(115).unwrap();
        assert!(matches!(r.data, ExthValue::Number(42)));
    }

    #[test]
    fn exth_hex_fields_are_hex_encoded() {
        let exth = build_exth(&[(300, &[0xDE, 0xAD, 0xBE, 0xEF])]);
        let h = ExthHeader::parse(&exth).expect("parses");
        let r = h.get(300).unwrap();
        assert!(matches!(&r.data, ExthValue::Hex(s) if s == "deadbeef"));
    }

    #[test]
    fn mobi_header_display_includes_key_sections() {
        let exth = build_exth(&[(100, b"An Author")]);
        let raw = sample_mobi_header_bytes(500, Some(&exth));
        let h = MobiHeader::parse(&raw, 0).expect("parses");
        let s = h.to_string();
        assert!(s.contains("MOBI 6 Header"));
        assert!(s.contains("Text length: 500"));
        assert!(s.contains("EXTH Header"));
        assert!(s.contains("Creator (100)"));
    }
}
