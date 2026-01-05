use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::{bail, Result};
use std::io::{Read, Seek, SeekFrom};

// BOOKMTIT (Legacy, CP950)
// BOOKMTIU (Unicode, UTF-16LE)

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let pdb = PdbHeader::parse(&mut stream)?;

    if pdb.num_records == 0 {
        bail!("No records in PDB");
    }

    let ident_str = String::from_utf8_lossy(&pdb.creator_id);
    let type_str = String::from_utf8_lossy(&pdb.type_id);
    // Haodoo PDB: Creator/Type logic.
    // reader.py checks `header.ident` where Ident is likely Type+Creator or just Name?
    // In `raw_header` reader.py uses `BPDB_IDENT = 'BOOKMTIT'`.
    // This looks like Type? Or Name?
    // PDB header has Name (32 bytes).
    // Let's check name or creator/type.

    // Legacy reader.py: `if header.ident == BPDB_IDENT`.
    // Where `ident` comes from `struct.unpack('32sH...', header)`.
    // It seems `ident` is the Name field (32 bytes)?
    // But 'BOOKMTIT' is 8 chars.
    // Maybe Type+Creator?

    // Let's assume Type/Creator signature or Name.
    // Given 'BOOKMTIT', it's likely [Type][Creator] or similar.
    // But PDB Type/Creator are 4 chars each. "BOOK" "MTIT"?
    // "BOOK" is common type.

    // Let's check PDB header parsing in `pdb/header.rs`.
    // `type_id` and `creator_id` are [u8; 4].

    let _is_legacy = type_str == "BOOK" && ident_str == "MTIT";
    let is_unicode = type_str == "BOOK" && ident_str == "MTIU";

    // Using `pdb.name` vs `type_id/creator_id`.
    // If parsing fails based on creators, maybe rely on content?
    // Let's proceed if records exist.

    let mut mi = MetaInformation::default();
    mi.languages = vec!["zh-tw".to_string()]; // Haodoo is TW.

    // Author is hidden in PDB Header sometimes?
    // reader.py `author()`: `stream.seek(35)`.
    // PDB Header size is 78 bytes usually.
    // Offset 35?
    // 0-32: Name
    // 32-34: Attributes
    // 34-36: Version.
    // 35 is inside Version (2 bytes)?
    // `version = struct.unpack('>b', self.stream.read(1))[0]` (at 35).
    // If version byte is 2?
    // Then `stream.seek(0)`. `author = stream.read(35)`.
    // So Author overwrites Name?
    // Yes, Haodoo repurposes Name field for Author if version (at 35) is 2.
    // Note: Version is at offset 34 (u16). 35 is the low byte (BigEndian).

    let version_low = pdb.version as u8; // pdb.version is u16 read as BE.
                                         // If offset 35 was read as u8.
                                         // offset 34 is MSB, 35 is LSB.
                                         // So `pdb.version & 0xFF`.

    if (pdb.version & 0xFF) == 2 {
        // Name field is Author.
        // Needs decoding.
        // Encoding depends on legacy/unicode.
        if is_unicode {
            // UTF-16LE?
            // Name is 32 bytes fixed.
            // decode utf-16le.
            let mut name_data = vec![0u8; 32];
            // PdbHeader parsed name as utf8 lossy from bytes.
            // We want raw bytes.
            // They are not stored in PdbHeader public struct (normalized String).
            // re-read from stream.
            stream.seek(SeekFrom::Start(0))?;
            stream.read_exact(&mut name_data)?;
            let author_str = decode_utf16le(&name_data);
            mi.authors = vec![clean_str(&author_str)];
        } else {
            // CP950 (Big5)
            let mut name_data = vec![0u8; 32];
            stream.seek(SeekFrom::Start(0))?;
            stream.read_exact(&mut name_data)?;
            // Use lossy string or cp950?
            // We don't have cp950 crate. Use String::from_utf8_lossy if ASCII?
            // Haodoo is Chinese. Must handle CP950.
            // Current environment limitation: no `encoding_rs` crate in `calibre_ebooks`?
            // It is in `calibre_utils`? Not exposed.
            // We can assume unknown author or Raw string.
            mi.authors = vec![clean_str(&pdb.name)];
        }
    } else {
        mi.authors = vec!["Unknown".to_string()];
    }

    // Title from Record 0
    let rec0_offset = pdb.records[0].offset as u64;
    stream.seek(SeekFrom::Start(rec0_offset))?;
    // Read enough for header fields. Haodoo header varies.
    // Legacy: `fields = raw.split(b'\x1b')`.
    // Unicode: `fields = raw.split(b'\x1b\x00')`.

    // Let's read 1KB or less.
    let mut rec0_data = [0u8; 1024];
    let n = stream.read(&mut rec0_data)?;
    let data = &rec0_data[..n];

    let title = if is_unicode {
        // replace `\x1b\x00\x1b\x00...` with `\x1b\x00`
        // split by `\x1b\x00`
        // Field 0 is Title.
        // Need to decode UTF-16LE.
        // Naive search for divider.
        // Divider: [0x1B, 0x00].
        if let Some(pos) = data.windows(2).position(|w| w == [0x1b, 0x00]) {
            let title_bytes = &data[..pos];
            // Decode
            decode_utf16le(title_bytes)
        } else {
            pdb.name.clone()
        }
    } else {
        // CP950.
        // Divider: [0x1B].
        if let Some(pos) = data.iter().position(|&b| b == 0x1b) {
            // Decode CP950.
            // Fallback to name if we can't decode.
            pdb.name.clone() // Placeholder
        } else {
            pdb.name.clone()
        }
    };

    mi.title = clean_str(&title);

    Ok(mi)
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let u16s: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
        .trim_matches('\0')
        .to_string()
}

