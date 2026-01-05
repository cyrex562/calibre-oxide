use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

pub const ITOLITLS: &[u8] = b"ITOLITLS";

#[derive(Debug, Clone)]
pub struct LitHeader {
    pub version: u32,
    pub hdr_len: i32,
    pub num_pieces: i32,
    pub directory_offset: u32,
    pub directory_size: i32,
}

impl LitHeader {
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        // 1. Check Magic
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if magic != ITOLITLS {
            bail!("Not a valid LIT file");
        }

        // 2. Read Header
        let version = reader.read_u32::<LittleEndian>()?;
        let hdr_len = reader.read_i32::<LittleEndian>()?;
        let num_pieces = reader.read_i32::<LittleEndian>()?;
        let _sec_hdr_len = reader.read_i32::<LittleEndian>()?;

        // Skip GUID (16 bytes)
        reader.seek(SeekFrom::Current(16))?;

        // 3. Find Directory Piece (Index 1)
        reader.seek(SeekFrom::Start(hdr_len as u64))?;

        let mut directory_offset = 0;
        let mut directory_size = 0;
        let mut found_dir = false;

        for i in 0..num_pieces {
            let offset = reader.read_u32::<LittleEndian>()?;
            let _zero1 = reader.read_u32::<LittleEndian>()?;
            let size = reader.read_i32::<LittleEndian>()?;
            let _zero2 = reader.read_u32::<LittleEndian>()?;

            if i == 1 {
                directory_offset = offset;
                directory_size = size;
                found_dir = true;
            }
        }

        if !found_dir {
            bail!("Directory piece not found in LIT header");
        }

        Ok(LitHeader {
            version,
            hdr_len,
            num_pieces,
            directory_offset,
            directory_size,
        })
    }
}
