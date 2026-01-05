use anyhow::Result;
use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::metadata::mobi::get_metadata;
use std::io::Cursor;

// Helper to build PDB
fn create_test_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
    let mut buffer = Vec::new();
    // Header (78 bytes)
    buffer.extend_from_slice(b"Test Book Title\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
    buffer.write_u16::<BigEndian>(0).unwrap();
    buffer.write_u16::<BigEndian>(0).unwrap();
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.write_u32::<BigEndian>(0).unwrap(); // mod_num
    buffer.write_u32::<BigEndian>(0).unwrap(); // app_info
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.extend_from_slice(b"BOOKMOBI");
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.write_u32::<BigEndian>(0).unwrap();
    buffer.write_u16::<BigEndian>(records.len() as u16).unwrap();

    // Record List
    let base_offset = 78 + (records.len() as u32 * 8) + 2;
    let mut offsets = Vec::new();
    let mut running_off = base_offset;

    for r in &records {
        offsets.push(running_off);
        running_off += r.len() as u32;
    }

    // Write Rec List
    for off in offsets {
        buffer.write_u32::<BigEndian>(off).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
    }

    // Pad to base_offset
    while buffer.len() < base_offset as usize {
        buffer.push(0);
    }

    // Write Records
    for r in records {
        buffer.extend_from_slice(&r);
    }

    buffer
}

fn main() -> Result<()> {
    eprintln!("START DEBUG MOBI");
    // 1. Construct Rec 0 (Header + EXTH)
    let mut rec0 = Vec::new();
    // PalmDOC (16)
    rec0.write_u16::<BigEndian>(1).unwrap();
    rec0.write_u16::<BigEndian>(0)?;
    rec0.write_u32::<BigEndian>(0)?;
    rec0.write_u16::<BigEndian>(0)?;
    rec0.write_u16::<BigEndian>(0)?;
    rec0.write_u16::<BigEndian>(0)?;
    rec0.write_u16::<BigEndian>(0)?;

    // MOBI Header
    rec0.extend_from_slice(b"MOBI");
    let mobi_len = 232u32;
    rec0.write_u32::<BigEndian>(mobi_len)?; // Len matches what we write
    rec0.write_u32::<BigEndian>(2)?;
    rec0.write_u32::<BigEndian>(65001)?;
    rec0.write_u32::<BigEndian>(0)?;
    rec0.write_u32::<BigEndian>(0)?;

    // Pad to 108 (Image Index)
    while rec0.len() < 108 {
        rec0.push(0);
    }
    rec0.write_u32::<BigEndian>(1)?; // First Image Index maps to Record 1 (0+1)

    // Pad to 128 (Flags)
    while rec0.len() < 128 {
        rec0.push(0);
    }
    rec0.write_u32::<BigEndian>(0x40)?; // EXTH

    // Pad to end of MOBI Header (Starts at 16, Len 232 -> Ends at 248)
    while rec0.len() < 248 {
        rec0.push(0);
    }

    // EXTH
    rec0.extend_from_slice(b"EXTH");
    rec0.write_u32::<BigEndian>(0)?; // Len
    rec0.write_u32::<BigEndian>(2)?; // Count

    // Title
    let title = "MOBI Title";
    rec0.write_u32::<BigEndian>(503)?;
    rec0.write_u32::<BigEndian>(8 + title.len() as u32)?;
    rec0.extend_from_slice(title.as_bytes());

    // Cover Offset (201)
    // Data 4 bytes.
    let cover_data_offset_in_image = 0u32; // Offset from start of image record? No, "Offset 0 from First Image Index"

    // Logic in mobi.rs:
    // let off = (&data[0..4]).read_u32::<BigEndian>()?;
    // let cover_idx = first_image_index + off;
    // We set first_image_index = 1.
    // If we set off = 0, cover_idx = 1.
    // Record 1 should be the cover.

    rec0.write_u32::<BigEndian>(201)?;
    rec0.write_u32::<BigEndian>(8 + 4)?;
    rec0.write_u32::<BigEndian>(cover_data_offset_in_image)?; // Offset 0

    // 2. Cover Record
    let cover = b"fake cover image".to_vec();

    // Build PDB
    eprintln!("Creating PDB...");
    let buffer = create_test_pdb(vec![rec0, cover]);
    eprintln!("PDB Created. Len: {}", buffer.len());

    let mut stream = Cursor::new(buffer);
    let mi = get_metadata(&mut stream)?;

    eprintln!("Title: {}", mi.title);
    let (ext, data) = &mi.cover_data;
    if !data.is_empty() {
        eprintln!(
            "Cover: {} bytes ({:?}) - starts with {:?}",
            data.len(),
            ext,
            &data[0..std::cmp::min(10, data.len())]
        );
    } else {
        eprintln!("No cover found");
    }

    if mi.title != "MOBI Title" {
        panic!("Title mismatch: expected 'MOBI Title', got '{}'", mi.title);
    }
    if !data.starts_with(b"fake cover image") {
        panic!("Cover mismatch");
    }

    eprintln!("Test Passed");
    Ok(())
}
