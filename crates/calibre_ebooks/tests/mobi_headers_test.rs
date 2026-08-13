mod common;

use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::headers::{BookHeader, MobiHeader, PalmDocHeader, NULL_INDEX};
use calibre_ebooks::mobi::mobi6::MobiReader;
use common::{build_mobi6_pdb, Mobi6Options};
use std::io::Cursor;

#[test]
fn test_parse_palmdoc_header() {
    let mut data = Vec::new();
    // compression (2), unused (2), text_length (4), record_count (2), record_size (2), encryption_type (2), unknown (2)
    data.write_u16::<BigEndian>(2).unwrap(); // PalmDOC compression
    data.write_u16::<BigEndian>(0).unwrap();
    data.write_u32::<BigEndian>(1000).unwrap();
    data.write_u16::<BigEndian>(10).unwrap();
    data.write_u16::<BigEndian>(4096).unwrap();
    data.write_u16::<BigEndian>(0).unwrap();
    data.write_u16::<BigEndian>(0).unwrap();

    let mut cursor = Cursor::new(data);
    let header = PalmDocHeader::parse(&mut cursor).expect("Failed to parse PalmDoc header");

    assert_eq!(header.compression, 2);
    assert_eq!(header.text_length, 1000);
    assert_eq!(header.record_count, 10);
}

#[test]
fn test_parse_mobi_header() {
    let mut data = Vec::new();
    // identifier (4)
    data.extend_from_slice(b"MOBI");
    // header_length (4)
    data.write_u32::<BigEndian>(232).unwrap();
    // mobi_type (4)
    data.write_u32::<BigEndian>(2).unwrap();
    // text_encoding (4)
    data.write_u32::<BigEndian>(65001).unwrap();
    // unique_id (4)
    data.write_u32::<BigEndian>(12345).unwrap();
    // file_version (4)
    data.write_u32::<BigEndian>(6).unwrap();

    // Fill remaining required fields to detect it valid (4 * 23 bytes detected before seek padding)
    for _ in 0..23 {
        data.write_u32::<BigEndian>(0).unwrap();
    }

    // Pad for seeking (32 bytes reserved)
    for _ in 0..32 {
        data.write_u8(0).unwrap();
    }

    // DRM info
    data.write_u32::<BigEndian>(0).unwrap(); // offset
    data.write_u32::<BigEndian>(0).unwrap(); // count
    data.write_u32::<BigEndian>(0).unwrap(); // size
    data.write_u32::<BigEndian>(0).unwrap(); // flags

    let mut cursor = Cursor::new(data);
    let header = MobiHeader::parse(&mut cursor).expect("Failed to parse MOBI header");

    assert_eq!(header.identifier, "MOBI");
    assert_eq!(header.header_length, 232);
    assert_eq!(header.mobi_type, 2);
    assert_eq!(header.text_encoding, 65001);
}

/// Parses `pdb` via the real `MobiReader::new` (which correctly slices
/// record 0 to its true PDB-record boundary) and returns its
/// `BookHeader`, rather than hand-slicing -- `BookHeader::parse`'s
/// length-gated fields (extra_flags, ncxidx, div/skel/fdst indices) are
/// only meaningful when `raw` is exactly record 0's bytes, not the whole
/// remaining PDB.
fn book_header0(pdb: &[u8]) -> BookHeader {
    MobiReader::new(pdb).unwrap().book_header
}

#[test]
fn test_book_header_basic_fields() {
    let opts = Mobi6Options::new();
    let pdb = build_mobi6_pdb("Header Test", b"hello", &opts);
    let bh = book_header0(&pdb);

    assert_eq!(bh.title, "Header Test");
    assert_eq!(bh.codec, "utf-8");
    assert_eq!(bh.mobi_version, 6);
    assert!(!bh.ancient);
    assert_eq!(bh.records, 1);
    assert_eq!(bh.compression_type, 1);
}

#[test]
fn test_book_header_kf8_version_detected() {
    let mut opts = Mobi6Options::new();
    opts.mobi_version = 8;
    let pdb = build_mobi6_pdb("KF8 Header", b"hello", &opts);
    let bh = book_header0(&pdb);
    assert_eq!(bh.mobi_version, 8);
    // With no div/skel/fdst table bytes present, these all resolve to
    // NULL_INDEX rather than garbage -- `read.len() >= 0xF8 + 16` is the
    // gate and our fixture's record 0 is shorter than that once EXTH is
    // absent (header ends right after the 232-byte MOBI header + title).
    assert_eq!(bh.dividx, NULL_INDEX);
    assert_eq!(bh.skelidx, NULL_INDEX);
}

#[test]
fn test_book_header_exth_metadata() {
    let mut opts = Mobi6Options::new();
    opts.exth = vec![
        (100, b"Smith, John".to_vec()),
        (104, b"978-0-13-468599-1".to_vec()),
        (105, b"fiction;adventure".to_vec()),
    ];
    let pdb = build_mobi6_pdb("EXTH Header", b"hello", &opts);
    let bh = book_header0(&pdb);

    let exth = bh.exth.expect("EXTH should be present");
    assert_eq!(exth.mi.authors, vec!["John Smith".to_string()]);
    assert_eq!(
        exth.mi.identifiers.get("isbn").map(|s| s.as_str()),
        Some("9780134685991")
    );
    assert!(exth.mi.tags.contains(&"fiction".to_string()));
    assert!(exth.mi.tags.contains(&"adventure".to_string()));
}

#[test]
fn test_book_header_encryption_type() {
    let mut opts = Mobi6Options::new();
    opts.encryption_type = 2;
    let pdb = build_mobi6_pdb("Encrypted", b"hello", &opts);
    let bh = book_header0(&pdb);
    assert_eq!(bh.encryption_type, 2);
}
