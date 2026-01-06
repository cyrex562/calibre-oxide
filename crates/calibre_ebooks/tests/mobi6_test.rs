use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::mobi6::MobiReader;
use std::io::Cursor;

#[test]
fn test_mobi_reader_init() {
    // Construct a minimal MOBI PDB file
    let mut data = Vec::new();

    // Name (32 bytes)
    data.extend_from_slice(b"Test Book                       ");

    // Attributes (2 bytes)
    data.write_u16::<BigEndian>(0).unwrap();

    // Version (2 bytes)
    data.write_u16::<BigEndian>(0).unwrap();

    // Dates & Indices (24 bytes)
    for _ in 0..6 {
        data.write_u32::<BigEndian>(0).unwrap();
    }

    // Type (4 bytes) -> BOOK
    data.extend_from_slice(b"BOOK");

    // Creator (4 bytes) -> MOBI
    data.extend_from_slice(b"MOBI");

    // Unique ID seed (4 bytes)
    data.write_u32::<BigEndian>(0).unwrap();

    // Next Record List ID (4 bytes) -> 0
    data.write_u32::<BigEndian>(0).unwrap(); // Actually PDB header format detail

    // Note: The offsets here are approximate based on standard PDB header (78 bytes then records)
    // Actually standard PDB header is 78 bytes including num_records at end?
    // Byte 76 is num_records (u16).
    // Let's verify standard PDB header layout:
    // 0-32: Name
    // 32-34: Attributes
    // 34-36: Version
    // 36-40: Creation Date
    // 40-44: Mod Date
    // 44-48: Backup Date
    // 48-52: Mod Num
    // 52-56: App Info
    // 56-60: Sort Info
    // 60-64: Type (BOOK)
    // 64-68: Creator (MOBI)
    // 68-72: UID Seed
    // 72-76: Next Record List
    // 76-78: Num Records

    // Ensure we are at 76
    assert_eq!(data.len(), 76);

    let num_sections = 1;
    data.write_u16::<BigEndian>(num_sections).unwrap(); // 76-78

    // Record attributes (8 bytes per record)
    // Offset, Attributes (1 byte), Unique ID (3 bytes)
    // Let's place Record 0 at offset header_len + record_list_len
    // header_len = 78
    // record_list_len = 8 * 1 = 8
    // Start offset = 86

    let offset_record0 = 86;
    data.write_u32::<BigEndian>(offset_record0).unwrap();
    data.write_u8(0).unwrap(); // attr
    data.write_u8(0).unwrap(); // val 1
    data.write_u8(0).unwrap(); // val 2
    data.write_u8(0).unwrap(); // val 3

    // Now at 86. Write Record 0 Data (BookHeader)

    // PalmDoc Header (16 bytes)
    data.write_u16::<BigEndian>(1).unwrap(); // Compression (1 = No compression)
    data.write_u16::<BigEndian>(0).unwrap(); // Unused
    data.write_u32::<BigEndian>(100).unwrap(); // Text Length
    data.write_u16::<BigEndian>(1).unwrap(); // Record Count
    data.write_u16::<BigEndian>(4096).unwrap(); // Record Size
    data.write_u16::<BigEndian>(0).unwrap(); // Encryption Type (0 = None)
    data.write_u16::<BigEndian>(0).unwrap(); // Unknown

    // Identifier (4 bytes)
    data.extend_from_slice(b"MOBI");

    // MOBI Header
    // Length (4), Type (4), Encoding (4), UID (4), Version (4)
    data.write_u32::<BigEndian>(232).unwrap(); // header length
    data.write_u32::<BigEndian>(2).unwrap(); // mobi type
    data.write_u32::<BigEndian>(65001).unwrap(); // text encoding (utf-8)
    data.write_u32::<BigEndian>(123).unwrap(); // uid
    data.write_u32::<BigEndian>(6).unwrap(); // file version

    // Fill rest of MOBI header with 0s to meet length requirement
    // We wrote 20 bytes of MOBI header so far. Need 232 total.
    // 232 - 20 = 212 bytes.
    for _ in 0..212 {
        data.write_u8(0).unwrap();
    }

    let cursor = Cursor::new(data);
    let reader = MobiReader::new(cursor);
    assert!(
        reader.is_ok(),
        "MobiReader initialization failed: {:?}",
        reader.err()
    );

    let mobi = reader.unwrap();
    assert_eq!(mobi.name, "Test Book                       ");
    assert_eq!(mobi.num_sections, 1);
    assert_eq!(mobi.book_header.codec, "utf-8");
    assert_eq!(mobi.book_header.mobi_version, 6);
}
