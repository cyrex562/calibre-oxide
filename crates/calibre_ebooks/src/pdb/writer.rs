use crate::compression::palmdoc::compress;
use anyhow::Result;
use byteorder::{BigEndian, WriteBytesExt};
use chrono::Local;
use std::io::Write;

pub struct PdbWriter;

impl PdbWriter {
    pub fn new() -> Self {
        PdbWriter
    }

    pub fn write<W: Write>(&self, name: &str, content: &[u8], writer: &mut W) -> Result<()> {
        // Simple PDB Writer for PalmDoc content
        // 1. Compress Content
        let mut records = Vec::new();
        let chunk_size = 4096;
        for chunk in content.chunks(chunk_size) {
            let compressed = compress(chunk)?;
            records.push(compressed);
        }

        // 2. PDB Header
        let safe_name = name
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .collect::<String>();
        let mut db_name = [0u8; 32];
        let name_bytes = safe_name.as_bytes();
        let len = std::cmp::min(name_bytes.len(), 31);
        db_name[..len].copy_from_slice(&name_bytes[..len]);

        writer.write_all(&db_name)?;
        writer.write_u16::<BigEndian>(0)?; // Attributes
        writer.write_u16::<BigEndian>(0)?; // Version

        let now = Local::now().timestamp() as u32;
        writer.write_u32::<BigEndian>(now)?; // Creation
        writer.write_u32::<BigEndian>(now)?; // Modification
        writer.write_u32::<BigEndian>(0)?; // Backup
        writer.write_u32::<BigEndian>(0)?; // Mod Num
        writer.write_u32::<BigEndian>(0)?; // App Info
        writer.write_u32::<BigEndian>(0)?; // Sort Info

        writer.write_all(b"TEXt")?; // Type
        writer.write_all(b"REAd")?; // Creator

        writer.write_u32::<BigEndian>(0)?; // UID Seed
        writer.write_u32::<BigEndian>(0)?; // Next Rec List

        // 3. Record List
        // Rec 0: PalmDoc Header (Compression info)
        // Rec 1..N: Text
        let num_records = 1 + records.len();
        writer.write_u16::<BigEndian>(num_records as u16)?;

        // Offset Calculation
        // Header (78) + (NumRecs * 8) + 2 (Pad)
        let base_offset = 78 + (num_records as u32 * 8) + 2;
        let mut current_offset = base_offset;

        // Rec 0 (Compression Header) - 16 bytes
        writer.write_u32::<BigEndian>(current_offset)?;
        writer.write_all(&[0x00, 0x00, 0x00, 0x00])?; // Attr + UID
        current_offset += 16;

        for record in &records {
            writer.write_u32::<BigEndian>(current_offset)?;
            writer.write_all(&[0x00, 0x00, 0x00, 0x00])?;
            current_offset += record.len() as u32;
        }

        writer.write_u16::<BigEndian>(0)?; // Gap

        // 4. Write Data

        // Rec 0: PalmDoc Header
        writer.write_u16::<BigEndian>(2)?; // Compression (PalmDoc)
        writer.write_u16::<BigEndian>(0)?; // Unused
        writer.write_u32::<BigEndian>(content.len() as u32)?; // Uncompressed Length
        writer.write_u16::<BigEndian>(records.len() as u16)?; // Record Count
        writer.write_u16::<BigEndian>(4096)?; // Record Size
        writer.write_u32::<BigEndian>(0)?; // Encryption + Unknown

        // Text Records
        for record in &records {
            writer.write_all(record)?;
        }

        Ok(())
    }
}
