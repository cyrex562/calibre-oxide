//! Port of `calibre.ebooks.pdb.ereader.reader132`: reads eReader files
//! with a 132-byte header record 0, produced by Dropbook. PML-markup
//! text records (PalmDOC LZ77- or zlib-compressed), plus a footnote and
//! sidebar record scheme. No DRM support: a `compression` value of 260
//! or 272 means the payload is eReader-DRM'd, and `Reader132::new`
//! returns [`EreaderError::Drm`] (matching Python's `DRMError`) rather
//! than attempting to decode it.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use byteorder::{BigEndian, ByteOrder};
use encoding_rs::{Encoding, WINDOWS_1252};
use flate2::read::ZlibDecoder;
use regex::Regex;

use crate::compression::palmdoc;
use crate::input::pml_input::pml_to_html;
use crate::metadata::ereader::get_metadata as get_ereader_metadata;
use crate::metadata::toc::TOC;
use crate::metadata::MetaInformation;
use crate::mobi::opf_writer::{auto_manifest, write_ncx, write_opf};
use crate::pdb::ereader::EreaderError;
use crate::pdb::formatreader::FormatReader;
use crate::pdb::header::PdbHeader;

/// The first record in the file is always the header record. It holds
/// information related to the location of text, images, and so on in
/// the file. This is used in conjunction with the sections defined in
/// the PDB file header. Port of `reader132.py`'s `HeaderRecord`.
#[derive(Debug, Clone)]
pub struct HeaderRecord {
    pub compression: u16,
    pub non_text_offset: u16,
    pub chapter_count: u16,
    pub image_count: u16,
    pub link_count: u16,
    pub has_metadata: u16,
    pub footnote_count: u16,
    pub sidebar_count: u16,
    pub chapter_offset: u16,
    pub small_font_page_offset: u16,
    pub large_font_page_offset: u16,
    pub image_data_offset: u16,
    pub link_offset: u16,
    pub metadata_offset: u16,
    pub footnote_offset: u16,
    pub sidebar_offset: u16,
    pub last_data_offset: u16,
    /// `non_text_offset - 1`. Signed so a malformed (e.g. zero)
    /// `non_text_offset` produces a negative page count rather than
    /// panicking on unsigned underflow -- callers then treat any
    /// `num_text_pages <= 0` as "no text pages", matching Python's
    /// `range(1, num_text_pages + 1)` being empty for `num_text_pages <= 0`.
    pub num_text_pages: i64,
    /// `metadata_offset - image_data_offset`, same signed-underflow
    /// reasoning as `num_text_pages`.
    pub num_image_pages: i64,
}

fn u16_at(raw: &[u8], offset: usize) -> Result<u16> {
    let end = offset + 2;
    if raw.len() < end {
        anyhow::bail!("eReader header record truncated at offset {offset}");
    }
    Ok(BigEndian::read_u16(&raw[offset..end]))
}

impl HeaderRecord {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let compression = u16_at(raw, 0)?;
        let non_text_offset = u16_at(raw, 12)?;
        let chapter_count = u16_at(raw, 14)?;
        let image_count = u16_at(raw, 20)?;
        let link_count = u16_at(raw, 22)?;
        let has_metadata = u16_at(raw, 24)?;
        let footnote_count = u16_at(raw, 28)?;
        let sidebar_count = u16_at(raw, 30)?;
        let chapter_offset = u16_at(raw, 32)?;
        let small_font_page_offset = u16_at(raw, 36)?;
        let large_font_page_offset = u16_at(raw, 38)?;
        let image_data_offset = u16_at(raw, 40)?;
        let link_offset = u16_at(raw, 42)?;
        let metadata_offset = u16_at(raw, 44)?;
        let footnote_offset = u16_at(raw, 48)?;
        let sidebar_offset = u16_at(raw, 50)?;
        let last_data_offset = u16_at(raw, 52)?;

        let num_text_pages = non_text_offset as i64 - 1;
        let num_image_pages = metadata_offset as i64 - image_data_offset as i64;

