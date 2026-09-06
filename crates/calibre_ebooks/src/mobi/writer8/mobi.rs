//! `KF8Book`/`MOBIHeader`: assembles the KF8 `record0` (fixed MOBI
//! header + `EXTH` + title) and the full record list (text, `SKEL`/
//! `CHUNK`/`GUIDE`/`NCX` index, image/font resources, `FDST`, `FLIS`/
//! `FCIS`, EOF), then serializes a standalone `.azw3`-shaped PDB file.
//!
//! Port of `calibre.ebooks.mobi.writer8.mobi`.
//!
//! # `for_joint`
//!
//! Python's `KF8Book.build_records(writer, for_joint)` skips serializing
//! image/font resources when `for_joint` is set (a joint MOBI6+KF8 file
//! shares one resource block between both `record0`s, owned by the
//! MOBI6 sibling writer). [`KF8Book::new`] takes the same flag and the
//! same skip, even though wiring a real joint-output caller is a
//! separate, larger piece of work (see `writer2::main`'s module doc) --
//! the flag's *effect on this module* is fully real either way.

use std::collections::HashSet;

use anyhow::{Context, Result};

use crate::mobi::langcodes::iana2mobi;
use crate::mobi::utils::utf8_text;
use crate::mobi::writer2::resources::Resources;
use crate::mobi::writer8::exth::{build_exth, ExthParams};
use crate::mobi::writer8::header::{nulls, zeroes, FieldDef, FieldValue, Header, NULL};
use crate::oeb::metadata::Metadata;

/// `NULL_INDEX` in `mobi.py`.
pub const NULL_INDEX: u32 = 0xffff_ffff;

/// `FLIS` in `mobi.py` (byte-identical to `writer2::main`'s own `FLIS`
/// constant -- Python defines the same bytes twice too, once per writer
/// module, rather than sharing them).
const FLIS: &[u8] = b"FLIS\0\0\0\x08\0\x41\0\0\0\0\0\0\xff\xff\xff\xff\0\x01\0\x03\0\0\0\x03\0\0\0\x01\xff\xff\xff\xff";

/// Port of `fcis()` in `mobi.py`. Not the same bytes as
/// `writer2::main`'s `fcis()` (a different constant at byte offset 12:
/// `\x02` here vs `\x01` there) -- ported separately to match.
fn fcis(text_length: u32) -> Vec<u8> {
    let mut v = b"FCIS\x00\x00\x00\x14\x00\x00\x00\x10\x00\x00\x00\x02\x00\x00\x00\x00".to_vec();
    v.extend_from_slice(&text_length.to_be_bytes());
    v.extend_from_slice(b"\x00\x00\x00\x00\x00\x00\x00\x28\x00\x00\x00\x00\x00\x00\x00");
    v.extend_from_slice(b"\x28\x00\x00\x00\x08\x00\x01\x00\x01\x00\x00\x00\x00");
    v
}

/// `RECORD_SIZE` used as the fixed `record_size` header field value.
const RECORD_SIZE: u64 = crate::mobi::utils::RECORD_SIZE as u64;

