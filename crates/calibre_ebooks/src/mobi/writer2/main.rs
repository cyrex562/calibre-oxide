//! `MobiWriter`: the top-level orchestrator that assembles a standalone
//! MOBI 6 `.mobi` file (PDB header, PalmDOC header, MOBI header, EXTH,
//! text records, image/font records, `INDX` tree, FLIS/FCIS/EOF) from an
//! `OEBBook`.
//!
//! Port of `calibre.ebooks.mobi.writer2.main.MobiWriter`.
//!
//! # Joint MOBI6+KF8 (`.azw3`) output
//!
//! Port of `MobiWriter.generate_joint_record0` (issue #157). A joint file
//! shares one `Resources` block between the MOBI6 writer (this module)
//! and a [`KF8Book`] (`mobi::writer8`, built with `for_joint = true` so
//! it skips serializing its own resource records) built by the caller.
//! [`MobiWriter::write_joint`] runs the same `generate_content` this
//! module's standalone [`MobiWriter::write`] uses (own text/index
//! records), then instead of [`MobiWriter::generate_record0`], appends:
//! the shared resource records (keyed off the *union* of both writers'
//! `used_images`), FLIS/FCIS, a literal `BOUNDARY` marker record, the
//! `KF8Book`'s own embedded `record0` (via
//! [`KF8Book::record0_for_joint`], which additionally EXTH-encodes this
//! writer's own `start_offset` alongside the KF8 book's), and the rest of
//! the `KF8Book`'s own record list. The MOBI6-view `record0` is then
//! built via [`build_joint_mobi6_record0`] (the same 264-byte KF8-style
//! header shape `KF8Book::record0` itself uses, just stamped
//! `file_version = 6`) with an EXTH `kf8_header_index` marker pointing at
//! the embedded KF8 header -- what the reader's `kf8_type == "joint"`
//! detection (`crate::mobi::mobi6`, issue #33) looks for.
//!
//! One real upstream quirk, disclosed rather than silently "fixed": the
//! MOBI6-view header's `chunk_index`/`skel_index`/`guide_index` are set
//! to `NULL_INDEX` unconditionally, not remapped into the combined
//! record list -- a MOBI6 reader has no use for KF8's own structural
//! indices, and Python's `generate_joint_record0` does the same (see
//! [`JointMobi6Fields`]'s own doc for detail).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::compression::palmdoc::compress;
use crate::mobi::langcodes::iana2mobi;
use crate::mobi::utils::{
    create_text_record, detect_periodical, encode_trailing_data, RECORD_SIZE,
};
use crate::mobi::writer2::indexer::Indexer;
use crate::mobi::writer2::resources::{ResourceOpts, Resources};
use crate::mobi::writer2::serializer::{urlnormalize, Serializer};
use crate::mobi::writer2::{PALMDOC, UNCOMPRESSED};
use crate::mobi::writer8::exth::{build_exth, ExthParams};
use crate::mobi::writer8::mobi::{build_joint_mobi6_record0, JointMobi6Fields, KF8Book, NULL_INDEX};
use crate::mobi::MobiLog;
use crate::oeb::book::OEBBook;

/// Fields Python reads off its `opts` object. `prefer_author_sort`,
/// `share_not_sync` and `mobi_keep_original_images` are forwarded
/// verbatim to [`Resources`]/[`build_exth`]; `dont_compress` picks
/// [`UNCOMPRESSED`] vs [`PALMDOC`] text-record compression.
#[derive(Debug, Clone, Copy)]
pub struct MobiWriterOpts {
    pub dont_compress: bool,
    pub prefer_author_sort: bool,
    pub share_not_sync: bool,
    pub mobi_keep_original_images: bool,
}

impl Default for MobiWriterOpts {
    fn default() -> Self {
        MobiWriterOpts {
            dont_compress: false,
            prefer_author_sort: false,
            // Python's `Opts.share_not_sync` defaults `True`.
            share_not_sync: true,
            mobi_keep_original_images: false,
        }
    }
}

