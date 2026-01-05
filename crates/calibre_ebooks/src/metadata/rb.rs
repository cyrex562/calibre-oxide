use crate::metadata::{string_to_authors, MetaInformation};
use crate::rb::header::{RbHeader, RbTocEntry, MAGIC};
use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();
    mi.title = "Unknown".to_string();
    mi.authors = vec!["Unknown".to_string()];

    let header = RbHeader::parse(&mut stream)?;

    // Seek back to TOC start + 4 (count was read)
    stream.seek(SeekFrom::Start((header.toc_offset + 4) as u64))?;

    let mut info_offset = 0;
    let mut info_length = 0;
    let mut found = false;

    for _ in 0..header.toc_count {
        let entry = RbTocEntry::read(&mut stream)?;
        if entry.flag == 2 {
            info_offset = entry.offset;
            info_length = entry.length;
            found = true;
            break;
        }
    }

    if !found {
        bail!("INFO block not found in RB file");
    }

    // Read INFO block
    stream.seek(SeekFrom::Start(info_offset as u64))?;
    let mut buffer = vec![0u8; info_length as usize];
    stream.read_exact(&mut buffer)?;

    // Decode and parse
    let content = String::from_utf8_lossy(&buffer);

    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "TITLE" => mi.title = value.to_string(),
                "AUTHOR" => mi.authors = string_to_authors(value),
                _ => {}
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
    use std::io::Write;

    #[test]
    fn test_rb_metadata() -> Result<()> {
        let toc_start = 32u32;

        // Build Buffer
        let mut buffer = Vec::new();
        buffer.extend_from_slice(MAGIC); // 14 bytes
        buffer.extend_from_slice(&[0u8; 10]); // 10 bytes -> pos 24

        buffer.write_u32::<LittleEndian>(toc_start)?; // 4 bytes -> pos 28
        buffer.write_u32::<LittleEndian>(0)?; // Padding/Junk -> pos 32

        // now at 32: TOC
        // TOC Count: 2
        let mut toc_buffer = Vec::new();
        toc_buffer.write_u32::<LittleEndian>(2)?;

        // Entry 1 (Dummy)
        toc_buffer.extend_from_slice(&[0u8; 32]); // Name
        toc_buffer.write_u32::<LittleEndian>(0)?; // Length
        toc_buffer.write_u32::<LittleEndian>(0)?; // Offset
        toc_buffer.write_u32::<LittleEndian>(1)?; // Flag

        // Entry 2 (Info)
        let info_content = "TITLE=Test Book\nAUTHOR=Test Author";
        let info_len = info_content.len() as u32;

        toc_buffer.extend_from_slice(&[0u8; 32]);
        toc_buffer.write_u32::<LittleEndian>(info_len)?;

        // Calculate offsets
        let toc_size = 4 + 44 + 44; // Count + Entry1 + Entry2
        let info_actual_offset = toc_start + toc_size as u32;

        toc_buffer.write_u32::<LittleEndian>(info_actual_offset)?; // Offset
        toc_buffer.write_u32::<LittleEndian>(2)?; // Flag 2

        buffer.write_all(&toc_buffer)?;
        buffer.write_all(info_content.as_bytes())?;

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Test Book");
        assert_eq!(mi.authors, vec!["Test Author"]);

        Ok(())
    }
}