/// Port of `MOBIHeader.__init__(file_version=...)`: Python re-templates
/// its `DEFINITION` string per instance with `file_version` substituted
/// into both the `file_version` and `min_version` fields (`header_length`
/// stays the fixed 264-byte KF8-style layout regardless -- real joint
/// output's MOBI6-view header uses this same shape stamped `file_version
/// = 6`, not the smaller 232-byte legacy header `writer2::main`'s own
/// standalone-MOBI6 path builds by hand).
fn mobi_header_fields(file_version: u64) -> Vec<FieldDef> {
    vec![
        FieldDef::short("compression", FieldValue::Dyn),
        FieldDef::new("unused1", zeroes(2)),
        FieldDef::new("text_length", FieldValue::Dyn),
        FieldDef::short("last_text_record", FieldValue::Dyn),
        FieldDef::short("record_size", FieldValue::Int(RECORD_SIZE)),
        FieldDef::short("encryption_type", FieldValue::Int(0)),
        FieldDef::short("unused2", FieldValue::Int(0)),
        FieldDef::new("ident", FieldValue::Bytes(b"MOBI".to_vec())),
        FieldDef::new("header_length", FieldValue::Int(264)),
        FieldDef::new("book_type", FieldValue::Dyn),
        FieldDef::new("encoding", FieldValue::Int(65001)),
        FieldDef::new("uid", FieldValue::Dyn),
        FieldDef::new("file_version", FieldValue::Int(file_version)),
        FieldDef::new("meta_orth_record", FieldValue::Int(NULL)),
        FieldDef::new("meta_infl_index", FieldValue::Int(NULL)),
        FieldDef::new("extra_index0", FieldValue::Int(NULL)),
        FieldDef::new("extra_index1", FieldValue::Int(NULL)),
        FieldDef::new("extra_index2", FieldValue::Int(NULL)),
        FieldDef::new("extra_index3", FieldValue::Int(NULL)),
        FieldDef::new("extra_index4", FieldValue::Int(NULL)),
        FieldDef::new("extra_index5", FieldValue::Int(NULL)),
        FieldDef::new("extra_index6", FieldValue::Int(NULL)),
        FieldDef::new("extra_index7", FieldValue::Int(NULL)),
        FieldDef::new("first_non_text_record", FieldValue::Dyn),
        FieldDef::new("title_offset", FieldValue::Int(0)),
        FieldDef::new("title_length", FieldValue::Dyn),
        FieldDef::new("language_code", FieldValue::Dyn),
        FieldDef::new("in_lang", FieldValue::Int(0)),
        FieldDef::new("out_lang", FieldValue::Int(0)),
        FieldDef::new("min_version", FieldValue::Int(file_version)),
        FieldDef::new("first_resource_record", FieldValue::Dyn),
        FieldDef::new("huff_first_record", FieldValue::Int(0)),
        FieldDef::new("huff_count", FieldValue::Int(0)),
        FieldDef::new("huff_table_offset", zeroes(4)),
        FieldDef::new("huff_table_length", zeroes(4)),
        FieldDef::new("exth_flags", FieldValue::Dyn),
        FieldDef::new("unknown", zeroes(32)),
        FieldDef::new("unknown_index", FieldValue::Int(NULL)),
        FieldDef::new("drm_offset", FieldValue::Int(NULL)),
        FieldDef::new("drm_count", FieldValue::Int(0)),
        FieldDef::new("drm_size", FieldValue::Int(0)),
        FieldDef::new("drm_flags", FieldValue::Int(0)),
        FieldDef::new("unknown2", zeroes(8)),
        FieldDef::new("fdst_record", FieldValue::Dyn),
        FieldDef::new("fdst_count", FieldValue::Dyn),
        FieldDef::new("fcis_record", FieldValue::Dyn),
        FieldDef::new("fcis_count", FieldValue::Int(1)),
        FieldDef::new("flis_record", FieldValue::Dyn),
        FieldDef::new("flis_count", FieldValue::Int(1)),
        FieldDef::new("unknown3", zeroes(8)),
        FieldDef::new("srcs_record", FieldValue::Int(NULL)),
        FieldDef::new("srcs_count", FieldValue::Int(0)),
        FieldDef::new("unknown4", nulls(8)),
        FieldDef::new("extra_data_flags", FieldValue::Dyn),
        FieldDef::new("ncx_index", FieldValue::Dyn),
        FieldDef::new("chunk_index", FieldValue::Dyn),
        FieldDef::new("skel_index", FieldValue::Dyn),
        FieldDef::new("datp_index", FieldValue::Int(NULL)),
        FieldDef::new("guide_index", FieldValue::Dyn),
        FieldDef::new("unknown5", nulls(4)),
        FieldDef::new("unknown6", zeroes(4)),
        FieldDef::new("unknown7", nulls(4)),
        FieldDef::new("unknown8", zeroes(4)),
        FieldDef::new("exth", FieldValue::Dyn),
        FieldDef::new("full_title", FieldValue::Dyn),
        FieldDef::new("padding", zeroes(8192)),
    ]
}