/// FLIS record: fixed bytes, opaque to every reader calibre has ever
/// tested against ("Seems to serve no purpose" -- Python's own comment).
const FLIS: &[u8] = b"FLIS\0\0\0\x08\0\x41\0\0\0\0\0\0\xff\xff\xff\xff\0\x01\0\x03\0\0\0\x03\0\0\0\x01\xff\xff\xff\xff";

fn fcis(text_length: u32) -> Vec<u8> {
    let mut v = b"FCIS\x00\x00\x00\x14\x00\x00\x00\x10\x00\x00\x00\x01\x00\x00\x00\x00".to_vec();
    v.extend_from_slice(&text_length.to_be_bytes());
    v.extend_from_slice(
        b"\x00\x00\x00\x00\x00\x00\x00\x20\x00\x00\x00\x08\x00\x01\x00\x01\x00\x00\x00\x00",
    );
    v
}

fn nfc(s: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfc(s).collect()
}

/// Everything [`MobiWriter::generate_record0`] needs beyond `records`
/// itself, gathered here so the byte-assembly function doesn't have a
/// dozen positional parameters at the call site.
struct Record0Inputs<'a> {
    is_periodical: bool,
    primary_index_record_idx: Option<usize>,
    indexer: Option<&'a Indexer>,
    cover_offset: Option<usize>,
    thumbnail_offset: Option<usize>,
    text_length: usize,
    last_text_record_idx: usize,
    first_non_text_record_idx: usize,
}

/// Output of [`MobiWriter::generate_content`]: everything both
/// [`MobiWriter::write`] and [`MobiWriter::write_joint`] need to build
/// `records[0]`, owned (not borrowed) so both callers can build their
/// own [`Record0Inputs`]-shaped view (or, for the joint path, mutate the
/// record list directly) without fighting the borrow checker over an
/// `Indexer` reference.
struct ContentParts {
    records: Vec<Vec<u8>>,
    /// `Serializer::used_images`/`Serializer::start_offset` -- the only
    /// two `Serializer` fields either `generate_record0` or
    /// `generate_joint_record0` reads, pulled out here rather than
    /// storing the `Serializer` itself (which borrows a `HashMap` local
    /// to [`MobiWriter::generate_content`] and so can't outlive it).
    used_images: HashSet<String>,
    start_offset: Option<usize>,
    is_periodical: bool,
    cover_offset: Option<usize>,
    thumbnail_offset: Option<usize>,
    text_length: usize,
    last_text_record_idx: usize,
    first_non_text_record_idx: usize,
    primary_index_record_idx: Option<usize>,
    indexer: Option<Indexer>,
}

/// Assembles a standalone MOBI 6 file from an `OEBBook`.
pub struct MobiWriter {
    opts: MobiWriterOpts,
    write_page_breaks_after_item: bool,
    /// Diagnostic messages accumulated during the last [`Self::write`]
    /// call (index-generation failures, dangling links, ...).
    pub log: MobiLog,
}

impl MobiWriter {
    pub fn new(opts: MobiWriterOpts) -> Self {
        MobiWriter {
            opts,
            write_page_breaks_after_item: true,
            log: MobiLog::default(),
        }
    }

    pub fn with_page_breaks_after_item(mut self, value: bool) -> Self {
        self.write_page_breaks_after_item = value;
        self
    }

