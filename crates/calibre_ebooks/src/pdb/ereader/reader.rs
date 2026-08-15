//! Port of `calibre.ebooks.pdb.ereader.reader`: dispatches between
//! [`Reader132`] and [`Reader202`] based on record 0's length.

use std::io::{Read, Seek};
use std::path::Path;

use anyhow::Result;
use encoding_rs::Encoding;

use super::reader132::Reader132;
use super::reader202::Reader202;
use crate::pdb::ereader::EreaderError;
use crate::pdb::formatreader::FormatReader;
use crate::pdb::header::PdbHeader;

/// The eReader `FormatReader`: wraps whichever concrete reader matches
/// this file's record 0 length.
#[derive(Debug)]
pub enum Reader {
    V132(Reader132),
    V202(Reader202),
}

impl Reader {
    /// Port of `Reader.__init__`: reads record 0's length from `header`
    /// and picks [`Reader132`] (132 bytes) or [`Reader202`] (116 or 202
    /// bytes), erroring on anything else.
    pub fn new<R: Read + Seek>(
        header: &PdbHeader,
        stream: &mut R,
        encoding: Option<&'static Encoding>,
    ) -> Result<Self> {
        let record0_size = header.section_data(stream, 0)?.len();

        match record0_size {
            132 => Ok(Reader::V132(Reader132::new(header, stream, encoding)?)),
            116 | 202 => Ok(Reader::V202(Reader202::new(header, stream, encoding)?)),
            other => Err(EreaderError::msg(format!(
                "Size mismatch. eReader header record size {other} KB is not supported."
            ))
            .into()),
        }
    }

    /// Port of `Reader.dump_pml`.
    pub fn dump_pml(&self) -> Result<String> {
        match self {
            Reader::V132(r) => r.dump_pml(),
            Reader::V202(r) => r.dump_pml(),
        }
    }

    /// Port of `Reader.dump_images`.
    pub fn dump_images(&self, out_dir: &Path) -> Result<()> {
        match self {
            Reader::V132(r) => r.dump_images(out_dir),
            Reader::V202(r) => r.dump_images(out_dir),
        }
    }
}

impl FormatReader for Reader {
    fn extract_content(&self, output_dir: &Path) -> Result<()> {
        match self {
            Reader::V132(r) => r.extract_content(output_dir),
            Reader::V202(r) => r.extract_content(output_dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::palmdoc;
    use byteorder::{BigEndian, ByteOrder, WriteBytesExt};
    use std::io::Cursor;
    use std::io::Write;

    fn build_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut name = b"Test Book".to_vec();
        name.resize(32, 0);
        buffer.extend_from_slice(&name);
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.extend_from_slice(b"PNRd");
        buffer.extend_from_slice(b"PPrs");
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(records.len() as u16).unwrap();

        let base_offset = 78 + (records.len() as u32 * 8) + 2;
        let mut offset = base_offset;
        for r in &records {
            buffer.write_u32::<BigEndian>(offset).unwrap();
            buffer.write_all(&[0u8, 0, 0, 0]).unwrap();
            offset += r.len() as u32;
        }
        buffer.write_u16::<BigEndian>(0).unwrap();

        for r in &records {
            buffer.extend_from_slice(r);
        }
        buffer
    }

    #[test]
    fn dispatches_to_reader132_for_132_byte_record0() {
        let plain = b"hi";
        let compressed = palmdoc::compress(plain).unwrap();
        let mut record0 = vec![0u8; 132];
        BigEndian::write_u16(&mut record0[0..2], 2); // compression
        BigEndian::write_u16(&mut record0[12..14], 2); // non_text_offset
        BigEndian::write_u16(&mut record0[40..42], 2); // image_data_offset
        BigEndian::write_u16(&mut record0[44..46], 2); // metadata_offset

        let raw = build_pdb(vec![record0, compressed]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader::new(&header, &mut cursor, None).unwrap();
        assert!(matches!(reader, Reader::V132(_)));
        assert_eq!(reader.dump_pml().unwrap(), "hi");
    }

    #[test]
    fn dispatches_to_reader202_for_202_byte_record0() {
        let plain = b"hi";
        let compressed = palmdoc::compress(plain).unwrap();
        let xored: Vec<u8> = compressed.iter().map(|b| b ^ 0xA5).collect();
        let mut record0 = vec![0u8; 202];
        BigEndian::write_u16(&mut record0[0..2], 2); // version
        BigEndian::write_u16(&mut record0[8..10], 2); // non_text_offset

        let raw = build_pdb(vec![record0, xored]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader::new(&header, &mut cursor, None).unwrap();
        assert!(matches!(reader, Reader::V202(_)));
        assert_eq!(reader.dump_pml().unwrap(), "hi");
    }

    #[test]
    fn rejects_unsupported_record0_size() {
        let raw = build_pdb(vec![vec![0u8; 50]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let err = Reader::new(&header, &mut cursor, None).unwrap_err();
        assert!(err.to_string().contains("Size mismatch"), "{err}");
    }
}
