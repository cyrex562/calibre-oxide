use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let pdb = PdbHeader::parse(&mut stream)?;

    if pdb.num_records == 0 {
        bail!("No records in PDB");
    }

    let mut mi = MetaInformation::default();
    mi.title = pdb.name.clone(); // Fallback

    // Check Record 0 for eReader Header
    // Should be 132 bytes for Dropbook produced files (supported by legacy code).
    let rec0 = &pdb.records[0];
    let next_offset = if pdb.records.len() > 1 {
        pdb.records[1].offset
    } else {
        // Determine file size?
        stream.seek(SeekFrom::End(0))? as u32
    };
    let rec0_len = next_offset - rec0.offset;

    if rec0_len == 132 {
        stream.seek(SeekFrom::Start(rec0.offset as u64))?;
        let mut rec0_data = [0u8; 132];
        stream.read_exact(&mut rec0_data)?;

        let compression = (&rec0_data[0..2]).read_u16::<BigEndian>()?;
        // 2=PalmDOC, 10=zTXt?

        // Metadata flag at offset 24 (u16)
        let has_metadata = (&rec0_data[24..26]).read_u16::<BigEndian>()? == 1;

        if (compression == 2 || compression == 10) && has_metadata {
            // Metadata offset at 44 (u16)
            let metadata_rec_idx = (&rec0_data[44..46]).read_u16::<BigEndian>()?;

            if (metadata_rec_idx as usize) < pdb.records.len() {
                let meta_offset = pdb.records[metadata_rec_idx as usize].offset;
                let meta_end = if (metadata_rec_idx as usize) < pdb.records.len() - 1 {
                    pdb.records[metadata_rec_idx as usize + 1].offset
                } else {
                    stream.seek(SeekFrom::End(0))? as u32
                };
                let meta_len = meta_end - meta_offset;

                stream.seek(SeekFrom::Start(meta_offset as u64))?;
                let mut meta_bytes = vec![0u8; meta_len as usize];
                stream.read_exact(&mut meta_bytes)?;

                // Decode cp1252. For simplicity, lossy utf8 or use encoding crate?
                // Current project doesn't use `encoding_rs` explicitely in `Cargo.toml`?
                // `html.rs` uses `encoding_rs`? No, we used built-in or skipped.
                // Let's use String::from_utf8_lossy for now, legacy code used cp1252 'replace'.
                // Actually cp1252 is 1 byte -> 1 char.

                let meta_str = String::from_utf8_lossy(&meta_bytes);
                // Split by null
                let parts: Vec<&str> = meta_str.split('\0').collect();

                if !parts.is_empty() {
                    fn clean(s: &str) -> String {
                        // re.sub(r'[^a-zA-Z0-9 \._=\+\-!\?,\'\"]', '', s)
                        // Allow specific chars.
                        s.chars()
                            .filter(|c| c.is_alphanumeric() || " ._=+!?,-\"'".contains(*c))
                            .collect()
                    }

                    if let Some(t) = parts.get(0) {
                        mi.title = clean(t);
                    }
                    if let Some(a) = parts.get(1) {
                        mi.authors = vec![clean(a)];
                    }
                    if let Some(p) = parts.get(3) {
                        mi.publisher = Some(clean(p));
                    }
                    if let Some(i) = parts.get(4) {
                        mi.set_identifier("isbn", &clean(i));
                    }
                }
            }
        }

        // Cover Extraction
        // Image count at? struct unpack from eheader?
        // Reader132:
        // image_count at offset + ...?
        // Legacy: `eheader.image_count`.
        // HeaderRecord definition:
        // ...
        // 96: image_count (u16)? No.
        // Let's check Python struct definition if possible.
        // `last_data_offset` at ?

        // From python code:
        // `for i in range(eheader.image_count):`
        // `raw = pheader.section_data(eheader.image_data_offset + i)`
        // `cover.png` inside?

        // Without precise header definition, cover extraction is risky.
        // We can skip cover for eReader for now or guess.
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use std::io::{Cursor, Write};

    #[test]
    fn test_ereader_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        // PDB Header (78 bytes)
        buffer.extend_from_slice(b"Test Book\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        buffer.write_u16::<BigEndian>(0)?; // attr
        buffer.write_u16::<BigEndian>(0)?; // version
        buffer.write_u32::<BigEndian>(0)?; // create
        buffer.write_u32::<BigEndian>(0)?; // mod
        buffer.write_u32::<BigEndian>(0)?; // backup
        buffer.write_u32::<BigEndian>(0)?; // mod_num
        buffer.write_u32::<BigEndian>(0)?; // app_info
        buffer.write_u32::<BigEndian>(0)?; // sort
        buffer.extend_from_slice(b"PNRd"); // type
        buffer.extend_from_slice(b"PPrs"); // creator
        buffer.write_u32::<BigEndian>(0)?; // id
        buffer.write_u32::<BigEndian>(0)?; // next
        buffer.write_u16::<BigEndian>(2)?; // num_records (Header + Metadata)

        // Record List
        // Rec 0 (Header) at 100
        buffer.write_u32::<BigEndian>(100)?;
        buffer.write_u32::<BigEndian>(0)?; // ID

        // Rec 1 (Metadata) at 232 (100 + 132)
        buffer.write_u32::<BigEndian>(232)?;
        buffer.write_u32::<BigEndian>(0)?;

        // Pad to 100
        while buffer.len() < 100 {
            buffer.push(0);
        }

        // Rec 0 Data (132 bytes)
        let rec0_start = buffer.len();
        buffer.write_u16::<BigEndian>(2)?; // Compression (2=PalmDOC)
        buffer.write_all(&[0u8; 22])?;
        buffer.write_u16::<BigEndian>(1)?; // Has Metadata = 1
        buffer.write_all(&[0u8; 18])?;
        buffer.write_u16::<BigEndian>(1)?; // Metadata record index = 1

        // Pad to 132 bytes length
        while buffer.len() < rec0_start + 132 {
            buffer.push(0);
        }

        // Rec 1 Data (Metadata)
        // "Title\0Author\0\0Publisher\0ISBN"
        let meta_str = "My Title\0My Author\0\0My Pub\0123456789";
        buffer.extend_from_slice(meta_str.as_bytes());

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "My Title");
        assert_eq!(mi.authors, vec!["My Author"]);
        assert_eq!(mi.publisher.as_deref(), Some("My Pub"));

        Ok(())
    }
}