    /// Encode `oeb` as a complete `.mobi` file. Port of
    /// `MobiWriter.dump_stream` (the `kf8 = None` path: `generate_content`
    /// + `generate_record0` + `write_header` + `write_content`).
    pub fn write(&mut self, oeb: &OEBBook) -> Result<Vec<u8>> {
        let is_periodical = detect_periodical(&oeb.toc, Some(&mut self.log));
        let resource_opts = ResourceOpts {
            mobi_keep_original_images: self.opts.mobi_keep_original_images,
        };
        // Fonts are KF8-only content, so a standalone MOBI6 file never
        // needs them (`add_fonts = false`, matching Python's
        // `Resources(oeb, opts, is_periodical, add_fonts=create_kf8)`
        // with `create_kf8 = False` here).
        let mut resources = Resources::new(oeb, resource_opts, is_periodical, false);

        let mut parts = self.generate_content(oeb, &mut resources)?;

        let inputs = Record0Inputs {
            is_periodical: parts.is_periodical,
            primary_index_record_idx: parts.primary_index_record_idx,
            indexer: parts.indexer.as_ref(),
            cover_offset: parts.cover_offset,
            thumbnail_offset: parts.thumbnail_offset,
            text_length: parts.text_length,
            last_text_record_idx: parts.last_text_record_idx,
            first_non_text_record_idx: parts.first_non_text_record_idx,
        };
        self.generate_record0(
            oeb,
            &mut parts.records,
            &mut resources,
            &parts.used_images,
            parts.start_offset,
            &inputs,
        )?;

        let mut out = Vec::new();
        self.write_header(oeb, &parts.records, &mut out)?;
        for record in &parts.records {
            out.extend_from_slice(record);
        }
        Ok(out)
    }

    /// Encode `oeb` as a joint MOBI6+KF8 (`.azw3`) file, sharing one
    /// resource block with `kf8`. Port of `MobiWriter.dump_stream`'s
    /// `kf8 is not None` path (`generate_content` + `generate_joint_record0`
    /// + `write_header` + `write_content`). See the module doc for the
    /// real interleaving this performs.
    ///
    /// `resources` must be the *same* `Resources` instance `kf8` itself
    /// was built from (via `KF8Writer::write_for_joint`) -- real
    /// upstream constructs one `Resources` object in the output plugin
    /// and threads it into both sub-writers by reference, rather than
    /// each independently deriving its own copy.
    pub fn write_joint(&mut self, oeb: &OEBBook, kf8: &KF8Book, resources: &mut Resources) -> Result<Vec<u8>> {
        let mut parts = self.generate_content(oeb, resources)?;
        self.generate_joint_record0(oeb, &mut parts, resources, kf8)?;

        let mut out = Vec::new();
        self.write_header(oeb, &parts.records, &mut out)?;
        for record in &parts.records {
            out.extend_from_slice(record);
        }
        Ok(out)
    }

