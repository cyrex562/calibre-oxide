//! Shared fixture builders for MOBI reader tests. Builds synthetic
//! PDB/MOBI byte streams by hand (no real `.mobi` file fixtures exist in
//! this repo -- see `mobi6_test.rs`/`mobi_headers_test.rs` for the
//! established pattern this follows).

use byteorder::{BigEndian, WriteBytesExt};

pub const NULL_INDEX: u32 = 0xFFFF_FFFF;

/// One EXTH metadata record: `(type_id, raw_content_bytes)`.
pub type ExthRecord = (u32, Vec<u8>);

#[derive(Default)]
pub struct Mobi6Options {
    pub mobi_version: u32,
    pub compression: u16,
    pub encryption_type: u16,
    pub text_encoding: u32,
    pub exth: Vec<ExthRecord>,
    pub extra_flags: u16,
    /// Extra raw records appended after the text record (e.g. images).
    pub extra_records: Vec<Vec<u8>>,
    pub first_image_index: Option<u32>,
}

impl Mobi6Options {
    pub fn new() -> Self {
        Mobi6Options {
            mobi_version: 6,
            compression: 1,
            encryption_type: 0,
            text_encoding: 65001,
            exth: Vec::new(),
            extra_flags: 0,
            extra_records: Vec::new(),
            first_image_index: None,
        }
    }
}

fn build_exth(records: &[ExthRecord]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, content) in records {
        body.write_u32::<BigEndian>(*id).unwrap();
        body.write_u32::<BigEndian>((8 + content.len()) as u32)
            .unwrap();
        body.extend_from_slice(content);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"EXTH");
    out.write_u32::<BigEndian>((12 + body.len()) as u32)
        .unwrap();
    out.write_u32::<BigEndian>(records.len() as u32).unwrap();
    out.extend_from_slice(&body);
    out
}