        Ok(HeaderRecord {
            compression,
            non_text_offset,
            chapter_count,
            image_count,
            link_count,
            has_metadata,
            footnote_count,
            sidebar_count,
            chapter_offset,
            small_font_page_offset,
            large_font_page_offset,
            image_data_offset,
            link_offset,
            metadata_offset,
            footnote_offset,
            sidebar_offset,
            last_data_offset,
            num_text_pages,
            num_image_pages,
        })
    }
}

fn decode(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (cow, _, _) = encoding.decode(bytes);
    cow.into_owned()
}

fn trim_nulls(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

#[derive(Debug)]
pub struct Reader132 {
    sections: Vec<Vec<u8>>,
    pub header_record: HeaderRecord,
    pub mi: MetaInformation,
    encoding: &'static Encoding,
}

impl Reader132 {
    /// Port of `Reader132.__init__`. `header`/`stream` mirror the
    /// Python's `(header, stream, log, options)` constructor contract
    /// (see `crate::pdb::formatreader`'s module docs for why the
    /// constructor itself isn't part of the `FormatReader` trait);
    /// `encoding` mirrors `options.input_encoding` (`None` defaults to
    /// cp1252, exactly as the eReader spec mandates).
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

        if !matches!(header_record.compression, 2 | 10) {
            if matches!(header_record.compression, 260 | 272) {
                return Err(EreaderError::Drm("eReader DRM is not supported.".to_string()).into());
            }
            return Err(EreaderError::msg(format!(
                "Unknown book compression {}.",
                header_record.compression
            ))
            .into());
        }

        stream
            .seek(SeekFrom::Start(0))
            .context("seeking to read eReader metadata")?;
        let mi = get_ereader_metadata(&mut *stream).context("reading eReader metadata")?;

        Ok(Reader132 {
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

    fn decompress_text(&self, number: usize) -> Result<String> {
        let data = self.section_data(number)?;
        let decoded = match self.header_record.compression {
            2 => palmdoc::decompress(data)?,
            10 => {
                let mut decoder = ZlibDecoder::new(data);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .context("inflating zlib-compressed eReader text record")?;
                out
            }
            other => anyhow::bail!("Unknown book compression {other}."),
        };
        Ok(decode(&decoded, self.encoding))
    }

    /// Returns `("empty", &[])` for an out-of-range image index, matching
    /// Python's `get_image`.
    fn get_image(&self, number: usize) -> (String, Vec<u8>) {
        let lo = self.header_record.image_data_offset as i64;
        let hi = lo + self.header_record.num_image_pages - 1;
        let n = number as i64;
        if n < lo || n > hi {
            return ("empty".to_string(), Vec::new());
        }
        let Ok(data) = self.section_data(number) else {
            return ("empty".to_string(), Vec::new());
        };
        if data.len() < 62 {
            return (String::new(), Vec::new());
        }
        let name = decode(trim_nulls(&data[4..36]), self.encoding);
        let img = data[62..].to_vec();
        (name, img)
    }

    /// Only PalmDOC and zlib compression are supported. The text is
    /// assumed to be encoded as Windows-1252 (part of the eReader file
    /// spec). Returns `""` for an out-of-range page number, matching
    /// Python's `get_text_page`.
    pub fn get_text_page(&self, number: i64) -> Result<String> {
        if !(1 <= number && number <= self.header_record.num_text_pages) {
            return Ok(String::new());
        }
        self.decompress_text(number as usize)
    }

    /// Port of `dump_pml`: the concatenated raw PML markup, primarily
    /// for debugging/3rd-party tools.
    pub fn dump_pml(&self) -> Result<String> {
        let mut pml = String::new();
        for i in 1..=self.header_record.num_text_pages.max(0) {
            pml.push_str(&self.get_text_page(i)?);
        }
        Ok(pml)
    }

    /// Port of `dump_images`: write every embedded image into
    /// `output_dir`, primarily for debugging/3rd-party tools.
    pub fn dump_images(&self, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;
        for i in 0..self.header_record.num_image_pages.max(0) {
            let (name, img) =
                self.get_image((self.header_record.image_data_offset as i64 + i) as usize);
            std::fs::write(output_dir.join(&name), &img)?;
        }
        Ok(())
    }

    fn create_opf(
        &self,
        output_dir: &Path,
        images: &[String],
        toc: TOC,
    ) -> Result<std::path::PathBuf> {
        let mut mi = self.mi.clone();
        let cover_href = if images.iter().any(|i| i == "cover.png") {
            let href = "images/cover.png".to_string();
            mi.cover_id = Some(href.clone());
            Some(href)
        } else {
            None
        };

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

        let opf_xml = write_opf(
            &mi,
            &manifest,
            &["id1".to_string()],
            &[],
            Some("ncx"),
            cover_href.as_deref(),
            None,
            None,
        );
        let ncx_xml = write_ncx(&toc, mi.uuid.as_deref().unwrap_or("eReader"), &mi.title);

        std::fs::write(output_dir.join("metadata.opf"), opf_xml)?;
        std::fs::write(output_dir.join("toc.ncx"), ncx_xml)?;

        Ok(output_dir.join("metadata.opf"))
    }
}

impl FormatReader for Reader132 {
    fn extract_content(&self, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;

        let title = &self.mi.title;
        let mut html = format!("<html><head><title>{title}</title></head><body>");

        let mut pml = String::new();
        for i in 1..=self.header_record.num_text_pages.max(0) {
            pml.push_str(&self.get_text_page(i)?);
        }
        html.push_str(&pml_to_html(&pml));

        // Footnotes / sidebar. `pmlconverter.py`'s `footnote_to_html`/
        // `sidebar_to_html` add "return" anchors tied into the full
        // `PML_HTMLizer`'s id scheme; that whole converter isn't ported
        // (see `pml_to_html`'s docs), so this uses a narrower rendering:
        // each entry is still extracted and converted, just without the
        // cross-linked anchors.
        if self.header_record.footnote_count > 0 {
            html.push_str("<br /><h1>Footnotes</h1>");
            let ids = self.section_ids(self.header_record.footnote_offset as usize)?;
            let start = self.header_record.footnote_offset as i64 + 1;
            let end = self.header_record.footnote_offset as i64
                + self.header_record.footnote_count as i64;
            for (fid, i) in (start..end).enumerate() {
                let text = self.decompress_text(i as usize)?;
                let id = ids.get(fid).cloned().unwrap_or_default();
                html.push_str(&format!("<div id=\"fn-{id}\">{}</div>", pml_to_html(&text)));
            }
        }

        if self.header_record.sidebar_count > 0 {
            html.push_str("<br /><h1>Sidebar</h1>");
            let ids = self.section_ids(self.header_record.sidebar_offset as usize)?;
            let start = self.header_record.sidebar_offset as i64 + 1;
            let end =
                self.header_record.sidebar_offset as i64 + self.header_record.sidebar_count as i64;
            for (sid, i) in (start..end).enumerate() {
                let text = self.decompress_text(i as usize)?;
                let id = ids.get(sid).cloned().unwrap_or_default();
                html.push_str(&format!("<div id=\"sb-{id}\">{}</div>", pml_to_html(&text)));
            }
        }

        html.push_str("</body></html>");
        std::fs::write(output_dir.join("index.html"), html.as_bytes())?;

        let images_dir = output_dir.join("images");
        std::fs::create_dir_all(&images_dir)?;
        let mut images = Vec::new();
        for i in 0..self.header_record.num_image_pages.max(0) {
            let (name, img) =
                self.get_image((self.header_record.image_data_offset as i64 + i) as usize);
            images.push(name.clone());
            std::fs::write(images_dir.join(&name), &img)?;
        }

        self.create_opf(output_dir, &images, TOC::new())?;

        Ok(())
    }
}

impl Reader132 {
    /// Port of the `re.findall(r'\w+(?=\x00)', ...)` call in
    /// `extract_content` that pulls footnote/sidebar ids out of the id
    /// index record.
    fn section_ids(&self, offset: usize) -> Result<Vec<String>> {
        let data = self.section_data(offset)?;
        let text = decode(data, self.encoding);
        // Port of `re.findall(r'\w+(?=\x00)', ...)`. Rust's `regex` crate
        // has no look-ahead support, so this matches `\w+` and then
        // checks the following byte directly instead.
        let re = Regex::new(r"\w+").expect("static regex is valid");
        let bytes = text.as_bytes();
        Ok(re
            .find_iter(&text)
            .filter(|m| bytes.get(m.end()) == Some(&0))
            .map(|m| m.as_str().to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{ByteOrder, WriteBytesExt};
    use std::io::{Cursor, Write};

    /// Builds a minimal, spec-correct v1.32 eReader PDB: PDB header,
    /// then a 132-byte record 0, then one PalmDOC-compressed text
    /// record.
    fn build_ereader_132(compression: u16, text_records: &[Vec<u8>]) -> Vec<u8> {
        let mut record0 = vec![0u8; 132];
        BigEndian::write_u16(&mut record0[0..2], compression); // compression
        let non_text_offset = 1 + text_records.len() as u16;
        BigEndian::write_u16(&mut record0[12..14], non_text_offset);
        // metadata_offset == image_data_offset -> num_image_pages == 0
        BigEndian::write_u16(&mut record0[40..42], non_text_offset);
        BigEndian::write_u16(&mut record0[44..46], non_text_offset);

        let mut records = vec![record0];
        records.extend(text_records.iter().cloned());
        build_pdb(records)
    }

    fn build_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut name = b"Test Book".to_vec();
        name.resize(32, 0);
        buffer.extend_from_slice(&name);
        buffer.write_u16::<BigEndian>(0).unwrap(); // attributes
        buffer.write_u16::<BigEndian>(0).unwrap(); // version
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.extend_from_slice(b"PNRd");
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

    #[test]
    fn rejects_unknown_compression() {
        let raw = build_ereader_132(99, &[]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let err = Reader132::new(&header, &mut cursor, None).unwrap_err();
        assert!(
            err.to_string().contains("Unknown book compression"),
            "{err}"
        );
    }

    #[test]
    fn detects_drm_compression_codes() {
        for code in [260u16, 272] {
            let raw = build_ereader_132(code, &[]);
            let mut cursor = Cursor::new(raw);
            let header = PdbHeader::parse(&mut cursor).unwrap();
            let err = Reader132::new(&header, &mut cursor, None).unwrap_err();
            let drm = err.downcast_ref::<EreaderError>();
            assert!(matches!(drm, Some(EreaderError::Drm(_))), "{err}");
        }
    }

    #[test]
    fn reads_palmdoc_compressed_text_page() {
        let plain = b"Hello eReader world";
        let compressed = palmdoc::compress(plain).unwrap();
        let raw = build_ereader_132(2, &[compressed]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader132::new(&header, &mut cursor, None).unwrap();
        assert_eq!(reader.header_record.num_text_pages, 1);
        let page = reader.get_text_page(1).unwrap();
        assert_eq!(page, "Hello eReader world");
    }

    #[test]
    fn reads_zlib_compressed_text_page() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let plain = b"Zlib eReader text";
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(plain).unwrap();
        let compressed = enc.finish().unwrap();

        let raw = build_ereader_132(10, &[compressed]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader132::new(&header, &mut cursor, None).unwrap();
        let page = reader.get_text_page(1).unwrap();
        assert_eq!(page, "Zlib eReader text");
    }

    #[test]
    fn out_of_range_text_page_is_empty_not_an_error() {
        let raw = build_ereader_132(2, &[]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader132::new(&header, &mut cursor, None).unwrap();
        assert_eq!(reader.get_text_page(0).unwrap(), "");
        assert_eq!(reader.get_text_page(5).unwrap(), "");
    }

    #[test]
    fn extract_content_writes_index_html_and_opf() {
        let plain = b"Some \\bbold\\b text";
        let compressed = palmdoc::compress(plain).unwrap();
        let raw = build_ereader_132(2, &[compressed]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let reader = Reader132::new(&header, &mut cursor, None).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        reader.extract_content(tmp.path()).unwrap();

        let html = std::fs::read_to_string(tmp.path().join("index.html")).unwrap();
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(tmp.path().join("metadata.opf").exists());
        assert!(tmp.path().join("toc.ncx").exists());
    }
}