    /// Port of `MobiWriter.generate_content`: resource bookkeeping, text
    /// serialization/compression/splitting into records, and (if the
    /// `OEBBook` has a TOC) the MOBI index. Shared by both [`Self::write`]
    /// and [`Self::write_joint`], which only differ in how `records[0]`
    /// gets built afterwards (and, for the joint path, in whether
    /// `resources` is this writer's own or shared with a sibling
    /// [`KF8Book`]).
    fn generate_content(&mut self, oeb: &OEBBook, resources: &mut Resources) -> Result<ContentParts> {
        if !resources.records.is_empty() && resources.records[0].is_none() {
            anyhow::bail!("Failed to find masthead image in manifest");
        }
        let is_periodical = detect_periodical(&oeb.toc, Some(&mut self.log));
        let masthead_offset = resources.masthead_offset;
        let cover_offset = resources.cover_offset;
        let thumbnail_offset = resources.thumbnail_offset;

        let image_map: HashMap<String, usize> = resources
            .item_map
            .iter()
            .map(|(href, idx)| (urlnormalize(href), *idx))
            .collect();

        let mut records: Vec<Vec<u8>> = vec![Vec::new()];

        let mut serializer = Serializer::new(
            oeb,
            &image_map,
            is_periodical,
            self.write_page_breaks_after_item,
        );
        let text = serializer.serialize();
        for w in serializer.warnings() {
            self.log.warn(w.clone());
        }
        let text_length = text.len();

        let mut pos = 0usize;
        let mut records_size = 0usize;
        while pos < text.len() {
            let (mut data, overlap) = create_text_record(&text, &mut pos);
            if !self.opts.dont_compress {
                data = compress(&data).unwrap_or(data);
            }
            data.extend_from_slice(&overlap);
            data.push(overlap.len() as u8);
            records_size += data.len();
            records.push(data);
        }
        let last_text_record_idx = records.len() - 1;
        let mut first_non_text_record_idx = last_text_record_idx + 1;
        if !records_size.is_multiple_of(4) {
            records.push(vec![0u8; records_size % 4]);
            first_non_text_record_idx += 1;
        }

        // `write_uncrossable_breaks` is a no-op here: Python's own
        // `WRITE_UNCROSSABLE_BREAKS` module constant is `False`
        // ("Disabled as I don't care about uncrossable breaks").

        let mut primary_index_record_idx: Option<usize> = None;
        let mut indexer: Option<Indexer> = None;
        if oeb.toc.count() < 1 {
            self.log.warn("No TOC, MOBI index not generated");
        } else {
            match Indexer::new(
                &serializer,
                oeb,
                last_text_record_idx,
                is_periodical,
                masthead_offset,
            ) {
                Ok(idx) => {
                    for w in &idx.warnings {
                        self.log.warn(w.clone());
                    }
                    primary_index_record_idx = Some(records.len());
                    // `i` is both the text-record *number* passed to
                    // `get_trailing_byte_sequence` and the `records`
                    // index -- record 0 is the (not yet built) header,
                    // so text record `i` really does live at
                    // `records[i]`. Not a candidate for `.iter_mut()`.
                    #[allow(clippy::needless_range_loop)]
                    for i in 1..=last_text_record_idx {
                        let tbs = idx.get_trailing_byte_sequence(i).to_vec();
                        records[i].extend(encode_trailing_data(&tbs));
                    }
                    records.extend(idx.records.clone());
                    indexer = Some(idx);
                }
                Err(e) => {
                    self.log.warn(format!("Failed to generate MOBI index: {e}"));
                }
            }
        }

        Ok(ContentParts {
            records,
            used_images: serializer.used_images,
            start_offset: serializer.start_offset,
            is_periodical,
            cover_offset,
            thumbnail_offset,
            text_length,
            last_text_record_idx,
            first_non_text_record_idx,
            primary_index_record_idx,
            indexer,
        })
    }

