use crate::rb::header::MAGIC;
use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::{Seek, SeekFrom, Write};

pub struct RbWriter {
    toc_entries: Vec<(String, Vec<u8>, u32)>, // name, data, flag
}

impl RbWriter {
    pub fn new() -> Self {
        RbWriter {
            toc_entries: Vec::new(),
        }
    }

    pub fn add_entry(&mut self, name: &str, data: Vec<u8>, flag: u32) {
        self.toc_entries.push((name.to_string(), data, flag));
    }

    pub fn write<W: Write + Seek>(&self, writer: &mut W) -> Result<()> {
        // 1. Write Header
        writer.write_all(MAGIC)?;
        writer.write_all(&[0u8; 10])?; // padding

        // Placeholder for TOC Offset
        let toc_offset_pos = writer.stream_position()?;
        writer.write_u32::<LittleEndian>(0)?;
        writer.write_u32::<LittleEndian>(0)?; // Extra padding? Based on metadata/rb.rs test, there's padding.

        // 2. Write Data Blocks (and track offsets)
        // Actually, in RB, typically the TOC is at the end or after content?
        // Let's look at `metadata/rb.rs`:
        // Header (14) + Pad (10) + TocOffset (4) + Pad (4?) = 32 bytes?
        // Wait, Header(14) + Pad(10) = 24.
        // TocOffset(4) = 28.
        // Pad(4) = 32.
        // So content starts at 32 usually? No, TOC offset points to TOC.
        // Let's write content first, then TOC.

        let mut current_offset = 32;
        let mut entry_offsets = Vec::new();

        // But wait, if we write content first, we need to know where it starts.
        // Valid RB file structure usually puts content after header, and TOC somewhere.
        // We can put TOC at the end.

        writer.seek(SeekFrom::Start(32))?;

        for (_, data, _) in &self.toc_entries {
            entry_offsets.push(current_offset);
            writer.write_all(data)?;
            current_offset += data.len() as u32;
        }

        // 3. Write TOC
        let toc_start = current_offset;
        writer.write_u32::<LittleEndian>(self.toc_entries.len() as u32)?;

        for (i, (name, data, flag)) in self.toc_entries.iter().enumerate() {
            // Name (32 bytes, null padded)
            let mut name_bytes = [0u8; 32];
            let bytes = name.as_bytes();
            let len = bytes.len().min(32);
            name_bytes[..len].copy_from_slice(&bytes[..len]);
            writer.write_all(&name_bytes)?;

            writer.write_u32::<LittleEndian>(data.len() as u32)?;
            writer.write_u32::<LittleEndian>(entry_offsets[i])?;
            writer.write_u32::<LittleEndian>(*flag)?;
        }

        // 4. Update Header with TOC Offset
        writer.seek(SeekFrom::Start(toc_offset_pos))?;
        writer.write_u32::<LittleEndian>(toc_start)?;

        Ok(())
    }
}
