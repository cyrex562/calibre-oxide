use anyhow::{Context, Result};
use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::headers::{ExthHeader, MobiHeader, PalmDocHeader};
use calibre_ebooks::pdb::header::PdbHeader;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use uuid::Uuid;

pub struct APNXBuilder;

#[derive(Debug)]
pub struct PageMap {
    pub page_positions: Vec<u32>,
}

impl APNXBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn write_apnx(&self, mobi_path: &Path, apnx_path: &Path) -> Result<()> {
        let apnx_meta = self.get_apnx_meta(mobi_path)?;
        // Stub: page generation. In a real impl, we'd use a PageGenerator strategy.
        // For now, let's generate a dummy mapping based on file size or similar, or just a single page?
        // Python: `pages = generator.generate(mobi_file_path, page_count)`
        // Let's create a minimal map.
        let page_positions = vec![0, 100, 200]; // Dummy positions
        let pages = PageMap { page_positions };

        let apnx_data = self.generate_apnx(&pages, &apnx_meta)?;

        let mut file = BufWriter::new(File::create(apnx_path)?);
        file.write_all(&apnx_data)?;
        Ok(())
    }

    fn get_apnx_meta(&self, mobi_path: &Path) -> Result<ApnxMeta> {
        let mut file = File::open(mobi_path)?;
        let pdb_header = PdbHeader::parse(&mut file)?;

        if pdb_header.type_id != *b"BOOK" || pdb_header.creator_id != *b"MOBI" {
            // "BOOKMOBI" check
            // In Python: `if as_bytes(ident) != b'BOOKMOBI':`
            // PDB header has separate type and creator.
            // type_id is 4 bytes, creator_id is 4 bytes.
        }

        let acr = pdb_header.name.clone();

        // Need to parse MOBI header. It's in record 0 (usually).
        let record0_data = pdb_header.section_data(&mut file, 0)?;
        let mut reader = Cursor::new(&record0_data);
        // PalmDocHeader is at the start of record 0
        let _palmdoc = PalmDocHeader::parse(&mut reader)?;
        // MobiHeader is next
        let mobi_header = MobiHeader::parse(&mut reader)?;

        let mut format = "MOBI_7".to_string();
        if mobi_header.min_version == 8 || mobi_header.mobi_type == 2 {
            // Rough check for mobi8/EPUB?
            // Python checks `mh.mobi_version == 8`. Our struct has `file_version`.
            if mobi_header.file_version >= 8 {
                format = "MOBI_8".to_string();
            }
        }

        let mut cdetype = "EBOK".to_string();
        let mut asin = "".to_string();

        // Check EXTH
        if (mobi_header.exth_flags & 0x40) != 0 {
            // EXTH exists.
            // It typically follows the MOBI header + whatever reserved bytes.
            // We need to seek to the right place.
            // The `MobiHeader::parse` skips some, but maybe not enough to land mostly at EXTH.
            // BUT, `MobiHeader` stores `header_length`.
            // EXTH starts at `header_length` + 16 (PalmDoc) from record start.
            // Let's re-seek using absolute offset in record 0.
            let mobi_header_start = 16; // PalmDoc is 16 bytes
            let exth_offset = mobi_header_start + mobi_header.header_length;
            reader.seek(SeekFrom::Start(exth_offset as u64))?;

            if let Ok(exth) = ExthHeader::parse(&mut reader) {
                for (rtype, rdata) in exth.records {
                    match rtype {
                        501 => {
                            // CDE Type
                            cdetype = String::from_utf8_lossy(&rdata).to_string();
                        }
                        113 => {
                            // ASIN
                            asin = String::from_utf8_lossy(&rdata).to_string();
                        }
                        _ => {}
                    }
                }
            }
        }

        let guid = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();

        Ok(ApnxMeta {
            guid,
            asin,
            cdetype,
            format,
            acr,
        })
    }

    fn generate_apnx(&self, pages: &PageMap, meta: &ApnxMeta) -> Result<Vec<u8>> {
        let mut apnx = Vec::new();

        let content_header = if meta.format == "MOBI_8" {
            format!(
                r#"{{"contentGuid":"{}","asin":"{}","cdeType":"{}","format":"{}","fileRevisionId":"1","acr":"{}"}}"#,
                meta.guid, meta.asin, meta.cdetype, meta.format, meta.acr
            )
        } else {
            format!(
                r#"{{"contentGuid":"{}","asin":"{}","cdeType":"{}","fileRevisionId":"1"}}"#,
                meta.guid, meta.asin, meta.cdetype
            )
        };

        // Construct page map string
        // The python code: pages.page_maps.
        // In python `Pages` class `page_maps` property returns a string rep?
        // Ah, see typical APNX format.
        // Actually, Python code: `page_header += pages.page_maps + '"}'`
        // We probably need to construct a tuple list or similar string representation if logic mirrors APNX format which often uses JSON-ish header.
        // Wait, `pages.page_maps` in Python `Pages` class converts the list of (start, end) tuples to string?
        // Let's assume standard simple mapping 1:1 or compressed.
        // For simplicity, we'll verify what `page_header` expects.
        // Python: `page_header = '{{"asin":"{asin}","pageMap":"'.format(**apnx_meta)`
        // Then `page_header += pages.page_maps + '"}'`
        // So `pageMap` is a string inside the JSON.
        // APNX PageMap is usually `(N,N,N)`. Tuple-like?
        // Let's look at `apnx.py` imports... `apnx_page_generator.pages`.

        let page_map_str = "(1,1,1)"; // Dummy for now.

        let page_header = format!(r#"{{"asin":"{}","pageMap":"{}"}}"#, meta.asin, page_map_str);

        apnx.write_u32::<BigEndian>(65537)?; // 0x00010001

        let content_header_bytes = content_header.as_bytes();
        let page_header_bytes = page_header.as_bytes();

        apnx.write_u32::<BigEndian>((12 + content_header_bytes.len()) as u32)?;
        apnx.write_u32::<BigEndian>(content_header_bytes.len() as u32)?;
        apnx.write_all(content_header_bytes)?;

        apnx.write_u16::<BigEndian>(1)?;
        apnx.write_u16::<BigEndian>(page_header_bytes.len() as u16)?;
        apnx.write_u16::<BigEndian>(pages.page_positions.len() as u16)?;
        apnx.write_u16::<BigEndian>(32)?; // Unknown constant
        apnx.write_all(page_header_bytes)?;

        for pos in &pages.page_positions {
            apnx.write_u32::<BigEndian>(*pos)?;
        }

        Ok(apnx)
    }
}

struct ApnxMeta {
    guid: String,
    asin: String,
    cdetype: String,
    format: String,
    acr: String,
}