/// Port of `MOBIHeader`.
///
/// Python sets a class attribute `ALIGN = True` on this class, but
/// `Header.__call__` actually consults `self.ALIGN_BLOCK` (the base
/// class default, `False`) -- `MOBIHeader` never overrides
/// `ALIGN_BLOCK`, only the unrelated, unused `ALIGN` name. So the real
/// Python runtime behavior is "never 4-byte-align `record0`'s tail",
/// despite the apparent intent; this port reproduces the *actual*
/// behavior (`align = false` below), not the apparently-intended one,
/// since matching what kindlegen/the Kindle firmware actually receives
/// is what matters for compatibility.
fn mobi_header(file_version: u64) -> Header {
    Header::new(
        b"",
        false,
        &mobi_header_fields(file_version),
        &[("title_offset", "full_title")],
    )
}

/// The fields [`KF8Book::record0`] needs beyond what
/// [`KF8Book::build_records`] itself computes -- `writer.opts`'s
/// `EXTH`-relevant flags, threaded through explicitly rather than via a
/// generic options object (mirroring how narrowly `writer2::main`
/// already trims Python's `opts` down to the fields it actually reads).
#[derive(Debug, Clone, Default)]
pub struct Kf8Opts {
    pub prefer_author_sort: bool,
    pub share_not_sync: bool,
    pub mobi_periodical: bool,
}

/// Everything [`KF8Book::new`] needs from a finished `KF8Writer` pass.
/// Port of the attributes `KF8Book.build_records` reads off `writer`.
pub struct KF8BuildInputs<'a> {
    pub last_text_record_idx: usize,
    pub first_non_text_record_idx: usize,
    /// `writer.records`: index 0 is a placeholder, `1..=last_text_record_idx`
    /// are the (already-compressed, TBS-trailered) text records. Consumed
    /// (appended to) as everything else gets bolted on.
    pub records: Vec<Vec<u8>>,
    pub text_length: usize,
    pub chunk_records: Vec<Vec<u8>>,
    pub skel_records: Vec<Vec<u8>>,
    pub guide_records: Vec<Vec<u8>>,
    pub ncx_records: Vec<Vec<u8>>,
    pub resources: &'a mut Resources,
    pub used_images: HashSet<String>,
    pub fdst_count: usize,
    pub fdst_records: Vec<Vec<u8>>,
    pub compress: bool,
    pub has_tbs: bool,
    pub start_offset: Option<u32>,
    pub metadata: &'a Metadata,
    pub opts: Kf8Opts,
    pub page_progression_direction: Option<String>,
    pub primary_writing_mode: Option<String>,
}

/// Port of `KF8Book`.
pub struct KF8Book {
    records: Vec<Vec<u8>>,
    /// `KF8Book.used_images` (`writer.used_images`, kept verbatim from
    /// the underlying `KF8Writer` pass) -- a real joint-output caller
    /// unions this with its own MOBI6 `Serializer::used_images` before
    /// serializing the shared resource block once.
    pub used_images: HashSet<String>,
    chunk_index: u32,
    skel_index: u32,
    guide_index: u32,
    ncx_index: u32,
    first_resource_record: u32,
    num_of_resources: u32,
    fdst_count: u32,
    fdst_record: u32,
    flis_record: u32,
    fcis_record: u32,
    compression: u16,
    book_type: u32,
    full_title: Vec<u8>,
    extra_data_flags: u32,
    uid: u32,
    language_code: u32,
    exth_flags: u32,
    text_length: u32,
    last_text_record: u32,
    first_non_text_record: u32,
    cover_offset: Option<u32>,
    thumbnail_offset: Option<u32>,
    kf8_unknown_count: Option<u32>,
    start_offset: Option<u32>,
    metadata_title: String,
    opts: Kf8Opts,
    page_progression_direction: Option<String>,
    primary_writing_mode: Option<String>,
}

