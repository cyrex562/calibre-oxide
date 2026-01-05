use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::headers::{MobiHeader, PalmDocHeader};
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
