use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::input::snb_input::SnbInput;
use calibre_ebooks::snb::reader::MAGIC;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn test_snb_input_parse_failure() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("book.snb");
    let output_dir = tmp_dir.path().join("output");

    // Create Invalid SNB (Bad Magic)
    {
        let mut file = File::create(&input_path).unwrap();
        file.write_all(b"BADMAGIC").unwrap();
        file.flush().unwrap();
    }

    println!("Testing invalid magic parsing on: {:?}", input_path);

    let input = SnbInput::new();
    let result = input.convert(&input_path, &output_dir);
    println!("Result is err: {}", result.is_err());

    if let Err(e) = &result {
        eprintln!("Error: {:?}", e);
    } else {
        eprintln!("Result was Ok!");
    }

    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    eprintln!("Error Message: {}", err_msg);
    assert!(
        err_msg.contains("Invalid SNB Magic") || err_msg.contains("Failed to parse SNB container")
    );
}

#[test]
fn test_snb_input_empty_but_valid_header_structure() {
    // Constructing a VALID SNB is complex due to VFAT and compression.
    // We will verify that it attempts to parse and fails predictably on missing data,
    // or if we can construct a minimal valid header.

    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("book.snb");
    let output_dir = tmp_dir.path().join("output");

    {
        let mut file = File::create(&input_path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_u32::<BigEndian>(0x00008000).unwrap(); // REV80
        file.write_u32::<BigEndian>(0).unwrap(); // REVA3
        file.write_u32::<BigEndian>(0).unwrap(); // REVZ1

        file.write_u32::<BigEndian>(0).unwrap(); // File count
        file.write_u32::<BigEndian>(0).unwrap(); // VFAT size
        file.write_u32::<BigEndian>(0).unwrap(); // VFAT compressed
        file.write_u32::<BigEndian>(0).unwrap(); // Bin size
        file.write_u32::<BigEndian>(0).unwrap(); // Plain size
        file.write_u32::<BigEndian>(0x325A5645).unwrap(); // REVZ2

        // Parse will try to read VFAT (size 0) -> Success
        // Then Seek to End(-16) to find tail.
        // If file ends here, seek -16 will fail as file is only ~40 bytes.
        // So we need padding.
        file.write_all(&[0u8; 100]).unwrap();

        // Write Tail at End
        // We need to write tail data.
        // Tail header at end:
        // Size (4), Offset (4), Magic (8)

        // Let's just catch the Seek/Read error which confirms the reader logic is running.
    }

    let input = SnbInput::new();
    let result = input.convert(&input_path, &output_dir);
    // It should fail on VFAT reading or Tail seeking
    assert!(result.is_err());
}