impl KF8Book {
    /// Port of `KF8Book.__init__` + `KF8Book.build_records`.
    pub fn new(mut inputs: KF8BuildInputs<'_>, for_joint: bool) -> Result<Self> {
        let mut records = std::mem::take(&mut inputs.records);

        let chunk_index = records.len() as u32;
        records.extend(inputs.chunk_records);
        let skel_index = records.len() as u32;
        records.extend(inputs.skel_records);

        let guide_index = if inputs.guide_records.is_empty() {
            NULL_INDEX
        } else {
            let idx = records.len() as u32;
            records.extend(inputs.guide_records);
            idx
        };
        let ncx_index = if inputs.ncx_records.is_empty() {
            NULL_INDEX
        } else {
            let idx = records.len() as u32;
            records.extend(inputs.ncx_records);
            idx
        };

        let cover_offset = inputs.resources.cover_offset.map(|v| v as u32);
        let thumbnail_offset = inputs.resources.thumbnail_offset.map(|v| v as u32);

        let before = records.len();
        let first_resource_record = if !inputs.resources.records.is_empty() {
            let idx = records.len() as u32;
            if !for_joint {
                inputs
                    .resources
                    .serialize(&mut records, &inputs.used_images);
            }
            idx
        } else {
            NULL_INDEX
        };
        let num_of_resources = (records.len() - before) as u32;

        let fdst_count = inputs.fdst_count as u32;
        let fdst_record = records.len() as u32;
        records.extend(inputs.fdst_records);

        let flis_record = records.len() as u32;
        records.push(FLIS.to_vec());
        let fcis_record = records.len() as u32;
        records.push(fcis(inputs.text_length as u32));

        records.push(vec![0xe9, 0x8e, 0x0d, 0x0a]); // EOF record

        let compression: u16 = if inputs.compress {
            crate::mobi::writer2::PALMDOC
        } else {
            crate::mobi::writer2::UNCOMPRESSED
        };
        let book_type: u32 = if inputs.opts.mobi_periodical {
            0x101
        } else {
            2
        };
        let title = inputs
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let full_title = utf8_text(&title);
        let mut extra_data_flags: u32 = 0b1;
        if inputs.has_tbs {
            extra_data_flags |= 0b10;
        }
        let uid: u32 = rand::random();
        let language = inputs
            .metadata
            .get("language")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let language_code =
            u32::from_be_bytes(iana2mobi(&language).try_into().unwrap_or([0, 0, 0, 0]));
        let mut exth_flags: u32 = 0b1010000;
        if inputs.opts.mobi_periodical {
            exth_flags |= 0b1000;
        }
        if inputs.resources.has_fonts {
            exth_flags |= 0b1000000000000;
        }

        let kf8_unknown_count = if !inputs.resources.records.is_empty() {
            Some(0)
        } else {
            None
        };

        // Without this the Kindle renderer does not respect
        // page_progression_direction.
        let mut primary_writing_mode = inputs.primary_writing_mode.clone();
        if inputs.page_progression_direction.as_deref() == Some("rtl")
            && primary_writing_mode.is_none()
        {
            primary_writing_mode = Some("horizontal-rl".to_string());
        }

        Ok(KF8Book {
            records,
            used_images: inputs.used_images,
            chunk_index,
            skel_index,
            guide_index,
            ncx_index,
            first_resource_record,
            num_of_resources,
            fdst_count,
            fdst_record,
            flis_record,
            fcis_record,
            compression,
            book_type,
            full_title,
            extra_data_flags,
            uid,
            language_code,
            exth_flags,
            text_length: inputs.text_length as u32,
            last_text_record: inputs.last_text_record_idx as u32,
            first_non_text_record: inputs.first_non_text_record_idx as u32,
            cover_offset,
            thumbnail_offset,
            kf8_unknown_count,
            start_offset: inputs.start_offset,
            metadata_title: title,
            opts: inputs.opts,
            page_progression_direction: inputs.page_progression_direction,
            primary_writing_mode,
        })
    }

    /// Port of the `KF8Book.record0` property: builds the `EXTH` block
    /// and the fixed MOBI header, freshly, each call (so callers can
    /// tweak fields -- e.g. a joint writer overwriting `first_resource_record`
    /// -- between `new()` and serialization, matching Python's intent).
    pub fn record0(&self, metadata: &Metadata) -> Result<Vec<u8>> {
        self.record0_with_start_offset(metadata, self.start_offset, None)
    }

