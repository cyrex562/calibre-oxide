//! Port of the header/TOC reading in `reader.py`'s `Reader.verify_file`
//! and `Reader.get_toc`.
//!
//! Verified byte layout (matches `RBWriter.write_content` in
//! `writer.py`, byte-for-byte):
//!
//! ```text
//! 0..14    HEADER magic (14 bytes)
//! 14..18   u32 0
//! 18..24   u32 0, u16 0            (packed '<IH', 6 bytes)
//! 24..28   u32 0x128                <- "toc_offset": position of the
//!                                      TOC's page-count field, NOT the
//!                                      first TOC entry
//! 28..32   u32 total_file_size      <- verify_file's size-check field;
//!                                      written as a 0 placeholder here,
//!                                      then the true value is patched in
//!                                      at the very end via
//!                                      `seek(0x1c); write(total_size)`
//!                                      -- 0x1c == 28, the SAME field,
//!                                      not a distinct one
//! 32..296  66 x u32 0               (exactly fills bytes 0x20..0x128)
//! 296..300 u32 page_count           <- what toc_offset (0x128) points at
//! 300..    TOC entries, 44 bytes each
//! ```

use crate::lit::urlunquote;
use crate::rb::RocketBookError;
use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub const MAGIC: &[u8] = b"\xB0\x0C\xB0\x0C\x02\x00NUVO\x00\x00\x00\x00";

#[derive(Debug, Clone)]
pub struct RbHeader {
    /// Byte offset of the TOC's page-count field (the value stored at
    /// file offset 24). TOC entries begin 4 bytes after this.
    pub toc_offset: u32,
    /// Number of TOC entries (the `pages` field read at `toc_offset`).
    pub toc_count: u32,
    /// The file's total size, as recorded at byte 28 -- verified
    /// against the stream's real length by [`RbHeader::parse`].
    pub total_size: u32,
}

impl RbHeader {
    /// Port of `Reader.verify_file` + the offset/count-reading half of
    /// `Reader.get_toc`.
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; 14];
        reader.read_exact(&mut header)?;
        if header != MAGIC {
            return Err(RocketBookError::InvalidHeader.into());
        }

        reader.seek(SeekFrom::Start(28))?;
        let total_size = reader.read_u32::<LittleEndian>()?;
        let real_size = reader.seek(SeekFrom::End(0))?;
        if total_size as u64 != real_size {
            return Err(RocketBookError::SizeMismatch.into());
        }

        reader.seek(SeekFrom::Start(24))?;
        let toc_offset = reader.read_u32::<LittleEndian>()?;

        reader.seek(SeekFrom::Start(toc_offset as u64))?;
        let toc_count = reader.read_u32::<LittleEndian>()?;

        Ok(RbHeader {
            toc_offset,
            toc_count,
            total_size,
        })
    }
}

pub struct RbTocEntry {
    pub name: String,
    pub length: u32,
    pub offset: u32,
    pub flag: u32,
}

impl RbTocEntry {
    /// Port of the per-entry read in `Reader.get_toc`:
    /// `name = unquote(self.stream.read(32).strip(b'\x00'))`, then
    /// `size, offset, flags = read_i32(), read_i32(), read_i32()`.
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let mut name_bytes = [0u8; 32];
        reader.read_exact(&mut name_bytes)?;

        // `bytes.strip(b'\x00')`: strip leading *and* trailing nulls,
        // before decoding -- not just a null-terminated-string read.
        let trimmed = name_bytes
            .as_slice()
            .strip_prefix_matches(0)
            .strip_suffix_matches(0);
        let decoded = String::from_utf8_lossy(trimmed).into_owned();
        let name = urlunquote(&decoded);

        let length = reader.read_u32::<LittleEndian>()?;
        let offset = reader.read_u32::<LittleEndian>()?;
        let flag = reader.read_u32::<LittleEndian>()?;

        Ok(RbTocEntry {
            name,
            length,
            offset,
            flag,
        })
    }
}

