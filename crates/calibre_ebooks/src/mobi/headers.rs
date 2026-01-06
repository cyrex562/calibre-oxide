use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

/// The common header for all PDB-based files is handled loosely here or via `pdb` module.
/// Here we focus on the specific MOBI headers which follow the PDB header's Record 0.

pub const NULL_INDEX: u32 = 0xFFFFFFFF;

#[derive(Debug, Clone)]
pub struct PalmDocHeader {
    pub compression: u16,
    pub unused: u16,
    pub text_length: u32,
    pub record_count: u16,
    pub record_size: u16,
    pub encryption_type: u16,
    pub unknown: u16,
}

impl PalmDocHeader {
    pub fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let compression = reader.read_u16::<BigEndian>()?;
        let unused = reader.read_u16::<BigEndian>()?;
        let text_length = reader.read_u32::<BigEndian>()?;
        let record_count = reader.read_u16::<BigEndian>()?;
        let record_size = reader.read_u16::<BigEndian>()?;
        let encryption_type = reader.read_u16::<BigEndian>()?;
        let unknown = reader.read_u16::<BigEndian>()?;

        Ok(PalmDocHeader {
            compression,
            unused,
            text_length,
            record_count,
            record_size,
            encryption_type,
            unknown,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MobiHeader {
    pub identifier: String, // "MOBI"
    pub header_length: u32,
    pub mobi_type: u32,
    pub text_encoding: u32,
    pub unique_id: u32,
    pub file_version: u32,
    pub ortographic_index: u32,
    pub inflection_index: u32,
    pub index_names: u32,
    pub index_keys: u32,
    pub extra_index_0: u32,
    pub extra_index_1: u32,
    pub extra_index_2: u32,
    pub extra_index_3: u32,
    pub extra_index_4: u32,
    pub extra_index_5: u32,
    pub first_non_book_index: u32,
    pub full_name_offset: u32,
    pub full_name_length: u32,
    pub locale: u32,
    pub input_language: u32,
    pub output_language: u32,
    pub min_version: u32,
    pub first_image_index: u32,
    pub huffman_record_offset: u32,
    pub huffman_record_count: u32,
    pub huffman_table_offset: u32,
    pub huffman_table_length: u32,
    pub exth_flags: u32,
    // Add fields as needed for extraction...
    pub drm_offset: u32,
    pub drm_count: u32,
    pub drm_size: u32,
    pub drm_flags: u32,
    // KF8 specific fields
    pub ncx_index: u32,
    pub skel_index: u32,
    pub div_index: u32,
    pub fdst_index: u32,
    pub fdst_count: u32,
}

impl MobiHeader {
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        // Assume reader positioned start of MOBI header (after PalmDoc)
        let mut ident = [0u8; 4];
        reader.read_exact(&mut ident)?;
        let identifier = String::from_utf8_lossy(&ident).to_string();

        if identifier != "MOBI" {
            bail!("Invalid MOBI identifier: {}", identifier);
        }

        let header_length = reader.read_u32::<BigEndian>()?;
        let mobi_type = reader.read_u32::<BigEndian>()?;
        let text_encoding = reader.read_u32::<BigEndian>()?;
        let unique_id = reader.read_u32::<BigEndian>()?;
        let file_version = reader.read_u32::<BigEndian>()?;

        let ortographic_index = reader.read_u32::<BigEndian>()?;
        let inflection_index = reader.read_u32::<BigEndian>()?;
        let index_names = reader.read_u32::<BigEndian>()?;
        let index_keys = reader.read_u32::<BigEndian>()?;
        let extra_index_0 = reader.read_u32::<BigEndian>()?;
        let extra_index_1 = reader.read_u32::<BigEndian>()?;
        let extra_index_2 = reader.read_u32::<BigEndian>()?;
        let extra_index_3 = reader.read_u32::<BigEndian>()?;
        let extra_index_4 = reader.read_u32::<BigEndian>()?;
        let extra_index_5 = reader.read_u32::<BigEndian>()?;

        let first_non_book_index = reader.read_u32::<BigEndian>()?;
        let full_name_offset = reader.read_u32::<BigEndian>()?;
        let full_name_length = reader.read_u32::<BigEndian>()?;
        let locale = reader.read_u32::<BigEndian>()?;
        let input_language = reader.read_u32::<BigEndian>()?;
        let output_language = reader.read_u32::<BigEndian>()?;
        let min_version = reader.read_u32::<BigEndian>()?;
        let first_image_index = reader.read_u32::<BigEndian>()?;

        let huffman_record_offset = reader.read_u32::<BigEndian>()?;
        let huffman_record_count = reader.read_u32::<BigEndian>()?;
        let huffman_table_offset = reader.read_u32::<BigEndian>()?;
        let huffman_table_length = reader.read_u32::<BigEndian>()?;

        let exth_flags = reader.read_u32::<BigEndian>()?;

        // Skip some fields to get to DRM info (usually at offset 0xA8 from start of header if enough length)
        // Current read u32 count: 4 (ident) + 27 * 4 = 112 bytes read.
        // We need to read remaining known fields.
        // 32 bytes of reserved?
        reader.seek(SeekFrom::Current(32))?;

        let drm_offset = reader.read_u32::<BigEndian>()?;
        let drm_count = reader.read_u32::<BigEndian>()?;
        let drm_size = reader.read_u32::<BigEndian>()?;
        let drm_flags = reader.read_u32::<BigEndian>()?;

        // Current position: 160 (16 + 4*24 + 32 + 4*4)
        // ncx_index is at 0xF4 (244) relative to start (if we started at 0)
        // Only read if header_length is large enough

        let mut ncx_index = NULL_INDEX;
        let mut skel_index = NULL_INDEX;
        let mut div_index = NULL_INDEX;
        let mut fdst_index = NULL_INDEX;
        let mut fdst_count = 0;

        if header_length >= 248 {
            // Require enough bytes for ncx_index
            // Skip from 160 to 244: 84 bytes
            reader.seek(SeekFrom::Current(84))?;
            ncx_index = reader.read_u32::<BigEndian>()?;
        }

        if header_length >= 264 {
            // KF8 Header fields
            // Skip from 248 (244+4) to 264: 16 bytes
            reader.seek(SeekFrom::Current(16))?;
            skel_index = reader.read_u32::<BigEndian>()?;
            div_index = reader.read_u32::<BigEndian>()?;
            fdst_index = reader.read_u32::<BigEndian>()?;
            fdst_count = reader.read_u32::<BigEndian>()?;
        }

        // We assume we don't need to be perfectly at end of header for now

        Ok(MobiHeader {
            identifier,
            header_length,
            mobi_type,
            text_encoding,
            unique_id,
            file_version,
            ortographic_index,
            inflection_index,
            index_names,
            index_keys,
            extra_index_0,
            extra_index_1,
            extra_index_2,
            extra_index_3,
            extra_index_4,
            extra_index_5,
            first_non_book_index,
            full_name_offset,
            full_name_length,
            locale,
            input_language,
            output_language,
            min_version,
            first_image_index,
            huffman_record_offset,
            huffman_record_count,
            huffman_table_offset,
            huffman_table_length,
            exth_flags,
            drm_offset,
            drm_count,
            drm_size,
            drm_flags,
            ncx_index,
            skel_index,
            div_index,
            fdst_index,
            fdst_count,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExthHeader {
    pub records: Vec<(u32, Vec<u8>)>,
}

impl ExthHeader {
    pub fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let mut ident = [0u8; 4];
        reader.read_exact(&mut ident)?;
        if &ident != b"EXTH" {
            // Not an EXTH header
            bail!("EXTH header not found");
        }

        let _header_len = reader.read_u32::<BigEndian>()?;
        let record_count = reader.read_u32::<BigEndian>()?;

        let mut records = Vec::new();
        for _ in 0..record_count {
            let record_type = reader.read_u32::<BigEndian>()?;
            let record_len = reader.read_u32::<BigEndian>()?;
            if record_len < 8 {
                bail!("Invalid EXTH record length");
            }
            let data_len = record_len - 8;
            let mut data = vec![0u8; data_len as usize];
            reader.read_exact(&mut data)?;
            records.push((record_type, data));
        }

        Ok(ExthHeader { records })
    }
}

#[derive(Debug, Clone)]
pub struct BookHeader {
    pub palmdoc: PalmDocHeader,
    pub mobi: MobiHeader,
    pub exth: Option<ExthHeader>,
    pub title: String,
    pub codec: String,
    pub compression_type: u16,
    pub encryption_type: u16,
    pub first_image_index: u32,
    pub mobi_version: u32,
    pub huff_offset: Option<u32>,
    pub huff_number: Option<u32>,
}

impl BookHeader {
    pub fn parse(raw: &[u8], user_encoding: Option<&str>) -> Result<Self> {
        let mut reader = std::io::Cursor::new(raw);
        let palmdoc = PalmDocHeader::parse(&mut reader)?;

        let identifier_offset = 16;
        reader.seek(SeekFrom::Start(identifier_offset))?;
        // Removed identifier reading check here as MobiHeader::parse does it.

        reader.seek(SeekFrom::Start(identifier_offset))?;
        let mobi = MobiHeader::parse(&mut reader)?;

        let mut exth = None;
        if (mobi.exth_flags & 0x40) != 0 {
            // calculated offset: 16 (PalmDoc) + mobi.header_length
            let mobi_header_start = 16;
            let exth_start = mobi_header_start + mobi.header_length as u64;
            reader.seek(SeekFrom::Start(exth_start))?;

            if let Ok(e) = ExthHeader::parse(&mut reader) {
                exth = Some(e);
            }
        }

        let codec = if mobi.text_encoding == 65001 {
            "utf-8".to_string()
        } else {
            match mobi.text_encoding {
                1252 => "cp1252".to_string(),
                _ => user_encoding.unwrap_or("cp1252").to_string(),
            }
        };

        let title_start = mobi.full_name_offset as u64;
        let title_len = mobi.full_name_length as usize;
        let mut title = String::from("Unknown");

        if (title_start as usize) < raw.len() {
            reader.seek(SeekFrom::Start(title_start))?;
            let mut title_bytes = vec![0u8; title_len];
            if reader.read_exact(&mut title_bytes).is_ok() {
                if codec == "utf-8" {
                    title = String::from_utf8_lossy(&title_bytes).to_string();
                } else {
                    // Basic fallback for now
                    let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(&title_bytes);
                    title = cow.to_string();
                }
            }
        }

        let mut huff_offset = None;
        let mut huff_number = None;
        if palmdoc.compression == 17480 {
            // 'DH'
            huff_offset = Some(mobi.huffman_record_offset);
            huff_number = Some(mobi.huffman_record_count);
        }

        let compression_type = palmdoc.compression;
        let encryption_type = palmdoc.encryption_type;

        Ok(BookHeader {
            palmdoc,
            mobi: mobi.clone(),
            exth,
            title,
            codec,
            compression_type,
            encryption_type,
            first_image_index: mobi.first_image_index,
            mobi_version: mobi.file_version,
            huff_offset,
            huff_number,
        })
    }
}
