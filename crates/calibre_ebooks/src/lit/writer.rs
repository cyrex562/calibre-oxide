use crate::lit::header::ITOLITLS;
use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::{Seek, SeekFrom, Write};

pub struct LitWriter;

impl LitWriter {
    pub fn new() -> Self {
        LitWriter
    }

    pub fn write_dummy<W: Write + Seek>(&self, writer: &mut W) -> Result<()> {
        // Write Basic valid LIT header structure for testing/export
        // ITOLITLS
        writer.write_all(ITOLITLS)?;
        writer.write_u32::<LittleEndian>(1)?; // Ver
        writer.write_i32::<LittleEndian>(40)?; // Hdr Len
        writer.write_i32::<LittleEndian>(2)?; // Num Pieces (0=Info, 1=Dir)
        writer.write_i32::<LittleEndian>(40)?; // Sec Hdr Len
        writer.write_all(&[0u8; 16])?; // GUID

        // Header Pieces (2 * 16 = 32 bytes)
        // Piece 0 (Dummy Metadata)
        writer.write_u32::<LittleEndian>(100)?; // Off
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_i32::<LittleEndian>(0)?; // Size
        writer.write_u32::<LittleEndian>(0)?;

        // Piece 1 (Directory)
        let dir_offset = 200;
        let dir_size = 64; // arbitrary simple Dir
        writer.write_u32::<LittleEndian>(dir_offset)?; // Off
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_i32::<LittleEndian>(dir_size)?; // Size
        writer.write_u32::<LittleEndian>(0)?;

        // Pad
        writer.seek(SeekFrom::Start(dir_offset as u64))?;

        // Write Minimal Directory (IFCM)
        writer.write_all(b"IFCM")?;
        writer.write_u32::<LittleEndian>(1)?;
        writer.write_i32::<LittleEndian>(40)?; // Chunk size
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_i32::<LittleEndian>(1)?; // Num chunks
                                              // Pad dir
        writer.write_all(&[0u8; 32])?;

        Ok(())
    }
}
