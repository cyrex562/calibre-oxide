//! Port of `calibre.ebooks.pdb.ereader.reader202`: reads eReader files
//! with a 116- or 202-byte header record 0, produced by Makebook.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use byteorder::{BigEndian, ByteOrder};
use encoding_rs::{Encoding, WINDOWS_1252};

use crate::compression::palmdoc;
use crate::metadata::ereader::get_metadata as get_ereader_metadata;
use crate::metadata::MetaInformation;
use crate::opf_writer::{auto_manifest, write_opf};
use crate::pdb::ereader::EreaderError;
use crate::pdb::formatreader::FormatReader;
use crate::pdb::header::PdbHeader;
use crate::pml::pmlconverter::pml_to_html;

fn u16_at(raw: &[u8], offset: usize) -> Result<u16> {
    let end = offset + 2;
    if raw.len() < end {
        anyhow::bail!("eReader header record truncated at offset {offset}");
    }
    Ok(BigEndian::read_u16(&raw[offset..end]))
}

fn decode(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (cow, _, _) = encoding.decode(bytes);
    cow.into_owned()
}

fn trim_nulls(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// The first record in the file is always the header record. Port of
/// `reader202.py`'s `HeaderRecord`.
#[derive(Debug, Clone)]
pub struct HeaderRecord {
    pub version: u16,
    pub non_text_offset: u16,
    /// `non_text_offset - 1`, signed for the same reason as
    /// `reader132::HeaderRecord::num_text_pages`.
    pub num_text_pages: i64,
}

impl HeaderRecord {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let version = u16_at(raw, 0)?;
        let non_text_offset = u16_at(raw, 8)?;
        let num_text_pages = non_text_offset as i64 - 1;
        Ok(HeaderRecord {
            version,
            non_text_offset,
            num_text_pages,
        })
    }
}

#[derive(Debug)]
pub struct Reader202 {
    sections: Vec<Vec<u8>>,
    pub header_record: HeaderRecord,
    pub mi: MetaInformation,
    encoding: &'static Encoding,
}

impl Reader202 {
    /// Port of `Reader202.__init__`; see [`super::reader132::Reader132::new`]'s
    /// docs for why `log`/full `options` aren't part of the signature.
    pub fn new<R: Read + Seek>(
        header: &PdbHeader,
        stream: &mut R,
        encoding: Option<&'static Encoding>,
    ) -> Result<Self> {
        let mut sections = Vec::with_capacity(header.records.len());
        for i in 0..header.records.len() {
            sections.push(header.section_data(stream, i)?);
        }

        let record0 = sections.first().cloned().unwrap_or_default();
        let header_record = HeaderRecord::parse(&record0)?;

        if !matches!(header_record.version, 2 | 4) {
            return Err(EreaderError::msg(format!(
                "Unknown book version {}.",
                header_record.version
            ))
            .into());
        }

        stream
            .seek(SeekFrom::Start(0))
            .context("seeking to read eReader metadata")?;
        let mi = get_ereader_metadata(&mut *stream).context("reading eReader metadata")?;

        Ok(Reader202 {
            sections,
            header_record,
            mi,
            encoding: encoding.unwrap_or(WINDOWS_1252),
        })
    }

    fn section_data(&self, number: usize) -> Result<&[u8]> {
        self.sections
            .get(number)
            .map(|v| v.as_slice())
            .ok_or_else(|| anyhow::anyhow!("eReader record {number} out of bounds"))
    }

    /// Only PalmDOC compression is supported. The text is XORed with
    /// 0xA5 and assumed to be encoded as Windows-1252 (part of the
    /// eReader file spec).
    fn decompress_text(&self, number: usize) -> Result<String> {
        let data = self.section_data(number)?;
        let xored: Vec<u8> = data.iter().map(|b| b ^ 0xA5).collect();
        let decoded = palmdoc::decompress(&xored)?;
        Ok(decode(&decoded, self.encoding))
    }

    /// Port of `get_image`: returns `(None, None)` (here, `None`) unless
    /// the record starts with the `PNG` magic (note: three bytes, no
    /// trailing space -- that's `reader132`'s image record layout, not
    /// this one).
    fn get_image(&self, number: usize) -> Option<(String, Vec<u8>)> {
        let data = self.section_data(number).ok()?;
        if !data.starts_with(b"PNG") {
            return None;
        }
        if data.len() < 62 {
            return None;
        }
        let name = decode(trim_nulls(&data[4..36]), self.encoding);
        let img = data[62..].to_vec();
        Some((name, img))
    }

    /// Returns `""` for an out-of-range page number, matching Python's
    /// `get_text_page`.
    pub fn get_text_page(&self, number: i64) -> Result<String> {
        if !(1 <= number && number <= self.header_record.num_text_pages) {
            return Ok(String::new());
        }
        self.decompress_text(number as usize)
    }

    /// Port of `dump_pml`.
    pub fn dump_pml(&self) -> Result<String> {
        let mut pml = String::new();
        for i in 1..=self.header_record.num_text_pages.max(0) {
            pml.push_str(&self.get_text_page(i)?);
        }
        Ok(pml)
    }

    /// Port of `dump_images`.
    ///
    /// Python's `Reader202.dump_images` reads
    /// `self.header_record.image_data_offset`/`.num_image_pages`, but
    /// `reader202.py`'s `HeaderRecord` (unlike `reader132.py`'s) never
    /// defines those attributes -- calling it on a real 202-byte-header
    /// book would raise `AttributeError` in the Python. That's a latent
    /// upstream bug in a rarely-exercised debug helper, not a documented
    /// behavior worth reproducing; this instead reuses the same
    /// "non-text records are the image records" scan `extract_content`
    /// already performs correctly.
    pub fn dump_images(&self, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;
        for i in (self.header_record.non_text_offset as usize)..self.sections.len() {
            if let Some((name, img)) = self.get_image(i) {
                std::fs::write(output_dir.join(&name), &img)?;
            }
        }
        Ok(())
    }