    /// Port of `KF8Book.record0` as accessed by `generate_joint_record0`,
    /// which first does `self.kf8.start_offset = (mobi6_start, kf8's own
    /// prior start_offset)` -- `build_exth` then writes one `startreading`
    /// EXTH record per non-`None` element of that pair. `mobi6_start` is
    /// the MOBI6 sibling writer's own `Serializer::start_offset`.
    pub fn record0_for_joint(&self, metadata: &Metadata, mobi6_start_offset: Option<u32>) -> Result<Vec<u8>> {
        self.record0_with_start_offset(metadata, mobi6_start_offset, self.start_offset)
    }

    fn record0_with_start_offset(
        &self,
        metadata: &Metadata,
        start_offset: Option<u32>,
        start_offset_secondary: Option<u32>,
    ) -> Result<Vec<u8>> {
        let exth_params = ExthParams {
            prefer_author_sort: self.opts.prefer_author_sort,
            is_periodical: self.opts.mobi_periodical,
            share_not_sync: self.opts.share_not_sync,
            cover_offset: self.cover_offset,
            thumbnail_offset: self.thumbnail_offset,
            num_of_resources: Some(self.num_of_resources),
            kf8_unknown_count: self.kf8_unknown_count,
            kf8_header_index: None,
            be_kindlegen2: true,
            start_offset,
            start_offset_secondary,
            mobi_doctype: self.book_type,
            page_progression_direction: self.page_progression_direction.clone(),
            primary_writing_mode: self.primary_writing_mode.clone(),
        };
        let exth = build_exth(metadata, &exth_params).context("building KF8 EXTH header")?;

        let mut header = mobi_header(8);
        header.set("compression", FieldValue::Int(self.compression as u64))?;
        header.set("text_length", FieldValue::Int(self.text_length as u64))?;
        header.set(
            "last_text_record",
            FieldValue::Int(self.last_text_record as u64),
        )?;
        header.set("book_type", FieldValue::Int(self.book_type as u64))?;
        header.set("uid", FieldValue::Int(self.uid as u64))?;
        header.set(
            "first_non_text_record",
            FieldValue::Int(self.first_non_text_record as u64),
        )?;
        header.set(
            "title_length",
            FieldValue::Int(self.full_title.len() as u64),
        )?;
        header.set("language_code", FieldValue::Int(self.language_code as u64))?;
        header.set(
            "first_resource_record",
            FieldValue::Int(self.first_resource_record as u64),
        )?;
        header.set("exth_flags", FieldValue::Int(self.exth_flags as u64))?;
        header.set("fdst_record", FieldValue::Int(self.fdst_record as u64))?;
        header.set("fdst_count", FieldValue::Int(self.fdst_count as u64))?;
        header.set("fcis_record", FieldValue::Int(self.fcis_record as u64))?;
        header.set("flis_record", FieldValue::Int(self.flis_record as u64))?;
        header.set(
            "extra_data_flags",
            FieldValue::Int(self.extra_data_flags as u64),
        )?;
        header.set("ncx_index", FieldValue::Int(self.ncx_index as u64))?;
        header.set("chunk_index", FieldValue::Int(self.chunk_index as u64))?;
        header.set("skel_index", FieldValue::Int(self.skel_index as u64))?;
        header.set("guide_index", FieldValue::Int(self.guide_index as u64))?;
        header.set("exth", FieldValue::Bytes(exth))?;
        header.set("full_title", FieldValue::Bytes(self.full_title.clone()))?;
        header.build()
    }

    /// This `KF8Book`'s own record list, minus the `record0` placeholder
    /// at index 0 -- what a joint writer appends after its own embedded
    /// `record0_for_joint()` (`self.kf8.records[1:]` in
    /// `generate_joint_record0`).
    pub fn tail_records(&self) -> &[Vec<u8>] {
        &self.records[1..]
    }