    /// Port of `MobiWriter.generate_joint_record0`. See the module doc
    /// for the full real interleaving this performs.
    fn generate_joint_record0(
        &mut self,
        oeb: &OEBBook,
        parts: &mut ContentParts,
        resources: &mut Resources,
        kf8: &KF8Book,
    ) -> Result<()> {
        // Shared resource block: the union of both sides' used images,
        // serialized once into the combined record list.
        let mut first_image_record: Option<usize> = None;
        let before_resources = parts.records.len();
        if !resources.is_empty() {
            let mut used_images = parts.used_images.clone();
            used_images.extend(kf8.used_images.iter().cloned());
            first_image_record = Some(parts.records.len());
            resources.serialize(&mut parts.records, &used_images);
        }
        let resource_record_count = parts.records.len() - before_resources;
        let last_content_record = parts.records.len() - 1;

        let flis_number = parts.records.len();
        parts.records.push(FLIS.to_vec());
        let fcis_number = parts.records.len();
        parts.records.push(fcis(parts.text_length as u32));

        // Insert the KF8 half: a literal `BOUNDARY` marker, the KF8
        // book's own embedded record0 (EXTH-encoding both writers'
        // start_offset), then the rest of its own record list.
        parts.records.push(b"BOUNDARY".to_vec());
        let kf8_header_index = parts.records.len();
        let kf8_record0 = kf8
            .record0_for_joint(&oeb.metadata, parts.start_offset.map(|v| v as u32))
            .context("building joint KF8 record0")?;
        parts.records.push(kf8_record0);
        parts.records.extend(kf8.tail_records().iter().cloned());

        let first_image_record = first_image_record.unwrap_or(parts.records.len());

        let mut exth_flags: u32 = 0b100001010000; // Kindlegen uses this
        if resources.has_fonts {
            exth_flags |= 0b1000000000000;
        }

        // MOBI6 has no real FDST record: this field packs the first/last
        // content record numbers as two big-endian u16s instead
        // (`pack('>HH', 1, last_content_record)` in Python).
        let fdst_record_packed = (1u32 << 16) | (last_content_record as u32 & 0xffff);

        let mut extra_data_flags: u32 = 0b1;
        if parts.primary_index_record_idx.is_some() {
            extra_data_flags |= 0b10;
        }
        let kuc = if resource_record_count > 0 {
            Some(0u32)
        } else {
            None
        };

        // Real upstream reads `opts.mobi_periodical` here rather than
        // this writer's own detected `is_periodical` -- moot in
        // practice, since `mobi_output.py` forces `mobi_type = 'old'`
        // (no KF8 at all) whenever a book is a periodical, so
        // `generate_joint_record0` never actually runs for one; using
        // the detected value is equivalent for every real caller.
        let exth_params = ExthParams {
            prefer_author_sort: self.opts.prefer_author_sort,
            is_periodical: parts.is_periodical,
            share_not_sync: self.opts.share_not_sync,
            cover_offset: parts.cover_offset.map(|v| v as u32),
            thumbnail_offset: parts.thumbnail_offset.map(|v| v as u32),
            num_of_resources: Some(resource_record_count as u32),
            kf8_unknown_count: kuc,
            kf8_header_index: Some(kf8_header_index as u32),
            be_kindlegen2: true,
            start_offset: parts.start_offset.map(|v| v as u32),
            start_offset_secondary: None,
            mobi_doctype: 2,
            page_progression_direction: None,
            primary_writing_mode: None,
        };
        let exth = build_exth(&oeb.metadata, &exth_params).context("building joint MOBI6 EXTH header")?;

        let ncx_index = parts
            .primary_index_record_idx
            .map(|v| v as u32)
            .unwrap_or(NULL_INDEX);

        let fields = JointMobi6Fields {
            compression: kf8.compression(),
            text_length: parts.text_length as u32,
            last_text_record: parts.last_text_record_idx as u32,
            book_type: kf8.book_type(),
            first_non_text_record: parts.first_non_text_record_idx as u32,
            language_code: kf8.language_code(),
            first_resource_record: first_image_record as u32,
            exth_flags,
            fdst_record_packed,
            ncx_index,
            extra_data_flags,
            flis_record: flis_number as u32,
            fcis_record: fcis_number as u32,
            uid: kf8.uid(),
            full_title: kf8.full_title().to_vec(),
            exth,
        };
        parts.records[0] = build_joint_mobi6_record0(fields)?;
        Ok(())
    }

    /// Build `records[0]` (the PalmDOC + MOBI + EXTH + title header) and
    /// append the image/font/FLIS/FCIS/EOF records. Port of
    /// `generate_record0`.
    fn generate_record0(
        &mut self,
        oeb: &OEBBook,
        records: &mut Vec<Vec<u8>>,
        resources: &mut Resources,
        used_images: &HashSet<String>,
        start_offset: Option<usize>,
        inputs: &Record0Inputs,
    ) -> Result<()> {
        let mut bt: u32 = 0x002;
        if let Some(idx) = inputs.indexer {
            if inputs.primary_index_record_idx.is_some() && idx.is_periodical {
                bt = if idx.is_flat_periodical { 0x103 } else { 0x101 };
            }
        }

        let exth_params = ExthParams {
            prefer_author_sort: self.opts.prefer_author_sort,
            is_periodical: inputs.is_periodical,
            share_not_sync: self.opts.share_not_sync,
            cover_offset: inputs.cover_offset.map(|v| v as u32),
            thumbnail_offset: inputs.thumbnail_offset.map(|v| v as u32),
            start_offset: start_offset.map(|v| v as u32),
            mobi_doctype: bt,
            ..Default::default()
        };
        let exth = build_exth(&oeb.metadata, &exth_params).context("building EXTH header")?;

        let mut first_image_record: Option<usize> = None;
        if !resources.is_empty() {
            first_image_record = Some(records.len());
            resources.serialize(records, used_images);
        }
        let last_content_record = records.len() - 1;

        let flis_number = records.len();
        records.push(FLIS.to_vec());
        let fcis_number = records.len();
        records.push(fcis(inputs.text_length as u32));
        records.push(vec![0xE9, 0x8E, 0x0D, 0x0A]);

        let metadata = &oeb.metadata;
        let title = metadata
            .get("title")
            .first()
            .map(|i| nfc(&i.value))
            .unwrap_or_else(|| "Unknown".to_string());
        let title_bytes = title.into_bytes();
        let language = metadata
            .get("language")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "en".to_string());
        let uid: u32 = rand::random();

