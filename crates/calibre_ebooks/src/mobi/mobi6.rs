use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

use crate::mobi::headers::BookHeader;

pub struct MobiReader {
    pub header: Vec<u8>,
    pub name: String,
    pub num_sections: u16,
    pub sections: Vec<(Vec<u8>, (u32, u8, u32))>, // (data, (offset, flags, val))
    pub book_header: BookHeader,
    pub kf8_type: Option<String>,
    pub kf8_boundary: Option<usize>,
}

impl MobiReader {
    pub fn new<R: Read + Seek>(mut reader: R) -> Result<Self> {
        let mut header = vec![0u8; 78];
        reader.read_exact(&mut header)?;

        // Remove null bytes from name
        let name_bytes = &header[0..32];
        let name = String::from_utf8_lossy(name_bytes)
            .trim_matches(char::from(0))
            .to_string();

        let mut cursor = std::io::Cursor::new(&header[76..78]);
        let num_sections = cursor.read_u16::<BigEndian>()?;

        let ident = &header[0x3C..0x3C + 8];
        let ident_str = String::from_utf8_lossy(ident).to_uppercase();

        if ident_str != "BOOKMOBI" && ident_str != "TEXTREAD" {
            bail!("Unknown book type: {}", ident_str);
        }

        let mut section_headers = Vec::new();
        for _ in 0..num_sections {
            let offset = reader.read_u32::<BigEndian>()?;
            let a1 = reader.read_u8()?;
            let a2 = reader.read_u8()?;
            let a3 = reader.read_u8()?;
            let a4 = reader.read_u8()?;
            let val = ((a2 as u32) << 16) | ((a3 as u32) << 8) | (a4 as u32);
            section_headers.push((offset, a1, val));
        }

        let mut sections = Vec::new();
        let file_len = reader.seek(SeekFrom::End(0))?;

        for i in 0..num_sections as usize {
            let (start_offset, flags, val) = section_headers[i];
            let end_offset = if i == (num_sections as usize) - 1 {
                file_len as u32
            } else {
                section_headers[i + 1].0
            };

            if start_offset > end_offset {
                bail!(
                    "Invalid section offset: start {} > end {}",
                    start_offset,
                    end_offset
                );
            }

            let len = end_offset - start_offset;
            reader.seek(SeekFrom::Start(start_offset as u64))?;
            let mut data = vec![0u8; len as usize];
            reader.read_exact(&mut data)?;
            sections.push((data, (start_offset, flags, val)));
        }

        if sections.is_empty() {
            bail!("No sections found in MOBI file");
        }

        // Parse BookHeader from first section (Record 0)
        let book_header = BookHeader::parse(&sections[0].0, None)?;

        // KF8 detection logic
        let mut kf8_type = None;
        let mut kf8_boundary = None;
        let mut final_book_header = book_header.clone();

        let k8i = if let Some(ref exth) = final_book_header.exth {
            exth.records
                .iter()
                .find(|(id, _)| *id == 121)
                .map(|(_, data)| {
                    let mut r = std::io::Cursor::new(data);
                    r.read_u32::<BigEndian>().unwrap_or(0xFFFFFFFF)
                })
        } else {
            None
        };

        let k8i_val = if let Some(val) = k8i {
            if val != 0xFFFFFFFF {
                Some(val as usize)
            } else {
                None
            }
        } else {
            None
        };

        if final_book_header.mobi_version == 8 {
            kf8_type = Some("standalone".to_string());
        } else if let Some(k_index) = k8i_val {
            if k_index > 0 && k_index < sections.len() {
                let raw_prev = &sections[k_index - 1].0;
                if raw_prev == b"BOUNDARY" {
                    let kf8_header_raw = &sections[k_index].0;
                    if let Ok(mut bh8) = BookHeader::parse(kf8_header_raw, None) {
                        bh8.first_image_index =
                            bh8.first_image_index.saturating_add(k_index as u32);

                        if let Some(ho) = bh8.huff_offset {
                            bh8.huff_offset = Some(ho + k_index as u32);
                        }

                        final_book_header = bh8;
                        kf8_type = Some("joint".to_string());
                        kf8_boundary = Some(k_index - 1);
                    }
                }
            }
        }

        Ok(MobiReader {
            header: header.to_vec(),
            name,
            num_sections,
            sections,
            book_header: final_book_header,
            kf8_type,
            kf8_boundary,
        })
    }

    pub fn extract_content(&self, output_dir: &std::path::Path) -> Result<()> {
        // TODO: Implement content extraction
        // 1. Check for DRM
        // 2. Extract text (lz77/palmdoc/huff/cdic)
        // 3. Process HTML (upshift markup, cleanup)
        // 4. Extract images
        // 5. Create OPF/NCX
        bail!("extract_content not implemented")
    }
}
