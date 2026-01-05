use crate::metadata::MetaInformation;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    if header != *b"TPZ0" && header != *b"TPZ1" && header != *b"TPZ2" && header != *b"TPZ3" {
        // Python just checks starts with TPZ
    }
    if &header[0..3] != b"TPZ" {
        bail!("Not a Topaz file");
    }

    let records_count_vwi = decode_vwi(&mut stream)?;
    let header_records = records_count_vwi.0;

    // Parse headers to find 'metadata' tag
    let mut metadata_offset = 0;
    let mut metadata_found = false;

    for _ in 0..header_records {
        let (tag_len, _) = decode_vwi(&mut stream)?;
        let mut tag_buf = vec![0u8; tag_len as usize];
        stream.read_exact(&mut tag_buf)?;
        let tag = String::from_utf8_lossy(&tag_buf).to_string();

        let (num_vals, _) = decode_vwi(&mut stream)?;

        // If tag is metadata, we need the offset of the first block
        if tag == "metadata" && num_vals > 0 {
            // Read first block info
            let (hdr_offset, _) = decode_vwi(&mut stream)?;
            metadata_offset = hdr_offset; // This is RELATIVE to base offset
            metadata_found = true;

            // Skip rest of this block info (len_uncomp, len_comp)
            let _ = decode_vwi(&mut stream)?;
            let _ = decode_vwi(&mut stream)?;

            // Skip remaining blocks for this tag
            for _ in 1..num_vals {
                let _ = decode_vwi(&mut stream)?;
                let _ = decode_vwi(&mut stream)?;
                let _ = decode_vwi(&mut stream)?;
            }
        } else {
            // Skip all blocks for this tag
            for _ in 0..num_vals {
                let _ = decode_vwi(&mut stream)?;
                let _ = decode_vwi(&mut stream)?;
                let _ = decode_vwi(&mut stream)?;
            }
        }
    }

    // After parsing all headers, read one more byte (eoth? Python says self.data[offset])
    // The loop consumes headers.
    // The base offset starts AFTER headers + 1 byte?
    // Python: `offset += 1; self.base = offset`
    let mut byte = [0u8; 1];
    stream.read_exact(&mut byte)?;
    let base_offset = stream.stream_position()?;

    if !metadata_found {
        bail!("No metadata record found in Topaz file");
    }

    // Seek to metadata block
    let abs_meta_offset = base_offset + metadata_offset as u64;
    stream.seek(SeekFrom::Start(abs_meta_offset))?;

    // Verify 'metadata' signature in body?
    // Python: `if self.data[md_offset+1:md_offset+9] != b'metadata': raise`
    // Wait, let's decode the metadata block header.

    // Metadata block format:
    // vwi(tag_len) + tag + flags(1) + num_recs(1)
    let (tag_len, _) = decode_vwi(&mut stream)?;
    let mut tag_buf = vec![0u8; tag_len as usize];
    stream.read_exact(&mut tag_buf)?;
    if tag_buf != b"metadata" {
        bail!("Damaged metadata record (tag mismatch)");
    }

    let mut flags = [0u8; 1];
    stream.read_exact(&mut flags)?;

    let mut num_recs = [0u8; 1];
    stream.read_exact(&mut num_recs)?;
    let count = num_recs[0] as usize;

    let mut metadata_map: HashMap<String, String> = HashMap::new();

    for _ in 0..count {
        let (t_len, _) = decode_vwi(&mut stream)?;
        let mut t_buf = vec![0u8; t_len as usize];
        stream.read_exact(&mut t_buf)?;
        let key = String::from_utf8_lossy(&t_buf).to_string();

        let (val_len, _) = decode_vwi(&mut stream)?;
        let mut val_buf = vec![0u8; val_len as usize];
        stream.read_exact(&mut val_buf)?;
        let val = String::from_utf8_lossy(&val_buf).to_string();

        metadata_map.insert(key, val);
    }

    let mut mi = MetaInformation::default();
    if let Some(title) = metadata_map.get("Title") {
        mi.title = title.clone();
    }
    if let Some(authors) = metadata_map.get("Authors") {
        mi.authors = authors.split(';').map(|s| s.trim().to_string()).collect();
    }

    Ok(mi)
}

