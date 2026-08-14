//! `MobiWriter`: the top-level orchestrator that assembles a standalone
//! MOBI 6 `.mobi` file (PDB header, PalmDOC header, MOBI header, EXTH,
//! text records, image/font records, `INDX` tree, FLIS/FCIS/EOF) from an
//! `OEBBook`.
//!
//! Port of `calibre.ebooks.mobi.writer2.main.MobiWriter`.
//!
//! # Scope: joint MOBI6+KF8 (`.azw3`) output
//!
//! Python's `MobiWriter.__init__` takes an optional `kf8` (a
//! `Mobi8Writer` from the still-unported `mobi/writer8`), and when
//! present, `dump_stream` calls `generate_joint_record0` instead of
//! `generate_record0` to interleave a second, KF8-format `record0` and
//! record set into the same file. `mobi/writer8` has not been ported
//! (a separate, future issue -- see `writer2/mod.rs`'s module doc), so
//! [`MobiWriter::write_joint`] takes the real Python call shape
//! (`kf8: Option<&()>` standing in for the not-yet-existing
//! `Mobi8Writer` trait/type) and is a documented `todo!()`. The
//! standalone path ([`MobiWriter::write`], `kf8 = None` in Python) is
//! fully implemented.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::compression::palmdoc::compress;
use crate::mobi::langcodes::iana2mobi;
use crate::mobi::utils::{
    create_text_record, detect_periodical, encode_trailing_data, RECORD_SIZE,
};
use crate::mobi::writer2::exth::{build_exth, ExthParams};
use crate::mobi::writer2::indexer::Indexer;
use crate::mobi::writer2::resources::{ResourceOpts, Resources};
use crate::mobi::writer2::serializer::{urlnormalize, Serializer};
use crate::mobi::writer2::{PALMDOC, UNCOMPRESSED};
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
        let mut resources = Resources::new(oeb, resource_opts, is_periodical, false);
        if !resources.records.is_empty() && resources.records[0].is_none() {
            anyhow::bail!("Failed to find masthead image in manifest");
        }
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

        let inputs = Record0Inputs {
            is_periodical,
            primary_index_record_idx,
            indexer: indexer.as_ref(),
            cover_offset,
            thumbnail_offset,
            text_length,
            last_text_record_idx,
            first_non_text_record_idx,
        };
        self.generate_record0(oeb, &mut records, &mut resources, &serializer, &inputs)?;

        let mut out = Vec::new();
        self.write_header(oeb, &records, &mut out)?;
        for record in &records {
            out.extend_from_slice(record);
        }
        Ok(out)
    }

    /// Placeholder for the joint MOBI6+KF8 (`.azw3`) output path.
    ///
    /// `kf8` stands in for Python's `Mobi8Writer` (`mobi/writer8/main.py`,
    /// not yet ported). See the module scope note.
    pub fn write_joint(&mut self, _oeb: &OEBBook, _kf8: Option<&()>) -> Result<Vec<u8>> {
        todo!(
            "placeholder: joint KF8 output requires mobi/writer8, not yet ported — see issue #34 scope note"
        )
    }

    /// Build `records[0]` (the PalmDOC + MOBI + EXTH + title header) and
    /// append the image/font/FLIS/FCIS/EOF records. Port of
    /// `generate_record0`.
    fn generate_record0(
        &mut self,
        oeb: &OEBBook,
        records: &mut Vec<Vec<u8>>,
        resources: &mut Resources,
        serializer: &Serializer,
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
            start_offset: serializer.start_offset.map(|v| v as u32),
            mobi_doctype: bt,
            ..Default::default()
        };
        let exth = build_exth(&oeb.metadata, &exth_params).context("building EXTH header")?;

        let mut first_image_record: Option<usize> = None;
        if !resources.is_empty() {
            let used_images = serializer.used_images.clone();
            first_image_record = Some(records.len());
            resources.serialize(records, &used_images);
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