    /// `KF8Book.compression`, reused as-is for a joint file's MOBI6-view
    /// header (`generate_joint_record0` never overwrites `compression`).
    pub fn compression(&self) -> u16 {
        self.compression
    }

    /// `KF8Book.book_type`, reused as-is for a joint file's MOBI6-view
    /// header.
    pub fn book_type(&self) -> u32 {
        self.book_type
    }

    /// `KF8Book.full_title`, reused as-is (bytes and length both) for a
    /// joint file's MOBI6-view header.
    pub fn full_title(&self) -> &[u8] {
        &self.full_title
    }

    /// `KF8Book.language_code`, reused as-is for a joint file's MOBI6-view
    /// header.
    pub fn language_code(&self) -> u32 {
        self.language_code
    }

    /// `KF8Book.uid`, reused as-is (both headers of a joint file share one
    /// UID).
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// Port of `KF8Book.write`: assemble the PDB header + all records
    /// into a complete `.azw3`-shaped byte stream.
    pub fn to_bytes(&self, metadata: &Metadata) -> Result<Vec<u8>> {
        let record0 = self.record0(metadata)?;
        let mut records = Vec::with_capacity(self.records.len());
        records.push(record0);
        records.extend(self.records[1..].iter().cloned());

        let mut out = Vec::new();
        let mut name = calibre_utils::filenames::ascii_filename(&self.metadata_title)
            .replace(' ', "_")
            .into_bytes();
        name.truncate(31);
        name.resize(32, 0);
        out.extend_from_slice(&name);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&now.to_be_bytes());
        out.extend_from_slice(&now.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"BOOKMOBI");
        let nrecords = records.len();
        out.extend_from_slice(&((2 * nrecords as u32).wrapping_sub(1)).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(nrecords as u16).to_be_bytes());

        let mut offset = out.len() + (8 * nrecords) + 2;
        for (i, record) in records.iter().enumerate() {
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.push(0);
            let uid = (2 * i) as u32;
            out.extend_from_slice(&uid.to_be_bytes()[1..4]);
            offset += record.len();
        }
        out.extend_from_slice(&[0, 0]);

        for rec in &records {
            out.extend_from_slice(rec);
        }
        Ok(out)
    }
}

/// The 20 `HEADER_FIELDS` values `generate_joint_record0` computes for a
/// joint file's MOBI6-view `record0` -- some copied verbatim from the
/// `KF8Book` that was built alongside (`compression`, `book_type`,
/// `full_title`, `language_code`, `uid`), the rest computed fresh by the
/// MOBI6 sibling writer once the shared resource block, `BOUNDARY`
/// marker, and embedded KF8 record set have all been appended to one
/// combined record list. `chunk_index`/`skel_index`/`guide_index` are
/// always [`NULL_INDEX`] here -- a MOBI6 reader has no use for KF8's own
/// structural indices, and Python's `generate_joint_record0` sets them
/// to `NULL_INDEX` unconditionally rather than remapping them into the
/// combined list.
pub struct JointMobi6Fields {
    pub compression: u16,
    pub text_length: u32,
    pub last_text_record: u32,
    pub book_type: u32,
    pub first_non_text_record: u32,
    pub language_code: u32,
    pub first_resource_record: u32,
    pub exth_flags: u32,
    /// MOBI6 has no real FDST record -- this field instead packs the
    /// first/last content record numbers as two big-endian `u16`s
    /// (`pack('>HH', 1, last_content_record)` in Python), matching the
    /// `MOBIHeader.DEFINITION` comment: "In MOBI 6 the fdst record is
    /// instead two two byte fields storing the index of the first and
    /// last content records."
    pub fdst_record_packed: u32,
    pub ncx_index: u32,
    pub extra_data_flags: u32,
    pub flis_record: u32,
    pub fcis_record: u32,
    pub uid: u32,
    pub full_title: Vec<u8>,
    pub exth: Vec<u8>,
}

