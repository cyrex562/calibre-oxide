use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub const MAGIC: &[u8] = b"\xB0\x0C\xB0\x0C\x02\x00NUVO\x00\x00\x00\x00";

#[derive(Debug, Clone)]
pub struct RbHeader {
    pub toc_offset: u32,
    pub toc_count: u32,
}

impl RbHeader {
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        // Check header
        let mut header = [0u8; 14];
        reader.read_exact(&mut header)?;
        if header != MAGIC {
            bail!("Invalid RB header");
        }

        // Skip 10 bytes (unknown/reserved)
        reader.seek(SeekFrom::Current(10))?;

        // Read TOC offset
        let toc_offset = reader.read_u32::<LittleEndian>()?;

        // Go to TOC to read count
        reader.seek(SeekFrom::Start(toc_offset as u64))?;
        let toc_count = reader.read_u32::<LittleEndian>()?;

        Ok(RbHeader {
            toc_offset,
            toc_count,
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
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let mut name_bytes = [0u8; 32];
        reader.read_exact(&mut name_bytes)?;

        // Null terminated string
        let name = String::from_utf8_lossy(&name_bytes)
            .trim_matches(char::from(0))
            .to_string();

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