        let mut r0 = Vec::with_capacity(0x2000);

        // --- PalmDOC header (16 bytes): offsets [0x00, 0x10) ---
        let compression: u16 = if self.opts.dont_compress {
            UNCOMPRESSED
        } else {
            PALMDOC
        };
        r0.extend_from_slice(&compression.to_be_bytes());
        r0.extend_from_slice(&0u16.to_be_bytes());
        r0.extend_from_slice(&(inputs.text_length as u32).to_be_bytes());
        r0.extend_from_slice(&(inputs.last_text_record_idx as u16).to_be_bytes());
        r0.extend_from_slice(&(RECORD_SIZE as u16).to_be_bytes());
        r0.extend_from_slice(&0u16.to_be_bytes());
        r0.extend_from_slice(&0u16.to_be_bytes());

        // --- MOBI header: starts at absolute offset 0x10 ---
        r0.extend_from_slice(b"MOBI");
        r0.extend_from_slice(&0xe8u32.to_be_bytes()); // header length (232)
        r0.extend_from_slice(&bt.to_be_bytes()); // mobi type
        r0.extend_from_slice(&65001u32.to_be_bytes()); // text encoding: utf-8
        r0.extend_from_slice(&uid.to_be_bytes());
        r0.extend_from_slice(&6u32.to_be_bytes()); // generator version
                                                   // absolute 0x28
        r0.extend_from_slice(&[0xffu8; 8]); // ortographic/inflection index
                                            // absolute 0x30: secondary index record
        let sir: u32 = match (
            inputs.primary_index_record_idx,
            inputs.indexer.and_then(|i| i.secondary_record_offset),
        ) {
            (Some(pir), Some(off)) => (pir + off) as u32,
            _ => 0xffffffff,
        };
        r0.extend_from_slice(&sir.to_be_bytes());
        // absolute 0x34
        r0.extend_from_slice(&[0xffu8; 28]); // index_keys + extra_index_0..5
                                             // absolute 0x50: first non-text (non-book) record index
        r0.extend_from_slice(&(inputs.first_non_text_record_idx as u32).to_be_bytes());
        // absolute 0x54: title offset, title length
        let title_offset = 0xe8u32 + exth.len() as u32;
        r0.extend_from_slice(&title_offset.to_be_bytes());
        r0.extend_from_slice(&(title_bytes.len() as u32).to_be_bytes());
        // absolute 0x5c: language specifier
        r0.extend_from_slice(&iana2mobi(&language));
        // absolute 0x60: input/output language
        r0.extend_from_slice(&[0u8; 8]);
        // absolute 0x68: format version, first image record number
        r0.extend_from_slice(&6u32.to_be_bytes());
        r0.extend_from_slice(&(first_image_record.unwrap_or(records.len()) as u32).to_be_bytes());
        // absolute 0x70: huff/cdic + datp fields (unused by our writer)
        r0.extend_from_slice(&[0u8; 16]);
        // absolute 0x80: EXTH flags
        let mut exth_flags: u32 = 0b1010000;
        if inputs.is_periodical {
            exth_flags |= 0b1000;
        }
        if resources.has_fonts {
            exth_flags |= 0b1000000000000;
        }
        r0.extend_from_slice(&exth_flags.to_be_bytes());
        // absolute 0x84: reserved
        r0.extend_from_slice(&[0u8; 32]);
        // absolute 0xa4: DRM offset/count/size/flags
        r0.extend_from_slice(&0xffffffffu32.to_be_bytes());
        r0.extend_from_slice(&0xffffffffu32.to_be_bytes());
        r0.extend_from_slice(&0u32.to_be_bytes());
        r0.extend_from_slice(&0u32.to_be_bytes());
        // absolute 0xb4: reserved
        r0.extend_from_slice(&[0u8; 12]);
        // absolute 0xc0: first/last content record
        r0.extend_from_slice(&1u16.to_be_bytes());
        r0.extend_from_slice(&(last_content_record as u16).to_be_bytes());
        // absolute 0xc4
        r0.extend_from_slice(&[0u8, 0, 0, 1]);
        // absolute 0xc8: FCIS record number
        r0.extend_from_slice(&(fcis_number as u32).to_be_bytes());
        // absolute 0xcc
        r0.extend_from_slice(&1u32.to_be_bytes());
        // absolute 0xd0: FLIS record number
        r0.extend_from_slice(&(flis_number as u32).to_be_bytes());
        // absolute 0xd4
        r0.extend_from_slice(&1u32.to_be_bytes());
        // absolute 0xd8: reserved
        r0.extend_from_slice(&[0u8; 8]);
        // absolute 0xe0: reserved
        r0.extend_from_slice(&0xffffffffu32.to_be_bytes());
        r0.extend_from_slice(&0u32.to_be_bytes());
        r0.extend_from_slice(&0xffffffffu32.to_be_bytes());
        r0.extend_from_slice(&0xffffffffu32.to_be_bytes());
        // absolute 0xf0: extra record data flags
        let mut extra_data_flags: u32 = 0b1; // multibyte overlap bytes
        if inputs.primary_index_record_idx.is_some() {
            extra_data_flags |= 0b10;
        }
        r0.extend_from_slice(&extra_data_flags.to_be_bytes());
        // absolute 0xf4: primary index record
        r0.extend_from_slice(
            &(inputs
                .primary_index_record_idx
                .map(|v| v as u32)
                .unwrap_or(0xffffffff))
            .to_be_bytes(),
        );

