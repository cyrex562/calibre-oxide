use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::input::mobi_input::MOBIInput;
use std::fs;
use std::io::{Seek, Write};
use tempfile::tempdir;

#[test]
fn test_mobi_content_extraction() {
    let tmp_dir = tempdir().unwrap();
    let mobi_path = tmp_dir.path().join("book.mobi");
    let output_dir = tmp_dir.path().join("output_book");

    // Create Mock MOBI
    {
        let mut f = fs::File::create(&mobi_path).unwrap();

        // 1. PDB Header
        let name = b"MockBook\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"; // 32 bytes
        f.write_all(name).unwrap();
        f.write_u16::<BigEndian>(0).unwrap(); // attributes
        f.write_u16::<BigEndian>(0).unwrap(); // version
        f.write_u32::<BigEndian>(0).unwrap(); // create_time
        f.write_u32::<BigEndian>(0).unwrap(); // mod_time
        f.write_u32::<BigEndian>(0).unwrap(); // backup_time
        f.write_u32::<BigEndian>(0).unwrap(); // mod_num
        f.write_u32::<BigEndian>(0).unwrap(); // app_info
        f.write_u32::<BigEndian>(0).unwrap(); // sort_info
        f.write_all(b"BOOK").unwrap(); // type
        f.write_all(b"MOBI").unwrap(); // creator
        f.write_u32::<BigEndian>(0).unwrap(); // seed
        f.write_u32::<BigEndian>(0).unwrap(); // next_record
        f.write_u16::<BigEndian>(2).unwrap(); // num_records (Header + 1 Text)

        // Record List
        // Record 0 (Header): Offset 78 + 16 = 94.
        // 78 is PDB header size. Record list is 8 bytes per record. 2 records = 16 bytes.
        let rec0_offset = 78 + 16;
        f.write_u32::<BigEndian>(rec0_offset).unwrap();
        f.write_all(&[0, 0, 0, 0]).unwrap(); // attribs + id

        // Record 1 (Text): Offset = Rec0 + PalmDoc(16) + Mobi(232) + padding?
        // Let's make headers small for test or standard size.
        // Rec0 size: PalmDoc(16) + Mobi(232) = 248.
        let rec1_offset = rec0_offset + 248;
        f.write_u32::<BigEndian>(rec1_offset).unwrap();
        f.write_all(&[0, 0, 0, 0]).unwrap();

        // Record 0 Data (PalmDoc + Mobi)
        // PalmDoc
        f.write_u16::<BigEndian>(2).unwrap(); // Compression = 2 (PalmDoc)
        f.write_u16::<BigEndian>(0).unwrap(); // Unused
        f.write_u32::<BigEndian>(6).unwrap(); // Text Length (abcabc)
        f.write_u16::<BigEndian>(1).unwrap(); // Record Count (1 text record)
        f.write_u16::<BigEndian>(4096).unwrap(); // Record Size
        f.write_u16::<BigEndian>(0).unwrap(); // Encryption
        f.write_u16::<BigEndian>(0).unwrap(); // Unknown

        // Mobi Header
        f.write_all(b"MOBI").unwrap();
        f.write_u32::<BigEndian>(232).unwrap(); // Header Length
        f.write_u32::<BigEndian>(2).unwrap(); // Type
        f.write_u32::<BigEndian>(65001).unwrap(); // UTF-8
        f.write_u32::<BigEndian>(1).unwrap(); // ID
        f.write_u32::<BigEndian>(6).unwrap(); // Version
                                              // Fill rest with 0
        for _ in 0..(232 - 24) {
            f.write_u8(0).unwrap();
        }

        // Record 1 Data (Compressed Text)
        // "abcabc" -> "abc" + pair(3,3) -> b"abc\x80\x18"
        // Current pos should be rec1_offset?
        let pos = f.stream_position().unwrap();
        assert_eq!(pos, rec1_offset as u64);

        f.write_all(b"abc\x80\x18").unwrap();
    }

    // Run MOBIInput
    let plugin = MOBIInput::new();
    let book = plugin
        .convert(&mobi_path, &output_dir)
        .expect("Conversion failed");

    // Verify output file
    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());
    let content = fs::read_to_string(content_path).unwrap();
    assert_eq!(content, "abcabc");
}