/// Small helper trait so [`RbTocEntry::read`] can strip leading *and*
/// trailing null bytes (Python's `bytes.strip(b'\x00')`) without pulling
/// in a crate for it.
trait StripByte {
    fn strip_prefix_matches(&self, b: u8) -> &Self;
    fn strip_suffix_matches(&self, b: u8) -> &Self;
}

impl StripByte for [u8] {
    fn strip_prefix_matches(&self, b: u8) -> &Self {
        let start = self.iter().position(|&x| x != b).unwrap_or(self.len());
        &self[start..]
    }

    fn strip_suffix_matches(&self, b: u8) -> &Self {
        let end = self.iter().rposition(|&x| x != b).map_or(0, |i| i + 1);
        &self[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use std::io::{Cursor, Write};

    /// Builds a minimal, spec-correct RB file: header, zero-filled
    /// region, page count, `count` TOC entries (all zero size/offset/
    /// flag), then patches in the real total size at byte 28.
    fn build_rb_shell(entry_names: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.write_u32::<LittleEndian>(0).unwrap();
        buf.write_u32::<LittleEndian>(0).unwrap();
        buf.write_u16::<byteorder::LittleEndian>(0).unwrap();
        buf.write_u32::<LittleEndian>(0x128).unwrap(); // toc_offset
        buf.write_u32::<LittleEndian>(0).unwrap(); // total size placeholder
        for _ in (0x20..0x128).step_by(4) {
            buf.write_u32::<LittleEndian>(0).unwrap();
        }
        assert_eq!(buf.len(), 0x128);
        buf.write_u32::<LittleEndian>(entry_names.len() as u32)
            .unwrap();
        for name in entry_names {
            let mut field = [0u8; 32];
            let bytes = name.as_bytes();
            field[..bytes.len()].copy_from_slice(bytes);
            buf.write_all(&field).unwrap();
            buf.write_u32::<LittleEndian>(0).unwrap();
            buf.write_u32::<LittleEndian>(0).unwrap();
            buf.write_u32::<LittleEndian>(0).unwrap();
        }
        let total = buf.len() as u32;
        (&mut buf[28..32]).write_u32::<LittleEndian>(total).unwrap();
        buf
    }

    #[test]
    fn parses_a_valid_header() {
        let buf = build_rb_shell(&["index.html", "0.png"]);
        let mut cursor = Cursor::new(buf);
        let header = RbHeader::parse(&mut cursor).unwrap();
        assert_eq!(header.toc_offset, 0x128);
        assert_eq!(header.toc_count, 2);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = build_rb_shell(&[]);
        buf[0] = 0xFF;
        let mut cursor = Cursor::new(buf);
        let err = RbHeader::parse(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("RocketBook Header"), "{err}");
    }

    #[test]
    fn rejects_mismatched_total_size() {
        let mut buf = build_rb_shell(&[]);
        buf.extend_from_slice(b"trailing garbage that isn't accounted for");
        let mut cursor = Cursor::new(buf);
        let err = RbHeader::parse(&mut cursor).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("corrupt"), "{err}");
    }

    #[test]
    fn toc_entry_names_are_unquoted() {
        let mut buf = Vec::new();
        let mut field = [0u8; 32];
        let encoded = b"caf%C3%A9.html";
        field[..encoded.len()].copy_from_slice(encoded);
        buf.write_all(&field).unwrap();
        buf.write_u32::<LittleEndian>(10).unwrap();
        buf.write_u32::<LittleEndian>(300).unwrap();
        buf.write_u32::<LittleEndian>(0).unwrap();
        let mut cursor = Cursor::new(buf);
        let entry = RbTocEntry::read(&mut cursor).unwrap();
        assert_eq!(entry.name, "café.html");
        assert_eq!(entry.length, 10);
        assert_eq!(entry.offset, 300);
        assert_eq!(entry.flag, 0);
    }
}
