use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

use crate::html_entities::decode_entities;
use crate::metadata::{check_isbn, MetaInformation};
use crate::mobi::langcodes::mobi2iana;

/// The common header for all PDB-based files is handled loosely here or via `pdb` module.
/// Here we focus on the specific MOBI headers which follow the PDB header's Record 0.
pub const NULL_INDEX: u32 = 0xFFFFFFFF;

/// Port of `calibre.utils.cleantext.clean_ascii_chars`: strips ASCII control
/// characters (everything below 0x20 except tab/lf/cr, plus DEL) without
/// touching non-ASCII text. This is intentionally *not* the same as
/// `crate::conversion::utils::clean_ascii_chars`, which strips all non-ASCII
/// bytes and would corrupt Unicode titles/authors if reused here.
fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let cp = c as u32;
            !(cp < 0x20 && cp != 9 && cp != 10 && cp != 13) && cp != 127
        })
        .collect()
}

fn decode_with_codec(data: &[u8], codec: &str) -> String {
    if codec.eq_ignore_ascii_case("utf-8") {
        String::from_utf8_lossy(data).into_owned()
    } else {
        match encoding_rs::Encoding::for_label(codec.as_bytes()) {
            Some(enc) => enc.decode(data).0.into_owned(),
            None => String::from_utf8_lossy(data).into_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PalmDocHeader {
    pub compression: u16,
    pub unused: u16,
    pub text_length: u32,
    pub record_count: u16,
    pub record_size: u16,
    pub encryption_type: u16,
    pub unknown: u16,
}

impl PalmDocHeader {
    pub fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let compression = reader.read_u16::<BigEndian>()?;
        let unused = reader.read_u16::<BigEndian>()?;
        let text_length = reader.read_u32::<BigEndian>()?;
        let record_count = reader.read_u16::<BigEndian>()?;
        let record_size = reader.read_u16::<BigEndian>()?;
        let encryption_type = reader.read_u16::<BigEndian>()?;
        let unknown = reader.read_u16::<BigEndian>()?;

        Ok(PalmDocHeader {
            compression,
            unused,
            text_length,
            record_count,
            record_size,
            encryption_type,
            unknown,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MobiHeader {
    pub identifier: String, // "MOBI"
    pub header_length: u32,
    pub mobi_type: u32,
    pub text_encoding: u32,
    pub unique_id: u32,
    pub file_version: u32,
    pub ortographic_index: u32,
    pub inflection_index: u32,
    pub index_names: u32,
    pub index_keys: u32,
    pub extra_index_0: u32,
    pub extra_index_1: u32,
    pub extra_index_2: u32,
    pub extra_index_3: u32,
    pub extra_index_4: u32,
    pub extra_index_5: u32,
    pub first_non_book_index: u32,
    pub full_name_offset: u32,
    pub full_name_length: u32,
    pub locale: u32,
    pub input_language: u32,
    pub output_language: u32,
    pub min_version: u32,
    pub first_image_index: u32,
    pub huffman_record_offset: u32,
    pub huffman_record_count: u32,
    pub huffman_table_offset: u32,
    pub huffman_table_length: u32,
    pub exth_flags: u32,
    pub drm_offset: u32,
    pub drm_count: u32,
    pub drm_size: u32,
    pub drm_flags: u32,
}

impl MobiHeader {
    /// Parses the fixed-position portion of the MOBI header (up to and
    /// including the DRM fields, offsets 0x10..0xB4 relative to the start of
    /// the PDB record). This matches `struct.unpack` calls in
    /// `headers.py`'s `BookHeader.__init__` up to `self.exth_flag`/DRM info.
    /// The variable-length tail (extra_flags, KF8 index fields) depends on
    /// the *total record length*, not on anything `MobiHeader` alone knows,
    /// so it is parsed directly by `BookHeader::parse` instead.
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        // Assume reader positioned start of MOBI header (after PalmDoc)
        let mut ident = [0u8; 4];
        reader.read_exact(&mut ident)?;
        let identifier = String::from_utf8_lossy(&ident).to_string();

        if identifier != "MOBI" {
            bail!("Invalid MOBI identifier: {}", identifier);
        }

        let header_length = reader.read_u32::<BigEndian>()?;
        let mobi_type = reader.read_u32::<BigEndian>()?;
        let text_encoding = reader.read_u32::<BigEndian>()?;
        let unique_id = reader.read_u32::<BigEndian>()?;
        let file_version = reader.read_u32::<BigEndian>()?;

        let ortographic_index = reader.read_u32::<BigEndian>()?;
        let inflection_index = reader.read_u32::<BigEndian>()?;
        let index_names = reader.read_u32::<BigEndian>()?;
        let index_keys = reader.read_u32::<BigEndian>()?;
        let extra_index_0 = reader.read_u32::<BigEndian>()?;
        let extra_index_1 = reader.read_u32::<BigEndian>()?;
        let extra_index_2 = reader.read_u32::<BigEndian>()?;
        let extra_index_3 = reader.read_u32::<BigEndian>()?;
        let extra_index_4 = reader.read_u32::<BigEndian>()?;
        let extra_index_5 = reader.read_u32::<BigEndian>()?;

        let first_non_book_index = reader.read_u32::<BigEndian>()?;
        let full_name_offset = reader.read_u32::<BigEndian>()?;
        let full_name_length = reader.read_u32::<BigEndian>()?;
        let locale = reader.read_u32::<BigEndian>()?;
        let input_language = reader.read_u32::<BigEndian>()?;
        let output_language = reader.read_u32::<BigEndian>()?;
        let min_version = reader.read_u32::<BigEndian>()?;
        let first_image_index = reader.read_u32::<BigEndian>()?;

        let huffman_record_offset = reader.read_u32::<BigEndian>()?;
        let huffman_record_count = reader.read_u32::<BigEndian>()?;
        let huffman_table_offset = reader.read_u32::<BigEndian>()?;
        let huffman_table_length = reader.read_u32::<BigEndian>()?;

        let exth_flags = reader.read_u32::<BigEndian>()?;

        // 32 bytes of reserved fields between exth_flags (offset 0x80) and
        // the DRM fields (offset 0xA4).
        reader.seek(SeekFrom::Current(32))?;

        let drm_offset = reader.read_u32::<BigEndian>()?;
        let drm_count = reader.read_u32::<BigEndian>()?;
        let drm_size = reader.read_u32::<BigEndian>()?;
        let drm_flags = reader.read_u32::<BigEndian>()?;

        Ok(MobiHeader {
            identifier,
            header_length,
            mobi_type,
            text_encoding,
            unique_id,
            file_version,
            ortographic_index,
            inflection_index,
            index_names,
            index_keys,
            extra_index_0,
            extra_index_1,
            extra_index_2,
            extra_index_3,
            extra_index_4,
            extra_index_5,
            first_non_book_index,
            full_name_offset,
            full_name_length,
            locale,
            input_language,
            output_language,
            min_version,
            first_image_index,
            huffman_record_offset,
            huffman_record_count,
            huffman_table_offset,
            huffman_table_length,
            exth_flags,
            drm_offset,
            drm_count,
            drm_size,
            drm_flags,
        })
    }
}

/// Port of `headers.py`'s `EXTHHeader`. `records` holds the raw
/// `(type, data)` pairs (used as-is by `crate::mobi::reader` and
/// `calibre_devices::kindle::apnx` for simple tag lookups); call
/// [`ExthHeader::process`] to populate the higher-level fields
/// (`mi`, `kf8_header`, `cover_offset`, ...) the way Python's
/// `EXTHHeader.__init__` does inline.
#[derive(Debug, Clone)]
pub struct ExthHeader {
    pub records: Vec<(u32, Vec<u8>)>,
    pub mi: MetaInformation,
    pub has_fake_cover: bool,
    pub start_offset: Option<u32>,
    pub kf8_header: Option<u32>,
    pub uuid: Option<String>,
    pub cdetype: Option<String>,
    pub cover_offset: Option<u32>,
    pub thumbnail_offset: Option<u32>,
    pub page_progression_direction: Option<String>,
    pub primary_writing_mode: Option<String>,
}

impl Default for ExthHeader {
    fn default() -> Self {
        ExthHeader {
            records: Vec::new(),
            mi: MetaInformation::new("Unknown", vec!["Unknown".to_string()]),
            has_fake_cover: true,
            start_offset: None,
            kf8_header: None,
            uuid: None,
            cdetype: None,
            cover_offset: None,
            thumbnail_offset: None,
            page_progression_direction: None,
            primary_writing_mode: None,
        }
    }
}

impl ExthHeader {
    /// Parses only the raw `(type, data)` records, matching the wire
    /// format. Does not interpret them into metadata -- see
    /// [`ExthHeader::process`].
    pub fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let mut ident = [0u8; 4];
        reader.read_exact(&mut ident)?;
        if &ident != b"EXTH" {
            // Not an EXTH header
            bail!("EXTH header not found");
        }

        let _header_len = reader.read_u32::<BigEndian>()?;
        let record_count = reader.read_u32::<BigEndian>()?;

        let mut records = Vec::new();
        for _ in 0..record_count {
            let record_type = reader.read_u32::<BigEndian>()?;
            let record_len = reader.read_u32::<BigEndian>()?;
            if record_len < 8 {
                bail!("Invalid EXTH record length");
            }
            let data_len = record_len - 8;
            let mut data = vec![0u8; data_len as usize];
            reader.read_exact(&mut data)?;
            records.push((record_type, data));
        }

        Ok(ExthHeader {
            records,
            ..Default::default()
        })
    }

    /// Port of `EXTHHeader.__init__`'s record-walking loop and
    /// `process_metadata`. `codec` is the book's text codec
    /// (`BookHeader::codec`). Python's original only ever sets `mi.title`
    /// from EXTH record 503 (long title); if that record is absent, we
    /// leave `mi.title` at its default rather than reproducing Python's
    /// `UnboundLocalError` footgun for that case.
    pub fn process(&mut self, codec: &str) {
        self.mi = MetaInformation::new("Unknown", vec!["Unknown".to_string()]);
        self.has_fake_cover = true;
        self.start_offset = None;
        self.kf8_header = None;
        self.uuid = None;
        self.cdetype = None;
        self.cover_offset = None;
        self.thumbnail_offset = None;
        self.page_progression_direction = None;
        self.primary_writing_mode = None;

        let mut found_authors = false;
        let mut author_sort_accum: Option<String> = None;
        let mut title: Option<String> = None;

        let decode =
            |data: &[u8]| -> String { strip_control_chars(&decode_with_codec(data, codec)) };

        let records = self.records.clone();
        for (idx, content) in records {
            if (100..200).contains(&idx) {
                self.process_metadata_record(
                    idx,
                    &content,
                    codec,
                    &decode,
                    &mut found_authors,
                    &mut author_sort_accum,
                );
            } else {
                match idx {
                    203 => {
                        if content.len() >= 4 {
                            let v = u32::from_be_bytes([
                                content[0], content[1], content[2], content[3],
                            ]);
                            self.has_fake_cover = v != 0;
                        }
                    }
                    201 => {
                        if content.len() >= 4 {
                            let co = u32::from_be_bytes([
                                content[0], content[1], content[2], content[3],
                            ]);
                            if co < NULL_INDEX {
                                self.cover_offset = Some(co);
                            }
                        }
                    }
                    202 => {
                        if content.len() >= 4 {
                            self.thumbnail_offset = Some(u32::from_be_bytes([
                                content[0], content[1], content[2], content[3],
                            ]));
                        }
                    }
                    501 => {
                        if let Ok(s) = std::str::from_utf8(&content) {
                            self.cdetype = Some(s.to_string());
                        } else {
                            self.cdetype = None;
                        }
                        if content == b"EBSP" {
                            self.mi.tags.push("Sample Book".to_string());
                        }
                    }
                    503 => {
                        title = Some(decode(&content));
                    }
                    524 => {
                        let lang = decode_with_codec(&content, codec);
                        let lang = canonicalize_lang(&lang);
                        if let Some(lang) = lang {
                            self.mi.languages = vec![lang];
                        }
                    }
                    525 => {
                        let pwm = decode_with_codec(&content, codec);
                        if !pwm.is_empty() {
                            self.primary_writing_mode = Some(pwm);
                        }
                    }
                    527 => {
                        let ppd = decode_with_codec(&content, codec);
                        if !ppd.is_empty() {
                            self.page_progression_direction = Some(ppd);
                        }
                    }
                    113 => {
                        // ASIN or other id; handled in process_metadata_record range
                        // check is 100..200 so 113 falls there already. Kept here
                        // only as documentation of the python record id.
                    }
                    _ => {}
                }
            }
        }

        if let Some(t) = title {
            self.mi.title = decode_entities(&crate::beautiful_soup::clean_xml_chars(
                &strip_control_chars(&t),
            ));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_metadata_record(
        &mut self,
        idx: u32,
        content: &[u8],
        codec: &str,
        decode: &dyn Fn(&[u8]) -> String,
        found_authors: &mut bool,
        author_sort_accum: &mut Option<String>,
    ) {
        match idx {
            100 => {
                if !*found_authors {
                    self.mi.authors.clear();
                    *found_authors = true;
                }
                let au = crate::beautiful_soup::clean_xml_chars(decode(content).trim());
                if let Some((last, first)) = split_ln_fn(&au) {
                    // Default calibre tweak (`author_sort_copy_method = 'comma'`):
                    // display name is inverted to "First Last", author_sort keeps
                    // the original "Last, First" form.
                    self.mi.authors.push(format!("{first} {last}"));
                    let sort_val = format!("{last}, {first}");
                    match author_sort_accum {
                        Some(existing) => {
                            existing.push_str(" & ");
                            existing.push_str(&sort_val);
                        }
                        None => *author_sort_accum = Some(sort_val),
                    }
                    self.mi.author_sort = author_sort_accum.clone();
                } else {
                    self.mi.authors.push(au);
                }
            }
            101 => {
                let publisher = crate::beautiful_soup::clean_xml_chars(decode(content).trim());
                if publisher != "Unknown" {
                    self.mi.publisher = Some(publisher);
                } else {
                    self.mi.publisher = None;
                }
            }
            103 => {
                self.mi.comments = Some(crate::beautiful_soup::clean_xml_chars(
                    decode(content).trim(),
                ));
            }
            104 => {
                let raw = decode(content).trim().replace('-', "");
                if let Some(isbn) = check_isbn(&raw) {
                    self.mi.identifiers.insert("isbn".to_string(), isbn);
                }
            }
            105 => {
                let cleaned = crate::beautiful_soup::clean_xml_chars(&decode(content));
                for tag in cleaned.split(';') {
                    let tag = tag.trim().to_string();
                    if !tag.is_empty() && !self.mi.tags.contains(&tag) {
                        self.mi.tags.push(tag);
                    }
                }
            }
            106 => {
                if let Some(dt) = calibre_utils::date::parse_date(&decode(content), false) {
                    self.mi.pubdate = Some(dt);
                }
            }
            108 => {
                self.mi.user_metadata.insert(
                    "book_producer".to_string(),
                    crate::beautiful_soup::clean_xml_chars(decode(content).trim()),
                );
            }
            109 => {
                self.mi.user_metadata.insert(
                    "rights".to_string(),
                    crate::beautiful_soup::clean_xml_chars(decode(content).trim()),
                );
            }
            112 => {
                let s = decode_with_codec(content, codec);
                let s = s.trim();
                let isig = "urn:isbn:";
                if s.to_lowercase().starts_with(isig) {
                    if let Some(isbn) = check_isbn(&s[isig.len()..]) {
                        self.mi
                            .identifiers
                            .entry("isbn".to_string())
                            .or_insert(isbn);
                    }
                } else if let Some(cid) = s.strip_prefix("calibre:") {
                    if !cid.is_empty() {
                        self.mi.uuid = Some(cid.to_string());
                    }
                }
            }
            113 => {
                if let Ok(s) = std::str::from_utf8(content) {
                    self.uuid = Some(s.to_string());
                    self.mi.set_identifier("mobi-asin", s);
                } else {
                    self.uuid = None;
                }
            }
            116 => {
                if content.len() >= 4 {
                    self.start_offset = Some(u32::from_be_bytes([
                        content[0], content[1], content[2], content[3],
                    ]));
                }
            }
            121 if content.len() >= 4 => {
                let v = u32::from_be_bytes([content[0], content[1], content[2], content[3]]);
                self.kf8_header = if v == NULL_INDEX { None } else { Some(v) };
            }
            _ => {}
        }
    }
}

/// Split "Last, First" author names, mirroring
/// `re.match(r'([^,]+?)\s*,\s+([^,]+)$', au.strip())` in `process_metadata`.
fn split_ln_fn(au: &str) -> Option<(String, String)> {
    let au = au.trim();
    let comma = au.find(',')?;
    let last = au[..comma].trim();
    let first = au[comma + 1..].trim();
    if last.is_empty() || first.is_empty() || first.contains(',') {
        return None;
    }
    Some((last.to_string(), first.to_string()))
}

/// Narrow stand-in for `calibre.utils.localization.canonicalize_lang`
/// (which maps arbitrary language name/code spellings against a
/// multi-thousand-entry ISO 639 table). We only need "best effort"
/// normalization here: lowercase + trim. Unlike upstream this will not
/// map e.g. "English" -> "eng"; full fidelity is out of scope for the
/// reader pipeline, which only stores this in `mi.languages`.
fn canonicalize_lang(raw: &str) -> Option<String> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[derive(Debug, Clone)]
pub struct BookHeader {
    pub palmdoc: PalmDocHeader,
    pub mobi: MobiHeader,
    pub exth: Option<ExthHeader>,
    pub title: String,
    pub codec: String,
    pub compression_type: u16,
    pub encryption_type: u16,
    pub first_image_index: u32,
    pub mobi_version: u32,
    pub huff_offset: Option<u32>,
    pub huff_number: Option<u32>,

    /// `self.records` in `headers.py` -- the PalmDOC text record count
    /// (`raw[8:12]`, i.e. `palmdoc.record_count`).
    pub records: u16,
    /// `self.extra_flags` -- multibyte/trailing-entry bitfield used by
    /// `sizeof_trailing_entries`.
    pub extra_flags: u16,
    /// `self.ancient` -- true for pre-MOBI PalmDOC files with a
    /// PDB record 0 too short to hold a MOBI header at all.
    pub ancient: bool,
    /// KF8 NCX index (`self.ncxidx`).
    pub ncxidx: u32,
    /// KF8 skeleton index (`self.skelidx`).
    pub skelidx: u32,
    /// KF8 fragment/div index (`self.dividx`).
    pub dividx: u32,
    /// KF8 "datp" index (`self.datpidx`), unused by the reader pipeline
    /// itself but kept for parity with `headers.py`.
    pub datpidx: u32,
    /// KF8 guide/"other" index (`self.othidx`).
    pub othidx: u32,
    /// KF8 flow boundary (FDST) record index (`self.fdstidx`).
    pub fdstidx: u32,
    /// Number of FDST entries (`self.fdstcnt`).
    pub fdstcnt: u32,

    /// Set by `MobiReader.__init__` when a joint MOBI6+KF8 file's KF8
    /// header is parsed: `first_image_index` of the *outer* MOBI6 header
    /// plus the KF8 boundary record index.
    pub kf8_first_image_index: Option<u32>,
    /// Set on a joint file's KF8 `BookHeader`: the MOBI6 header's own
    /// `records` count (`bh.mobi6_records` in Python).
    pub mobi6_records: Option<u16>,
}

impl BookHeader {
    /// Port of `headers.py`'s `BookHeader.__init__`.
    ///
    /// `ident` is the 8-byte PDB creator identifier (`BOOKMOBI`/`TEXTREAD`);
    /// `try_extra_data_fix` mirrors the same-named Python kwarg (a fallback
    /// used when the normal `extra_flags` heuristic misparses certain
    /// generator quirks -- see `MobiReader.__init__`'s caller in Python).
    pub fn parse(
        raw: &[u8],
        ident: &[u8],
        user_encoding: Option<&str>,
        try_extra_data_fix: bool,
    ) -> Result<Self> {
        if raw.len() <= 16 {
            // "Ancient" PalmDOC-only file: no MOBI header at all.
            let palmdoc = PalmDocHeader::parse(&mut std::io::Cursor::new(raw))?;
            let mobi = MobiHeader {
                identifier: String::new(),
                header_length: 0,
                mobi_type: 0,
                text_encoding: 0,
                unique_id: 0,
                file_version: 1,
                ortographic_index: NULL_INDEX,
                inflection_index: NULL_INDEX,
                index_names: NULL_INDEX,
                index_keys: NULL_INDEX,
                extra_index_0: NULL_INDEX,
                extra_index_1: NULL_INDEX,
                extra_index_2: NULL_INDEX,
                extra_index_3: NULL_INDEX,
                extra_index_4: NULL_INDEX,
                extra_index_5: NULL_INDEX,
                first_non_book_index: NULL_INDEX,
                full_name_offset: 0,
                full_name_length: 0,
                locale: 0,
                input_language: 0,
                output_language: 0,
                min_version: 0,
                first_image_index: NULL_INDEX,
                huffman_record_offset: 0,
                huffman_record_count: 0,
                huffman_table_offset: 0,
                huffman_table_length: 0,
                exth_flags: 0,
                drm_offset: 0,
                drm_count: 0,
                drm_size: 0,
                drm_flags: 0,
            };
            let compression_type = palmdoc.compression;
            let encryption_type = palmdoc.encryption_type;
            let records = palmdoc.record_count;
            return Ok(BookHeader {
                palmdoc,
                mobi,
                exth: None,
                title: "Unknown".to_string(),
                codec: "cp1252".to_string(),
                compression_type,
                encryption_type,
                first_image_index: NULL_INDEX,
                mobi_version: 1,
                huff_offset: None,
                huff_number: None,
                records,
                extra_flags: 0,
                ancient: true,
                ncxidx: NULL_INDEX,
                skelidx: NULL_INDEX,
                dividx: NULL_INDEX,
                datpidx: NULL_INDEX,
                othidx: NULL_INDEX,
                fdstidx: NULL_INDEX,
                fdstcnt: 0,
                kf8_first_image_index: None,
                mobi6_records: None,
            });
        }

        let mut reader = std::io::Cursor::new(raw);
        let palmdoc = PalmDocHeader::parse(&mut reader)?;

        let identifier_offset = 16;
        reader.seek(SeekFrom::Start(identifier_offset))?;
        let mobi = MobiHeader::parse(&mut reader)?;

        let codec = match mobi.text_encoding {
            1252 => "cp1252".to_string(),
            65001 => "utf-8".to_string(),
            _ => user_encoding.unwrap_or("cp1252").to_string(),
        };

        // `raw[0x54:0x5c]` -- title offset/length.
        let (title_off, title_len) = {
            let mut c = std::io::Cursor::new(raw);
            c.seek(SeekFrom::Start(0x54))?;
            let toff = c.read_u32::<BigEndian>()?;
            let tlen = c.read_u32::<BigEndian>()?;
            (toff as usize, tlen as usize)
        };
        let title = if title_off + title_len <= raw.len() && title_off <= raw.len() {
            decode_with_codec(&raw[title_off..title_off + title_len], &codec)
        } else {
            "Unknown".to_string()
        };

        // `raw[0x5C:0x60]` -- packed language code.
        let langcode = {
            let mut c = std::io::Cursor::new(raw);
            c.seek(SeekFrom::Start(0x5C))?;
            c.read_u32::<BigEndian>()?
        };
        let langid = langcode & 0xFF;
        let sublangid = (langcode >> 10) & 0xFF;

        // `raw[0x68:0x6c]` -- despite the field's name in the wire format
        // ("minimum reader version needed"), this is the value Python's
        // `self.mobi_version` reads and treats as the MOBI6-vs-KF8
        // discriminant (`mobi_version == 8`). It is *not* the same as
        // `mobi.file_version` (`raw[0x24:0x28]`, called `self.version` in
        // Python, largely unused downstream).
        let mobi_version = mobi.min_version;
        let first_image_index = mobi.first_image_index;

        let mut exth = None;
        if (mobi.exth_flags & 0x40) != 0 {
            let exth_start = 16 + mobi.header_length as u64;
            if exth_start < raw.len() as u64 {
                reader.seek(SeekFrom::Start(exth_start))?;
                if let Ok(mut e) = ExthHeader::parse(&mut reader) {
                    e.process(&codec);
                    if e.mi.languages.is_empty() || e.mi.languages == vec!["und".to_string()] {
                        e.mi.languages = vec![mobi2iana(langid, sublangid)];
                    }
                    exth = Some(e);
                }
            }
        }

        // `raw[0xF2:0xF4]` -- extra_flags, gated on ident/header length.
        let max_header_length: u32 = 500;
        let extra_flags = if ident == b"TEXTREAD"
            || mobi.header_length < 0xE4
            || mobi.header_length > max_header_length
            || (try_extra_data_fix && mobi.header_length == 0xE4)
        {
            0
        } else if raw.len() >= 0xF4 {
            let mut c = std::io::Cursor::new(raw);
            c.seek(SeekFrom::Start(0xF2))?;
            c.read_u16::<BigEndian>()?
        } else {
            0
        };

        let mut huff_offset = None;
        let mut huff_number = None;
        if palmdoc.compression == 17480 {
            // 'DH'
            huff_offset = Some(mobi.huffman_record_offset);
            huff_number = Some(mobi.huffman_record_count);
        }

        // `raw[0xF4:0xF8]` -- NCX index, gated by `len(raw) >= 0xF8`.
        let ncxidx = if raw.len() >= 0xF8 {
            let mut c = std::io::Cursor::new(raw);
            c.seek(SeekFrom::Start(0xF4))?;
            c.read_u32::<BigEndian>()?
        } else {
            NULL_INDEX
        };

        // KF8 index fields: `raw[0xF8:0x108]` (dividx/skelidx/datpidx/othidx)
        // and `raw[0xC0:0xC8]` (fdstidx/fdstcnt), gated on
        // `mobi_version == 8 and len(raw) >= 0xF8 + 16`.
        let (dividx, skelidx, datpidx, othidx, fdstidx, fdstcnt) =
            if mobi_version == 8 && raw.len() >= 0xF8 + 16 {
                let mut c = std::io::Cursor::new(raw);
                c.seek(SeekFrom::Start(0xF8))?;
                let dividx = c.read_u32::<BigEndian>()?;
                let skelidx = c.read_u32::<BigEndian>()?;
                let datpidx = c.read_u32::<BigEndian>()?;
                let othidx = c.read_u32::<BigEndian>()?;
                c.seek(SeekFrom::Start(0xC0))?;
                let fdstidx_raw = c.read_u32::<BigEndian>()?;
                let fdstcnt = c.read_u32::<BigEndian>()?;
                let fdstidx = if fdstcnt <= 1 {
                    NULL_INDEX
                } else {
                    fdstidx_raw
                };
                (dividx, skelidx, datpidx, othidx, fdstidx, fdstcnt)
            } else {
                (
                    NULL_INDEX, NULL_INDEX, NULL_INDEX, NULL_INDEX, NULL_INDEX, 0,
                )
            };

        let compression_type = palmdoc.compression;
        let encryption_type = palmdoc.encryption_type;
        let records = palmdoc.record_count;

        Ok(BookHeader {
            palmdoc,
            mobi,
            exth,
            title,
            codec,
            compression_type,
            encryption_type,
            first_image_index,
            mobi_version,
            huff_offset,
            huff_number,
            records,
            extra_flags,
            ancient: false,
            ncxidx,
            skelidx,
            dividx,
            datpidx,
            othidx,
            fdstidx,
            fdstcnt,
            kf8_first_image_index: None,
            mobi6_records: None,
        })
    }
}