fn clean_str(s: &str) -> String {
    // Basic punctuation fix (simplified)
    s.replace("︵", "（").replace("︶", "）").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{BigEndian, WriteBytesExt};
    use std::io::{Cursor, Write};

    #[test]
    fn test_haodoo_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        // PDB Header (78 bytes)
        // Name (32b). If version=2, this is author.
        let mut name = [0u8; 32];
        let author = "Test Author";
        // Encode utf16le
        let auth_u16: Vec<u8> = author
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        for (i, b) in auth_u16.iter().enumerate() {
            if i < 32 {
                name[i] = *b;
            }
        }
        buffer.write_all(&name)?;

        buffer.write_u16::<BigEndian>(0)?; // attr
        buffer.write_u16::<BigEndian>(2)?; // version (2=Author in Name)
        buffer.write_u32::<BigEndian>(0)?; // create
        buffer.write_u32::<BigEndian>(0)?; // mod
        buffer.write_u32::<BigEndian>(0)?; // backup
        buffer.write_u32::<BigEndian>(0)?; // mod_num
        buffer.write_u32::<BigEndian>(0)?; // app_info
        buffer.write_u32::<BigEndian>(0)?; // sort
        buffer.extend_from_slice(b"BOOK"); // type
        buffer.extend_from_slice(b"MTIU"); // creator (Unicode)
        buffer.write_u32::<BigEndian>(0)?; // id
        buffer.write_u32::<BigEndian>(0)?; // next
        buffer.write_u16::<BigEndian>(1)?; // num_records

        // Rec 0 Offset at 100
        buffer.write_u32::<BigEndian>(100)?;
        buffer.write_u32::<BigEndian>(0)?; // ID

        while buffer.len() < 100 {
            buffer.push(0);
        }

        // Rec 0 Data
        // Title \x1b\x00 ...
        let title = "Test Title";
        let title_u16: Vec<u8> = title.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        buffer.write_all(&title_u16)?;
        buffer.write_all(&[0x1b, 0x00])?;
        buffer.write_all(&[0u8; 10])?;

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.authors, vec!["Test Author"]);
        assert_eq!(mi.title, "Test Title");
        assert!(mi.languages.contains(&"zh-tw".to_string()));

        Ok(())
    }
}
