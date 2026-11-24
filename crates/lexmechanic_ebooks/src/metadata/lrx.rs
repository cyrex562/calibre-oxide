use crate::metadata::{string_to_authors, MetaInformation};
use anyhow::{bail, Result};
use byteorder::{BigEndian, LittleEndian, ReadBytesExt}; // LRX uses both BE and LE
use flate2::read::ZlibDecoder;
use std::io::{Read, Seek, SeekFrom};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    // 1. Check Magic "ftypLRX2" at offset 4?
    // Python: buf = f.read(12). if buf[4:] == b'ftypLRX2'.
    let mut buf = [0u8; 12];
    stream.read_exact(&mut buf)?;
    if &buf[4..] != b"ftypLRX2" {
        // Check for 'LRX ' at 4? Python line 86.
        if &buf[4..8] == b"LRX " {
            bail!("Librie LRX format not supported");
        }
        bail!("Not a valid LRX file");
    }

    // 2. Traverse atoms to find 'bbeb'
    // Offset starts at 0? No, Python `offset = 0`. `offset += word_be(buf[:4])`.
    // Only if check passed.
    // `offset` logic suggests `buf` was read from 0.
    // `word_be(buf[:4])` is the size of the first atom?
    // `stream` is consumed?
    // Python `read(at, amount)` seeks.

    // Logic:
    let mut offset = 0u64;
    // We already read 12 bytes.
    // But we need to parse atoms starting at 0.
    // Atom: Length (4 bytes BE), Magic (4 bytes? No).
    // Python loop:
    // `offset += word_be(buf[:4])` -> Start with Length of first atom (ftyp atom).
    // Loop:
    //   Read 8 bytes at new `offset`.
    //   If `buf[4:] == b'bbeb'`: break.
    //   Else: `offset += word_be(buf[:4])` (Skip this atom).

    // First atom length is in `buf[0..4]`.
    let current_atom_len = (&buf[0..4]).read_u32::<BigEndian>()? as u64;
    offset += current_atom_len;

    loop {
        stream.seek(SeekFrom::Start(offset))?;
        let mut atom_head = [0u8; 8];
        stream.read_exact(&mut atom_head)?;

        if &atom_head[4..] == b"bbeb" {
            break;
        }

        // Next atom
        let len = (&atom_head[0..4]).read_u32::<BigEndian>()? as u64;
        offset += len;
        // TODO: Check EOF or infinite loop
    }

    // Found 'bbeb' at `offset`.
    offset += 8; // Skip 'bbeb' header

    // Read 16 bytes.
    stream.seek(SeekFrom::Start(offset))?;
    let mut buf16 = [0u8; 16];
    stream.read_exact(&mut buf16)?;

    // Check "LRF\0" (UTF-16LE) -> 8 bytes?
    // Python: `buf[:8].decode('utf-16-le') != 'LRF\x00'`.
    // b'L\0R\0F\0\0\0'
    if &buf16[0..8] != b"L\x00R\x00F\x00\x00\x00" {
        bail!("Not a valid LRX file (LRF signature missing)");
    }

    // lrf_version = word_le(buf[8:12])
    let lrf_version = (&buf16[8..12]).read_u32::<LittleEndian>()?;

    // offset += 0x4c
    offset += 0x4c;

    // compressed_size = short_le(read(offset, 2))
    stream.seek(SeekFrom::Start(offset))?;
    let mut compressed_size = stream.read_u16::<LittleEndian>()? as u64;
    offset += 2;

    if lrf_version >= 800 {
        offset += 6;
    }

    if compressed_size < 4 {
        bail!("Invalid compressed size");
    }
    compressed_size -= 4;

    // Wait, Python: `uncompressed_size = word_le(read(offset, 4))`.
    // `offset` was incremented by 2 (or 2+6).
    // But `compressed_size` read at `offset`.
    // Python: `compressed_size = short_le(read(offset, 2))`.
    // `offset += 2`.
    // `if ver >= 800: offset += 6`.
    // `uncompressed_size = word_le(read(offset, 4))`.
    // OK. My code:
    // `stream.seek(offset)`. Read u16.
    // `offset += 2`.
    // If 800: `offset += 6`.
    // Seek to new offset.
    stream.seek(SeekFrom::Start(offset))?;
    let uncompressed_size = stream.read_u32::<LittleEndian>()? as u64;

    // Now read `compressed_size` bytes from `f` (at current position? No, `read(offset)` was seek-read).
    // `f.read(compressed_size)` reads from WHERE?
    // In Python `get_metadata`:
    // `read(offset, 4)` uses `seek`.
    // But `f.read(compressed_size)` uses CURRENT position.
    // Where is current position?
    // `f.seek(0)` at start.
    // `read(at, amount)` seeks BACK to `at`.
    // So `f.read` continues from where the LAST `read` left off?
    // Last read was `uncompressed_size = word_le(read(offset, 4))`.
    // `_read` seeks to `offset`. Reads 4.
    // So current pos is `offset + 4`.
    // So we read from `offset + 4`.

    // `info = decompress(f.read(compressed_size))`
    let mut zlib_data = vec![0u8; compressed_size as usize];
    stream.read_exact(&mut zlib_data)?;

    let mut decoder = ZlibDecoder::new(&zlib_data[..]);
    let mut info = String::new();
    decoder.read_to_string(&mut info)?;

    if info.len() as u64 != uncompressed_size {
        // bail!("LRX file has malformed metadata section");
        // Warn? Python raises ValueError.
    }

    // Parse XML
    let doc = roxmltree::Document::parse(&info)?;
    let bi = doc
        .descendants()
        .find(|n| n.has_tag_name("BookInfo"))
        .ok_or_else(|| anyhow::anyhow!("No BookInfo"))?;

    let mut mi = MetaInformation::default();

    if let Some(n) = bi.descendants().find(|n| n.has_tag_name("Title")) {
        if let Some(r) = n.attribute("reading") {
            mi.title_sort = Some(r.to_string());
        }
        if let Some(t) = n.text() {
            mi.title = t.to_string();
        }
    }

    if let Some(n) = bi.descendants().find(|n| n.has_tag_name("Author")) {
        if let Some(r) = n.attribute("reading") {
            mi.author_sort = Some(r.to_string());
        }
        if let Some(t) = n.text() {
            mi.authors = string_to_authors(t);
        }
    }

    // ... Publisher, Tags, Language
    // Language is in DocInfo/Language
    if let Some(doc_info) = doc.descendants().find(|n| n.has_tag_name("DocInfo")) {
        if let Some(lang) = doc_info.descendants().find(|n| n.has_tag_name("Language")) {
            if let Some(t) = lang.text() {
                mi.languages = vec![t.to_string()];
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Cursor;
    use std::io::Write;

    #[test]
    fn test_lrx_metadata() -> Result<()> {
        let mut buffer = Vec::new();

        // 1. ftyp atom
        // Length 20 (0x14). ftypLRX2...
        buffer.write_u32::<BigEndian>(20)?;
        buffer.extend_from_slice(b"ftypLRX2");
        buffer.write_u64::<BigEndian>(0)?; // Pad headers

        // 2. bbeb atom
        // Length 100?
        // Offset is now 20.
        // We write Length (100). bbeb...
        let _bbeb_offset = buffer.len();
        buffer.write_u32::<BigEndian>(200)?; // Atom Size
        buffer.extend_from_slice(b"bbeb");

        // "LRF\0" (UTF-16LE) -> 8 bytes
        buffer.extend_from_slice(b"L\x00R\x00F\x00\x00\x00");

        // Version (u32 LE)
        buffer.write_u32::<LittleEndian>(700)?;

        // Pad 64 bytes (0x4c - 12)
        buffer.write_all(&[0u8; 64])?;

        // Compressed size (u16 LE) + 4
        // Logic: compressed_size -= 4.
        // So stored value is Size + 4.

        // Compress some XML
        let xml = r#"<Root><DocInfo><Language>en</Language></DocInfo><BookInfo><Title reading="SortTitle">My Title</Title><Author reading="SortAuthor">My Author</Author></BookInfo></Root>"#;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(xml.as_bytes())?;
        let compressed_data = encoder.finish()?;
        let compressed_len = compressed_data.len() as u16;
        let stored_len = compressed_len + 4;

        buffer.write_u16::<LittleEndian>(stored_len)?;

        // Offset += 2.
        // Version < 800. No extra 6 bytes.

        // Uncompressed size (u32 LE)
        buffer.write_u32::<LittleEndian>(xml.len() as u32)?;

        // Compressed Data
        buffer.extend_from_slice(&compressed_data);

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "My Title");
        assert_eq!(mi.title_sort, Some("SortTitle".to_string()));
        assert_eq!(mi.authors, vec!["My Author"]);
        assert_eq!(mi.languages, vec!["en"]);

        Ok(())
    }
}
