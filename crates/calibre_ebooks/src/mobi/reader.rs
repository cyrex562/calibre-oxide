use crate::compression::palmdoc::decompress;
use crate::mobi::headers::{ExthHeader, MobiHeader, PalmDocHeader};
use crate::pdb::header::PdbHeader;
use anyhow::{Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use encoding_rs::WINDOWS_1252;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct MobiSection {
    pub palmdoc: PalmDocHeader,
    pub mobi: MobiHeader,
    pub exth: Option<ExthHeader>,
    pub start_record: usize, // Index in PDB records
}

impl MobiSection {
    pub fn parse<R: Read + Seek>(
        reader: &mut R,
        pdb_header: &PdbHeader,
        record_index: usize,
    ) -> Result<Self> {
        let record = &pdb_header.records[record_index];
        reader.seek(SeekFrom::Start(record.offset as u64))?;

        let palmdoc = PalmDocHeader::parse(reader).context("Failed to parse PalmDoc Header")?;
        let mobi = MobiHeader::parse(reader).context("Failed to parse MOBI Header")?;

        let mut exth = None;
        if (mobi.exth_flags & 0x40) != 0 {
            let mobi_end = record.offset as u64 + 16 + mobi.header_length as u64;
            reader.seek(SeekFrom::Start(mobi_end))?;
            if let Ok(h) = ExthHeader::parse(reader) {
                exth = Some(h);
            }
        }

        Ok(MobiSection {
            palmdoc,
            mobi,
            exth,
            start_record: record_index,
        })
    }

    pub fn extract_text(&self, path: &Path, pdb_header: &PdbHeader) -> Result<String> {
        let mut f = File::open(path)?;
        let mut content_data = Vec::new();

        // Text records start after header.
        // For primary section (Rec 0), text starts at Rec 1.
        // For KF8 section, text starts at `start_record + 1`?
        // Yes, typically the header record is immediately followed by text records.
        let fst_txt_rec = self.start_record + 1;
        let lst_txt_rec = fst_txt_rec + self.palmdoc.record_count as usize;

        for idx in fst_txt_rec..lst_txt_rec {
            if idx >= pdb_header.records.len() {
                break;
            }

            let record_info = &pdb_header.records[idx];
            let offset = record_info.offset as u64;

            let next_offset = if idx + 1 < pdb_header.records.len() {
                pdb_header.records[idx + 1].offset as u64
            } else {
                f.metadata()?.len()
            };

            let len = next_offset - offset;
            let mut buf = vec![0u8; len as usize];
            f.seek(SeekFrom::Start(offset))?;
            f.read_exact(&mut buf)?;

            let decompressed = match self.palmdoc.compression {
                2 => decompress(&buf)?,
                1 => buf,
                17480 => {
                    // HUFF/CDIC
                    // Not implemented yet, return raw or empty?
                    // Warn and return raw for debugging
                    eprintln!("Warning: HUFF/CDIC compression not supported yet.");
                    buf
                }
                _ => anyhow::bail!("Unsupported compression type: {}", self.palmdoc.compression),
            };

            content_data.extend_from_slice(&decompressed);
        }

        let text = match self.mobi.text_encoding {
            65001 => String::from_utf8_lossy(&content_data).to_string(),
            1252 => {
                let (cow, _, _) = WINDOWS_1252.decode(&content_data);
                cow.into_owned()
            }
            _ => String::from_utf8_lossy(&content_data).to_string(),
        };

        Ok(text)
    }
}

pub struct MobiReader {
    pub pdb_header: PdbHeader,
    pub sections: Vec<MobiSection>,
    path: PathBuf,
}

impl MobiReader {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        let f = File::open(&path_buf)?;
        let mut reader = BufReader::new(f);

        let pdb_header = PdbHeader::parse(&mut reader).context("Failed to parse PDB Header")?;

        if pdb_header.records.is_empty() {
            anyhow::bail!("PDB has no records");
        }

        let mut sections = Vec::new();

        // 1. Parse Primary Section (Record 0)
        let section0 = MobiSection::parse(&mut reader, &pdb_header, 0)?;

        // 2. Check for KF8 Boundary
        let mut kf8_boundary = None;
        if let Some(exth) = &section0.exth {
            for (tag, data) in &exth.records {
                if *tag == 121 {
                    // KF8 Boundary Offset
                    // Data is u32
                    if data.len() >= 4 {
                        let mut curs = Cursor::new(data);
                        if let Ok(val) = curs.read_u32::<BigEndian>() {
                            // "The KF8 boundary is 0xffffffff if irrelevant..."
                            // Typically it points to the record index of the KF8 header?
                            // Or PDB record index?
                            // Python: getattr(self.book_header.exth, 'kf8_header', None)
                            // It usually stores the record index.
                            if val != 0xFFFFFFFF {
                                kf8_boundary = Some(val as usize);
                            }
                        }
                    }
                }
            }
        }

        sections.push(section0);

        if let Some(kf8_idx) = kf8_boundary {
            if kf8_idx < pdb_header.records.len() {
                // Parse KF8 Section
                // Is it guaranteed to be at kf8_idx?
                if let Ok(section_kf8) = MobiSection::parse(&mut reader, &pdb_header, kf8_idx) {
                    sections.push(section_kf8);
                } else {
                    eprintln!("Failed to parse KF8 section at index {}", kf8_idx);
                }
            }
        }

        Ok(MobiReader {
            pdb_header,
            sections,
            path: path_buf,
        })
    }
}
