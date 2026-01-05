use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

// SNB Format Constants
pub const MAGIC: &[u8] = b"SNBP000B";
const REV80: u32 = 0x00008000;
const REVZ1: u32 = 0x00000000;

#[derive(Debug)]
pub struct ParseHeader {
    pub file_count: u32,
    pub vfat_size: u32,
    pub vfat_compressed: bool,
    pub bin_stream_size: u32,
    pub plain_stream_uncompressed: bool,
}

#[derive(Debug, Clone)]
pub struct SnbFileEntry {
    pub attr: u32,
    pub file_name_offset: u32,
    pub file_size: u32,
    pub file_name: String,

    // Filled during tail parsing
    pub block_index: i32,
    pub content_offset: i32,

    pub file_body: Vec<u8>,
}

pub struct SnbReader<R: Read + Seek> {
    stream: R,
    pub header: Option<ParseHeader>,
    pub files: Vec<SnbFileEntry>,
}

impl<R: Read + Seek> SnbReader<R> {
    pub fn new(stream: R) -> Result<Self> {
        Ok(Self {
            stream,
            header: None,
            files: Vec::new(),
        })
    }

    pub fn parse(&mut self) -> Result<()> {
        let mut magic = [0u8; 8];
        self.stream.read_exact(&mut magic)?;
        if magic != MAGIC {
            bail!("Invalid SNB Magic");
        }

        let rev80 = self.stream.read_u32::<BigEndian>()?;
        let _reva3 = self.stream.read_u32::<BigEndian>()?;
        let revz1 = self.stream.read_u32::<BigEndian>()?;

        if rev80 != REV80 || revz1 != REVZ1 {
            bail!("Invalid SNB Revisions");
        }

        let file_count = self.stream.read_u32::<BigEndian>()?;
        let vfat_size = self.stream.read_u32::<BigEndian>()?;
        let vfat_compressed = self.stream.read_u32::<BigEndian>()?;
        let bin_stream_size = self.stream.read_u32::<BigEndian>()?;
        let plain_stream_uncompressed = self.stream.read_u32::<BigEndian>()?;
        let _revz2 = self.stream.read_u32::<BigEndian>()?;

        let header = ParseHeader {
            file_count,
            vfat_size,
            vfat_compressed: vfat_compressed != 0,
            bin_stream_size,
            plain_stream_uncompressed: plain_stream_uncompressed != 0,
        };

        self.header = Some(header);

        // Read VFAT
        let mut vfat_buf_comp = vec![0u8; vfat_compressed as usize];
        self.stream.read_exact(&mut vfat_buf_comp)?;

        let mut vfat_decoder = flate2::read::ZlibDecoder::new(&vfat_buf_comp[..]);
        let mut vfat_data = Vec::new();
        vfat_decoder.read_to_end(&mut vfat_data)?;

        // Parse Files from VFAT
        let file_names_offset = (file_count as usize) * 12; // 3 ints per file

        let mut cursor = Cursor::new(&vfat_data);
        for i in 0..file_count {
            cursor.set_position((i * 12) as u64);
            let attr = cursor.read_u32::<BigEndian>()?;
            let name_offset = cursor.read_u32::<BigEndian>()?;
            let size = cursor.read_u32::<BigEndian>()?;

            self.files.push(SnbFileEntry {
                attr,
                file_name_offset: name_offset,
                file_size: size,
                file_name: String::new(),
                block_index: 0,
                content_offset: 0,
                file_body: Vec::new(),
            });
        }

        // Parse Filenames
        let names_block = &vfat_data[file_names_offset..];
        let names: Vec<&[u8]> = names_block.split(|&b| b == 0).collect();
        for (i, file) in self.files.iter_mut().enumerate() {
            if i < names.len() {
                file.file_name = String::from_utf8_lossy(names[i]).to_string();
            }
        }

        // Tail logic
        self.stream.seek(SeekFrom::End(-16))?;
        let tail_size = self.stream.read_u32::<BigEndian>()?;
        let tail_offset = self.stream.read_u32::<BigEndian>()?;
        let mut tail_magic = [0u8; 8];
        self.stream.read_exact(&mut tail_magic)?;
        if tail_magic != MAGIC {
            bail!("Invalid Tail Magic");
        }

        self.stream.seek(SeekFrom::Start(tail_offset as u64))?;
        let mut tail_comp = vec![0u8; tail_size as usize];
        self.stream.read_exact(&mut tail_comp)?;
        let mut tail_dec = flate2::read::ZlibDecoder::new(&tail_comp[..]);
        let mut tail_data = Vec::new();
        tail_dec.read_to_end(&mut tail_data)?;

        let bin_block_count = (bin_stream_size + 0x8000 - 1) / 0x8000;
        let plain_block_count = (plain_stream_uncompressed + 0x8000 - 1) / 0x8000;
        let total_blocks = bin_block_count + plain_block_count;

        let mut blocks = Vec::new();
        let mut tail_cursor = Cursor::new(&tail_data);

        for _ in 0..total_blocks {
            let off = tail_cursor.read_u32::<BigEndian>()?;
            blocks.push(off);
        }

        for file in self.files.iter_mut() {
            file.block_index = tail_cursor.read_i32::<BigEndian>()?;
            file.content_offset = tail_cursor.read_i32::<BigEndian>()?;
        }

        // Extract plain stream content
        let mut uncompressed_plain = Vec::new();
        for i in 0..plain_block_count {
            let idx = (bin_block_count + i) as usize;
            let start_off = blocks[idx];
            let end_off = if idx + 1 < blocks.len() {
                blocks[idx + 1]
            } else {
                tail_offset
            };
            let len = end_off - start_off;

            self.stream.seek(SeekFrom::Start(start_off as u64))?;
            let mut chunk = vec![0u8; len as usize];
            self.stream.read_exact(&mut chunk)?;

            if chunk.len() < 32768 {
                let mut dec = bzip2::read::BzDecoder::new(&chunk[..]);
                match dec.read_to_end(&mut uncompressed_plain) {
                    Ok(_) => {}
                    Err(_) => {
                        uncompressed_plain.extend_from_slice(&chunk);
                    }
                }
            } else {
                uncompressed_plain.extend_from_slice(&chunk);
            }
        }

        // Assign bodies
        let mut plain_pos = 0;
        let mut bin_pos = 0;
        let start_bin = 44 + vfat_compressed;

        for file in self.files.iter_mut() {
            if (file.attr & 0x41000000) == 0x41000000 {
                // Plain
                let end = plain_pos + file.file_size as usize;
                if end <= uncompressed_plain.len() {
                    file.file_body = uncompressed_plain[plain_pos..end].to_vec();
                }
                plain_pos += file.file_size as usize;
            } else if (file.attr & 0x01000000) == 0x01000000 {
                // Binary
                self.stream
                    .seek(SeekFrom::Start((start_bin + bin_pos) as u64))?;
                let mut buf = vec![0u8; file.file_size as usize];
                self.stream.read_exact(&mut buf)?;
                file.file_body = buf;
                bin_pos += file.file_size;
            }
        }

        Ok(())
    }

    pub fn get_file(&self, name: &str) -> Option<Vec<u8>> {
        for f in &self.files {
            if f.file_name == name {
                return Some(f.file_body.clone());
            }
        }
        None
    }
}