    fn create_opf(&self, output_dir: &Path, images: &[String]) -> Result<std::path::PathBuf> {
        let mi = self.mi.clone();

        let mut manifest_items: Vec<(String, String)> = vec![(
            "index.html".to_string(),
            "application/xhtml+xml".to_string(),
        )];
        for name in images {
            let ext = Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png");
            let media_type = mime_guess::from_ext(ext)
                .first()
                .map(|m| m.essence_str().to_string())
                .unwrap_or_else(|| "image/png".to_string());
            manifest_items.push((format!("images/{name}"), media_type));
        }
        let manifest = auto_manifest(&manifest_items);

        // `Reader202.create_opf` in Python does not call `opf.set_toc` --
        // no NCX / table of contents is written for this record layout.
        let opf_xml = write_opf(
            &mi,
            &manifest,
            &["id1".to_string()],
            &[],
            None,
            None,
            None,
            None,
        );
        std::fs::write(output_dir.join("metadata.opf"), opf_xml)?;

        Ok(output_dir.join("metadata.opf"))
    }
}

impl FormatReader for Reader202 {
    fn extract_content(&self, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;

        let mut pml = String::new();
        for i in 1..=self.header_record.num_text_pages.max(0) {
            pml.push_str(&self.get_text_page(i)?);
        }

        let title = &self.mi.title;
        let html = format!(
            "<html><head><title>{title}</title></head><body>{}</body></html>",
            pml_to_html(&pml)
        );
        std::fs::write(output_dir.join("index.html"), html.as_bytes())?;

        let images_dir = output_dir.join("images");
        std::fs::create_dir_all(&images_dir)?;
        let mut images = Vec::new();
        for i in (self.header_record.non_text_offset as usize)..self.sections.len() {
            if let Some((name, img)) = self.get_image(i) {
                images.push(name.clone());
                std::fs::write(images_dir.join(&name), &img)?;
            }
        }

        self.create_opf(output_dir, &images)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{ByteOrder, WriteBytesExt};
    use std::io::{Cursor, Write};

    fn build_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut name = b"Test Book".to_vec();
        name.resize(32, 0);
        buffer.extend_from_slice(&name);
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.extend_from_slice(b"PNPd");
        buffer.extend_from_slice(b"PPrs");
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(records.len() as u16).unwrap();

        let base_offset = 78 + (records.len() as u32 * 8) + 2;
        let mut offset = base_offset;
        for r in &records {
            buffer.write_u32::<BigEndian>(offset).unwrap();
            buffer.write_all(&[0u8, 0, 0, 0]).unwrap();
            offset += r.len() as u32;
        }
        buffer.write_u16::<BigEndian>(0).unwrap();

        for r in &records {
            buffer.extend_from_slice(r);
        }
        buffer
    }

    fn build_ereader_202(version: u16, text_records: &[Vec<u8>], trailing: &[Vec<u8>]) -> Vec<u8> {
        let mut record0 = vec![0u8; 202];
        BigEndian::write_u16(&mut record0[0..2], version);
        let non_text_offset = 1 + text_records.len() as u16;
        BigEndian::write_u16(&mut record0[8..10], non_text_offset);

        let mut records = vec![record0];
        records.extend(text_records.iter().cloned());
        records.extend(trailing.iter().cloned());
        build_pdb(records)
    }

    fn xor_a5(data: &[u8]) -> Vec<u8> {
        data.iter().map(|b| b ^ 0xA5).collect()
    }

    #[test]
    fn rejects_unknown_version() {
        let raw = build_ereader_202(99, &[], &[]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let err = Reader202::new(&header, &mut cursor, None).unwrap_err();
        assert!(err.to_string().contains("Unknown book version"), "{err}");
    }

    #[test]
    fn reads_palmdoc_compressed_xored_text_page() {
        let plain = b"Hello Makebook world";
        let compressed = palmdoc::compress(plain).unwrap();
        let xored = xor_a5(&compressed);
        let raw = build_ereader_202(2, &[xored], &[]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader202::new(&header, &mut cursor, None).unwrap();
        assert_eq!(reader.header_record.num_text_pages, 1);
        let page = reader.get_text_page(1).unwrap();
        assert_eq!(page, "Hello Makebook world");
    }

    #[test]
    fn out_of_range_text_page_is_empty_not_an_error() {
        let raw = build_ereader_202(2, &[], &[]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader202::new(&header, &mut cursor, None).unwrap();
        assert_eq!(reader.get_text_page(0).unwrap(), "");
        assert_eq!(reader.get_text_page(9).unwrap(), "");
    }

    #[test]
    fn extract_content_extracts_trailing_png_image() {
        let plain = b"Some text";
        let compressed = palmdoc::compress(plain).unwrap();
        let xored = xor_a5(&compressed);

        let mut image_record = b"PNG ".to_vec();
        let mut name = b"pic.png".to_vec();
        name.resize(32, 0);
        image_record.extend_from_slice(&name);
        image_record.resize(62, 0);
        image_record.extend_from_slice(b"fakepngbytes");

        let raw = build_ereader_202(2, &[xored], &[image_record]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader202::new(&header, &mut cursor, None).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        reader.extract_content(tmp.path()).unwrap();

        assert!(tmp.path().join("index.html").exists());
        assert!(tmp.path().join("metadata.opf").exists());
        let img = std::fs::read(tmp.path().join("images").join("pic.png")).unwrap();
        assert_eq!(img, b"fakepngbytes");
    }
}