fn decode_vwi<R: Read>(stream: &mut R) -> Result<(u32, usize)> {
    let mut val: u32 = 0;
    let mut shift: u32 = 0;
    let mut consumed = 0;
    let mut byte = [0u8; 1];

    loop {
        stream.read_exact(&mut byte)?;
        consumed += 1;
        let b = byte[0] as u32;

        let chunk = b & 0x7F;
        val = (val << 7) | chunk;

        if (b & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            // Safety break
            break;
        }
    }
    Ok((val, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Encode helper for tests
    fn encode_vwi(value: u32) -> Vec<u8> {
        let mut val = value;
        let mut chunks: Vec<u8> = Vec::new();

        if val == 0 {
            return vec![0];
        }

        while val > 0 {
            chunks.push((val & 0x7F) as u8);
            val >>= 7;
        }

        chunks.reverse();
        // Set high bit on all except last
        for i in 0..chunks.len() - 1 {
            chunks[i] |= 0x80;
        }
        chunks
    }

    #[test]
    fn test_topaz_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(b"TPZ0");

        // Helper to construct VWI and write
        let write_vwi = |v: u32, buf: &mut Vec<u8>| {
            buf.extend_from_slice(&encode_vwi(v));
        };

        // Header Records Count = 1 (just metadata)
        write_vwi(1, &mut buffer);

        // Header Record: "metadata"
        let tag = b"metadata";
        write_vwi(tag.len() as u32, &mut buffer);
        buffer.extend_from_slice(tag);

        // Num vals = 1 (one block)
        write_vwi(1, &mut buffer);

        // Block 1: Offset=?, LenUncomp=0, LenComp=0
        // Calculate offset:
        // Current buffer len is Header Base.
        // We are writing header NOW.
        // Metadata body will come AFTER header end + 1.

        // Let's placeholder offset. Logic is offset relative to BASE.
        // Base is after this header structure + 1.
        // So offset 0 means immediately after base.
        write_vwi(0, &mut buffer); // Offset 0
        write_vwi(0, &mut buffer); // Len uncomp (ignored)
        write_vwi(0, &mut buffer); // Len comp (ignored)

        // End of Headers byte
        buffer.push(0); // EOTH

        // BASE is here.
        // Metadata Body (Offset 0)

        // Body Header: VWI(len) + "metadata" + Flags(0) + Count(2)
        write_vwi(tag.len() as u32, &mut buffer);
        buffer.extend_from_slice(tag);
        buffer.push(0); // Flags
        buffer.push(2); // Count (Title, Authors)

        // Field 1: Title
        let key = b"Title";
        let val = b"Topaz Book";
        write_vwi(key.len() as u32, &mut buffer);
        buffer.extend_from_slice(key);
        write_vwi(val.len() as u32, &mut buffer);
        buffer.extend_from_slice(val);

        // Field 2: Authors
        let key = b"Authors";
        let val = b"Author One; Author Two";
        write_vwi(key.len() as u32, &mut buffer);
        buffer.extend_from_slice(key);
        write_vwi(val.len() as u32, &mut buffer);
        buffer.extend_from_slice(val);

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Topaz Book");
        assert_eq!(mi.authors, vec!["Author One", "Author Two"]);

        Ok(())
    }

    #[test]
    fn test_vwi_encoding() {
        // Test 0
        assert_eq!(encode_vwi(0), vec![0]);
        // Test 127
        assert_eq!(encode_vwi(127), vec![127]);
        // Test 128 (0x80) -> 1000 0000 -> 1 0000000 -> [10000001, 00000000] -> [0x81, 0x00]
        // Wait, logic:
        // 128 = 1000 0000 bin.
        // 7-bit chunks: lower 0000000 (0), next 0000001 (1).
        // Chunks: [1, 0].
        // Set MSB on 1 -> 0x81. Last is 0 -> 0x00.
        // Result: 0x81 00.
        assert_eq!(encode_vwi(128), vec![0x81, 0x00]);

        // Python VWI logic (from topaz.py):
        // if value == 0: return 0
        // while value: append(value & 0x7f); value >>= 7.
        // Then reverse?
        // Python `encode_vwi`:
        // loops while value.
        // if multi_byte: append(b | 0x80).
        // This suggests it writes High Bytes first?
        // "pack('>BBBB', ...)" -> Big Endian.
        // So yes, it writes High chunks first, with MSB set, last chunk MSB clear.
        // My implementation does `chunks.reverse()`, then sets bits. Correct.
    }
}