/// Builds a complete PDB byte stream containing a single-flow MOBI6 book:
/// record 0 is the PalmDOC+MOBI(+EXTH) header, record 1 is `text` (already
/// compressed/encoded to match `opts.compression`), followed by any
/// `opts.extra_records`.
pub fn build_mobi6_pdb(title: &str, text: &[u8], opts: &Mobi6Options) -> Vec<u8> {
    // --- Record 0: PalmDOC header + MOBI header (+ EXTH) + full title ---
    let mut rec0 = Vec::new();
    rec0.write_u16::<BigEndian>(opts.compression).unwrap();
    rec0.write_u16::<BigEndian>(0).unwrap(); // unused
    rec0.write_u32::<BigEndian>(text.len() as u32).unwrap(); // text_length (approx, unused by reader)
    rec0.write_u16::<BigEndian>(1).unwrap(); // record_count (1 text record)
    rec0.write_u16::<BigEndian>(4096).unwrap(); // record_size
    rec0.write_u16::<BigEndian>(opts.encryption_type).unwrap();
    rec0.write_u16::<BigEndian>(0).unwrap(); // unknown

    assert_eq!(rec0.len(), 16);
    rec0.extend_from_slice(b"MOBI");
    let header_length: u32 = 232;
    rec0.write_u32::<BigEndian>(header_length).unwrap();
    rec0.write_u32::<BigEndian>(2).unwrap(); // mobi_type
    rec0.write_u32::<BigEndian>(opts.text_encoding).unwrap();
    rec0.write_u32::<BigEndian>(0).unwrap(); // unique_id
    rec0.write_u32::<BigEndian>(6).unwrap(); // file_version (raw[0x24], mostly unused)

    // ortographic_index .. extra_index_5 (10 fields)
    for _ in 0..10 {
        rec0.write_u32::<BigEndian>(NULL_INDEX).unwrap();
    }
    assert_eq!(rec0.len(), 80);

    // first_non_book_index
    rec0.write_u32::<BigEndian>(NULL_INDEX).unwrap();
    // full_name_offset / full_name_length: filled in once we know where the
    // title bytes land (after the header + EXTH block); placeholder for now.
    let full_name_offset_pos = rec0.len();
    rec0.write_u32::<BigEndian>(0).unwrap();
    rec0.write_u32::<BigEndian>(0).unwrap();
    rec0.write_u32::<BigEndian>(0).unwrap(); // locale
    rec0.write_u32::<BigEndian>(0).unwrap(); // input_language
    rec0.write_u32::<BigEndian>(0).unwrap(); // output_language
    rec0.write_u32::<BigEndian>(opts.mobi_version).unwrap(); // raw[0x68] min_version == mobi_version
    rec0.write_u32::<BigEndian>(opts.first_image_index.unwrap_or(NULL_INDEX))
        .unwrap(); // raw[0x6c]
    assert_eq!(rec0.len(), 112);

    rec0.write_u32::<BigEndian>(0).unwrap(); // huffman_record_offset
    rec0.write_u32::<BigEndian>(0).unwrap(); // huffman_record_count
    rec0.write_u32::<BigEndian>(0).unwrap(); // huffman_table_offset
    rec0.write_u32::<BigEndian>(0).unwrap(); // huffman_table_length
    assert_eq!(rec0.len(), 128);

    let exth_bytes = if opts.exth.is_empty() {
        Vec::new()
    } else {
        build_exth(&opts.exth)
    };
    let exth_flags: u32 = if opts.exth.is_empty() { 0 } else { 0x40 };
    rec0.write_u32::<BigEndian>(exth_flags).unwrap();
    assert_eq!(rec0.len(), 132);

    // 32 reserved bytes (offset 132..164)
    rec0.extend(std::iter::repeat_n(0u8, 32));
    assert_eq!(rec0.len(), 164);

    // DRM offset/count/size/flags (164..180)
    for _ in 0..4 {
        rec0.write_u32::<BigEndian>(0).unwrap();
    }
    assert_eq!(rec0.len(), 180);

    // extra_flags (raw[0xF2:0xF4] == offset 242)
    while rec0.len() < 0xF2 {
        rec0.push(0);
    }
    rec0.write_u16::<BigEndian>(opts.extra_flags).unwrap();
    assert_eq!(rec0.len(), 0xF4);

    // ncxidx (raw[0xF4:0xF8])
    rec0.write_u32::<BigEndian>(NULL_INDEX).unwrap();
    assert_eq!(rec0.len(), 0xF8);

    // Pad out to header_length (16 + header_length == 248 total from
    // record start, i.e. 232 bytes of MOBI header starting at offset 16).
    while rec0.len() < 16 + header_length as usize {
        rec0.push(0);
    }
    assert_eq!(rec0.len(), 16 + header_length as usize);

    // EXTH immediately follows the MOBI header.
    rec0.extend_from_slice(&exth_bytes);

    // Full title follows EXTH (or the header, if no EXTH).
    let full_name_offset = rec0.len() as u32;
    let title_bytes = title.as_bytes();
    rec0.extend_from_slice(title_bytes);
    let full_name_length = title_bytes.len() as u32;

    rec0[full_name_offset_pos..full_name_offset_pos + 4]
        .copy_from_slice(&full_name_offset.to_be_bytes());
    rec0[full_name_offset_pos + 4..full_name_offset_pos + 8]
        .copy_from_slice(&full_name_length.to_be_bytes());

    // --- Assemble the PDB ---
    let mut records: Vec<Vec<u8>> = vec![rec0, text.to_vec()];
    records.extend(opts.extra_records.iter().cloned());
    build_pdb(title, &records)
}

/// Builds a minimal PDB envelope (78-byte header + record offset table)
/// around already-prepared record payloads.
pub fn build_pdb(name: &str, records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut name_bytes = [0u8; 32];
    let n = name.as_bytes();
    let len = n.len().min(31);
    name_bytes[..len].copy_from_slice(&n[..len]);
    out.extend_from_slice(&name_bytes);

    out.write_u16::<BigEndian>(0).unwrap(); // attributes
    out.write_u16::<BigEndian>(0).unwrap(); // version
    for _ in 0..6 {
        out.write_u32::<BigEndian>(0).unwrap(); // 4 dates/appinfo/sortinfo + 2 more = 24 bytes total
    }
    out.extend_from_slice(b"BOOK");
    out.extend_from_slice(b"MOBI");
    out.write_u32::<BigEndian>(0).unwrap(); // unique id seed
    out.write_u32::<BigEndian>(0).unwrap(); // next record list id
    assert_eq!(out.len(), 76);
    out.write_u16::<BigEndian>(records.len() as u16).unwrap();
    assert_eq!(out.len(), 78);

    let header_and_list_len = 78 + records.len() * 8;
    let mut offset = header_and_list_len as u32;
    let mut offsets = Vec::new();
    for r in records {
        offsets.push(offset);
        offset += r.len() as u32;
    }
    for off in &offsets {
        out.write_u32::<BigEndian>(*off).unwrap();
        out.write_u8(0).unwrap();
        out.write_u8(0).unwrap();
        out.write_u8(0).unwrap();
        out.write_u8(0).unwrap();
    }
    assert_eq!(out.len(), header_and_list_len);

    for r in records {
        out.extend_from_slice(r);
    }
    out
}
