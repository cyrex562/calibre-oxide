use crate::compression::palmdoc::decompress;
use crate::mobi::headers::{ExthHeader, MobiHeader, PalmDocHeader};
use crate::mobi::huffcdic::HuffReader;
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
    pub start_record: usize,
    pub huff_reader: Option<HuffReader>,
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

        // Load Huff/CDIC if compression is 17480
        let mut huff_reader = None;
        if palmdoc.compression == 17480 {
            let huff_offset = mobi.huffman_record_offset;
            let huff_count = mobi.huffman_record_count;

            if huff_count > 0 {
                let mut huff_records = Vec::new();
                for i in 0..huff_count {
                    let rec_idx = (huff_offset + i) as usize;
                    if rec_idx < pdb_header.records.len() {
                        let rec = &pdb_header.records[rec_idx];
                        reader.seek(SeekFrom::Start(rec.offset as u64))?;
                        // How much to read? Next record offset - this record offset
                        let next_offset = if rec_idx + 1 < pdb_header.records.len() {
                            pdb_header.records[rec_idx + 1].offset as u64
                        } else {
                            // Assuming EOF or last record
                            // We don't easily know file size here without querying metadata, which we can do if we pass File, but we pass Read+Seek
                            // However, usually we can just read enough? Or seek end.
                            reader.seek(SeekFrom::End(0))?
                        };
                        let len = next_offset - rec.offset as u64;
                        reader.seek(SeekFrom::Start(rec.offset as u64))?;

                        let mut buf = vec![0u8; len as usize];
                        reader.read_exact(&mut buf)?;
                        huff_records.push(buf);
                    }
                }

                if !huff_records.is_empty() {
                    match HuffReader::new(&huff_records) {
                        Ok(hr) => huff_reader = Some(hr),
                        Err(e) => eprintln!("Failed to init HuffReader: {}", e),
                    }
                }
            }
        }

        Ok(MobiSection {
            palmdoc,
            mobi,
            exth,
            start_record: record_index,
            huff_reader,
        })
    }

    pub fn extract_text(&mut self, path: &Path, pdb_header: &PdbHeader) -> Result<String> {
        let mut f = File::open(path)?;
        let mut content_data = Vec::new();

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
                    if let Some(hr) = &mut self.huff_reader {
                        hr.unpack(&buf)?
                    } else {
                        // Warn and return raw for debugging
                        eprintln!(
                            "Warning: HUFF/CDIC compression required but reader not initialized."
                        );
                        buf
                    }
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