/// Port of `MOBIHeader(file_version=6)(**header_fields)`, the last line
/// of `generate_joint_record0` -- builds the MOBI6-view `record0` of a
/// joint MOBI6+KF8 file using the same 264-byte KF8-style header layout
/// as [`KF8Book::record0`], just stamped `file_version = 6` and filled
/// from [`JointMobi6Fields`] instead of `KF8Book`'s own state.
pub fn build_joint_mobi6_record0(fields: JointMobi6Fields) -> Result<Vec<u8>> {
    let mut header = mobi_header(6);
    header.set("compression", FieldValue::Int(fields.compression as u64))?;
    header.set("text_length", FieldValue::Int(fields.text_length as u64))?;
    header.set(
        "last_text_record",
        FieldValue::Int(fields.last_text_record as u64),
    )?;
    header.set("book_type", FieldValue::Int(fields.book_type as u64))?;
    header.set("uid", FieldValue::Int(fields.uid as u64))?;
    header.set(
        "first_non_text_record",
        FieldValue::Int(fields.first_non_text_record as u64),
    )?;
    header.set(
        "title_length",
        FieldValue::Int(fields.full_title.len() as u64),
    )?;
    header.set("language_code", FieldValue::Int(fields.language_code as u64))?;
    header.set(
        "first_resource_record",
        FieldValue::Int(fields.first_resource_record as u64),
    )?;
    header.set("exth_flags", FieldValue::Int(fields.exth_flags as u64))?;
    header.set(
        "fdst_record",
        FieldValue::Int(fields.fdst_record_packed as u64),
    )?;
    header.set("fdst_count", FieldValue::Int(1))?;
    header.set("fcis_record", FieldValue::Int(fields.fcis_record as u64))?;
    header.set("flis_record", FieldValue::Int(fields.flis_record as u64))?;
    header.set(
        "extra_data_flags",
        FieldValue::Int(fields.extra_data_flags as u64),
    )?;
    header.set("ncx_index", FieldValue::Int(fields.ncx_index as u64))?;
    header.set("chunk_index", FieldValue::Int(NULL_INDEX as u64))?;
    header.set("skel_index", FieldValue::Int(NULL_INDEX as u64))?;
    header.set("guide_index", FieldValue::Int(NULL_INDEX as u64))?;
    header.set("exth", FieldValue::Bytes(fields.exth))?;
    header.set("full_title", FieldValue::Bytes(fields.full_title))?;
    header.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobi::writer2::resources::{ResourceOpts, Resources};
    use crate::oeb::book::OEBBook;
    use crate::oeb::container::NullContainer;

    fn minimal_metadata() -> Metadata {
        let mut m = Metadata::new();
        m.add("title", "A Book");
        m.add("creator", "Author");
        m.add("date", "2020-01-01T00:00:00+00:00");
        m.add("language", "en");
        m
    }

    #[test]
    fn builds_a_well_formed_record0_and_pdb_header() {
        let metadata = minimal_metadata();
        let oeb = OEBBook::new(Box::new(NullContainer::new()));
        let mut resources = Resources::new(&oeb, ResourceOpts::default(), false, true);

        let inputs = KF8BuildInputs {
            last_text_record_idx: 1,
            first_non_text_record_idx: 2,
            records: vec![Vec::new(), b"hello world".to_vec()],
            text_length: 11,
            chunk_records: vec![b"CHUNKDATA".to_vec()],
            skel_records: vec![b"SKELDATA".to_vec()],
            guide_records: Vec::new(),
            ncx_records: Vec::new(),
            resources: &mut resources,
            used_images: HashSet::new(),
            fdst_count: 1,
            fdst_records: vec![b"FDSTDATA".to_vec()],
            compress: false,
            has_tbs: false,
            start_offset: None,
            metadata: &metadata,
            opts: Kf8Opts::default(),
            page_progression_direction: None,
            primary_writing_mode: None,
        };
        let book = KF8Book::new(inputs, false).unwrap();
        let bytes = book.to_bytes(&metadata).unwrap();
        assert_eq!(&bytes[60..68], b"BOOKMOBI");
        let record0 = book.record0(&metadata).unwrap();
        assert_eq!(&record0[16..20], b"MOBI");
    }
}