        debug_assert_eq!(
            r0.len(),
            0xf8,
            "MOBI fixed header must be exactly 232 bytes"
        );

        r0.extend_from_slice(&exth);
        r0.extend_from_slice(&title_bytes);
        // Buffer so Amazon-side tooling can inject encryption info.
        r0.extend(std::iter::repeat_n(0u8, 1024 * 8));
        records[0] = crate::mobi::utils::align_block(&r0, 4, 0);

        Ok(())
    }

    /// Write the PDB (PalmDB) header: 32-byte name, standard PalmDB
    /// fields, `BOOK`/`MOBI` type/creator, and the record offset table.
    /// Port of `MobiWriter.write_header`.
    fn write_header(&self, oeb: &OEBBook, records: &[Vec<u8>], out: &mut Vec<u8>) -> Result<()> {
        let title = oeb
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        let ascii_title = calibre_utils::filenames::ascii_filename(&title).replace(' ', "_");
        let mut name = ascii_title.into_bytes();
        name.truncate(31);
        name.resize(32, 0);
        out.extend_from_slice(&name);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        out.extend_from_slice(&0u16.to_be_bytes()); // attributes
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&now.to_be_bytes()); // creation date
        out.extend_from_slice(&now.to_be_bytes()); // modification date
        out.extend_from_slice(&0u32.to_be_bytes()); // backup date
        out.extend_from_slice(&0u32.to_be_bytes()); // modification number
        out.extend_from_slice(&0u32.to_be_bytes()); // app info id
        out.extend_from_slice(&0u32.to_be_bytes()); // sort info id
        out.extend_from_slice(b"BOOK");
        out.extend_from_slice(b"MOBI");
        let nrecords = records.len();
        out.extend_from_slice(&((2 * nrecords - 1) as u32).to_be_bytes()); // unique id seed
        out.extend_from_slice(&0u32.to_be_bytes()); // next record list id
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
        Ok(())
    }
}
