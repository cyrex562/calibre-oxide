use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    eprintln!("get_metadata: parsing PdbHeader");
    let pdb = PdbHeader::parse(&mut stream)?;
    eprintln!(
        "get_metadata: PdbHeader parsed. Num records: {}",
        pdb.num_records
    );

    if pdb.num_records == 0 {
        bail!("No records in PDB");
    }

    // Record 0 contains PalmDOC Header + MOBI Header + EXTH Header
    let rec0_offset = pdb.records[0].offset as u64;
    stream.seek(SeekFrom::Start(rec0_offset))?;

    // Read header buffer (up to 512 bytes to cover EXTH start)
    let mut header_buf = [0u8; 512];
    let bytes_read = stream.read(&mut header_buf)?;

    // Check minimal size (PalmDOC 16 + MOBI 4 + Len 4 = 24)
    if bytes_read < 24 {
        eprintln!("MOBI header too short: read {}", bytes_read);
        bail!("MOBI header too short");
    }

    // Offset 16: Signature
    if &header_buf[16..20] != b"MOBI" {
        eprintln!("Invalid MOBI signature: {:?}", &header_buf[16..20]);
        bail!("Invalid MOBI signature");
    }

    // Offset 20: MOBI Header Length
    let mobi_header_len = (&header_buf[20..24]).read_u32::<BigEndian>()?;

    let mut mi = MetaInformation::default();
    mi.title = pdb.name.clone();

    // EXTH Flag at 128 (0x80)
    // Offset relative to Rec0 start.
    // Check if we read enough
    let has_exth = if bytes_read >= 132 {
        let flags = (&header_buf[128..132]).read_u32::<BigEndian>()?;
        (flags & 0x40) != 0
    } else {
        // Fallback seek
        stream.seek(SeekFrom::Start(rec0_offset + 128))?;
        let flags = stream.read_u32::<BigEndian>()?;
        (flags & 0x40) != 0
    };

    // First Image Index at 108
    let first_image_index = if bytes_read >= 112 {
        (&header_buf[108..112]).read_u32::<BigEndian>()?
    } else {
        stream.seek(SeekFrom::Start(rec0_offset + 108))?;
        stream.read_u32::<BigEndian>()?
    };

    if has_exth {
        // EXTH starts after MOBI header (16 + len)
        let exth_offset = rec0_offset + 16 + mobi_header_len as u64;
        stream.seek(SeekFrom::Start(exth_offset))?;

        let mut exth_sig = [0u8; 4];
        stream.read_exact(&mut exth_sig)?;
        if &exth_sig == b"EXTH" {
            let _len = stream.read_u32::<BigEndian>()?;
            let count = stream.read_u32::<BigEndian>()?;

            for _ in 0..count {
                let id = stream.read_u32::<BigEndian>()?;
                let size = stream.read_u32::<BigEndian>()?;
                if size < 8 {
                    break;
                }
                let data_len = size - 8;
                let mut data = vec![0u8; data_len as usize];
                stream.read_exact(&mut data)?;

                match id {
                    100 => {
                        if let Ok(s) = String::from_utf8(data) {
                            mi.authors = vec![s];
                        }
                    }
                    503 => {
                        if let Ok(s) = String::from_utf8(data) {
                            mi.title = s;
                        }
                    }
                    201 => {
                        // Cover Offset
                        if data.len() >= 4 {
                            let off = (&data[0..4]).read_u32::<BigEndian>()?;
                            let cover_idx = first_image_index + off;
                            if (cover_idx as usize) < pdb.records.len() {
                                let cover_off = pdb.records[cover_idx as usize].offset;
                                let end_off = if (cover_idx as usize) < pdb.records.len() - 1 {
                                    pdb.records[cover_idx as usize + 1].offset
                                } else {
                                    stream.seek(SeekFrom::End(0))? as u32
                                };
                                let len = end_off - cover_off;
                                if len > 0 {
                                    let pos = stream.stream_position()?;
                                    stream.seek(SeekFrom::Start(cover_off as u64))?;
                                    let mut img = vec![0u8; len as usize];
                                    stream.read_exact(&mut img)?;
                                    mi.cover_data = (Some("jpg".to_string()), img);
                                    stream.seek(SeekFrom::Start(pos))?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
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

    #[test]
    fn test_mobi_metadata() -> Result<()> {
        // println!("START TEST: test_mobi_metadata");
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
        rec0.write_u32::<BigEndian>(201)?;
        rec0.write_u32::<BigEndian>(8 + 4)?;
        rec0.write_u32::<BigEndian>(0)?; // Offset 0 from First Image Index (1) -> Record 1

        // 2. Cover Record
        let cover = b"fake cover image".to_vec();

        // Build PDB
        eprintln!("Creating PDB...");
        let buffer = create_test_pdb(vec![rec0, cover]);
        eprintln!("PDB Created. Len: {}", buffer.len());

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "MOBI Title");
        assert!(mi.cover_data.1.starts_with(b"fake cover image"));

        Ok(())
    }
}
