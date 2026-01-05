use anyhow::{Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct KindleAnnotation {
    pub id: String,
    pub displayed_location: u32,
    pub annotation_type: String,
    pub text: Option<String>,
}

#[derive(Debug)]
pub struct Bookmark {
    pub last_read_location: u32,
    pub last_read_offset: u32,
    pub timestamp: Option<SystemTime>,
    pub annotations: HashMap<u32, KindleAnnotation>,
}

impl Bookmark {
    pub fn parse(path: &Path, id: &str, extension: &str) -> Result<Self> {
        if extension == "mbp" {
            Self::parse_mbp(path, id)
        } else {
            // Stubs for tan, pdr
            // For now, return empty or error
            Ok(Bookmark {
                last_read_location: 0,
                last_read_offset: 0,
                timestamp: None,
                annotations: HashMap::new(),
            })
        }
    }

    fn parse_mbp(path: &Path, id: &str) -> Result<Self> {
        let mut file = File::open(path).context("Failed to open mbp file")?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .context("Failed to read mbp file")?;
        let mut reader = Cursor::new(&data);

        // Python:
        // self.timestamp, = unpack('>I', data[0x24:0x28])
        reader.seek(SeekFrom::Start(0x24))?;
        let timestamp_secs = reader.read_u32::<BigEndian>()?;
        // Convert to SystemTime
        let timestamp =
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(timestamp_secs as u64));

        // bpar_offset, = unpack('>I', data[0x4e:0x52])
        reader.seek(SeekFrom::Start(0x4e))?;
        let bpar_offset = reader.read_u32::<BigEndian>()? as u64;

        // lrlo = bpar_offset + 0x0c
        // self.last_read = int(unpack('>I', data[lrlo:lrlo+4])[0])
        reader.seek(SeekFrom::Start(bpar_offset + 0x0c))?;
        let last_read_offset = reader.read_u32::<BigEndian>()?;
        let magic_mobi_constant = 150;
        let last_read_location = last_read_offset / magic_mobi_constant + 1;

        // entries, = unpack('>I', data[0x4a:0x4e])
        reader.seek(SeekFrom::Start(0x4a))?;
        let _entries = reader.read_u32::<BigEndian>()?;

        // bpl = bpar_offset + 4
        // bpar_len, = unpack('>I', data[bpl:bpl+4])
        reader.seek(SeekFrom::Start(bpar_offset + 4))?;
        let bpar_len = reader.read_u32::<BigEndian>()?;
        // bpar_len += 8
        // eo = bpar_offset + bpar_len + 8
        let mut eo = bpar_offset + (bpar_len as u64) + 8;

        let mut annotations = HashMap::new();

        // Walk bookmark entries
        loop {
            if eo + 4 > data.len() as u64 {
                break;
            }
            reader.seek(SeekFrom::Start(eo))?;
            let mut sig = [0u8; 4];
            reader.read_exact(&mut sig)?;

            if &sig != b"DATA" {
                break;
            }

            // rec_len, = unpack('>I', data[eo+4:eo+8])
            reader.seek(SeekFrom::Start(eo + 4))?;
            let rec_len = reader.read_u32::<BigEndian>()? as u64;

            // Simplified parsing for now: skip detailed text extraction
            // to ensure basic structure works.

            eo += rec_len + 8;
        }

        // BKMK loop
        loop {
            if eo + 4 > data.len() as u64 {
                break;
            }
            reader.seek(SeekFrom::Start(eo))?;
            let mut sig = [0u8; 4];
            reader.read_exact(&mut sig)?;

            if &sig != b"BKMK" {
                break;
            }
            // Logic for BKMK to fixup Highlights...
            // end_loc at eo+0x10
            reader.seek(SeekFrom::Start(eo + 0x10))?;
            let end_loc = reader.read_u32::<BigEndian>()?;

            if end_loc != last_read_offset {
                // Add bookmark
                let displayed_location = end_loc / magic_mobi_constant + 1;
                annotations.insert(
                    end_loc,
                    KindleAnnotation {
                        id: id.to_string(),
                        displayed_location,
                        annotation_type: "Bookmark".to_string(),
                        text: None,
                    },
                );
            }

            reader.seek(SeekFrom::Start(eo + 4))?;
            let rec_len = reader.read_u32::<BigEndian>()? as u64;
            eo += rec_len + 8;
        }

        Ok(Bookmark {
            last_read_location,
            last_read_offset,
            timestamp,
            annotations,
        })
    }
}
