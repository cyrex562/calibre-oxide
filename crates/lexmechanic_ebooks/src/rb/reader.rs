//! Port of `calibre.ebooks.rb.reader.Reader`.

use crate::metadata::rb::get_metadata;
use crate::metadata::MetaInformation;
use crate::rb::header::{RbHeader, RbTocEntry};
use anyhow::{Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use encoding_rs::{Encoding, WINDOWS_1252};
use flate2::read::ZlibDecoder;
use std::io::{Read, Seek, SeekFrom};

/// `(toc entry name, decoded text)` pairs, in TOC order.
pub type RbPages = Vec<(String, String)>;
/// `(toc entry name, raw bytes)` pairs, in TOC order.
pub type RbImages = Vec<(String, Vec<u8>)>;

pub struct RbReader<R> {
    reader: R,
    #[allow(dead_code)]
    header: RbHeader,
    /// Port of `Reader.mi` (`get_metadata(self.stream)`, ported for
    /// real at `crate::metadata::rb::get_metadata`).
    pub mi: MetaInformation,
    /// Port of `Reader.toc`.
    pub toc: Vec<RbTocEntry>,
    encoding: &'static Encoding,
}

impl<R: Read + Seek> RbReader<R> {
    pub fn new(reader: R) -> Result<Self> {
        Self::with_encoding(reader, None)
    }

    /// `encoding` mirrors the Python constructor's `encoding` override
    /// (`None` defaults to cp1252, as the RB text records are always
    /// written that way by `RBWriter`).
    pub fn with_encoding(mut reader: R, encoding: Option<&'static Encoding>) -> Result<Self> {
        let header = RbHeader::parse(&mut reader).context("Failed to parse RB header")?;
        let mi = get_metadata(&mut reader).context("Failed to read RB metadata")?;
        let toc = Self::read_toc(&mut reader, &header)?;
        Ok(RbReader {
            reader,
            header,
            mi,
            toc,
            encoding: encoding.unwrap_or(WINDOWS_1252),
        })
    }

    /// Port of the entry-reading half of `Reader.get_toc` (offset/count
    /// reading lives in [`RbHeader::parse`]).
    fn read_toc(reader: &mut R, header: &RbHeader) -> Result<Vec<RbTocEntry>> {
        reader.seek(SeekFrom::Start((header.toc_offset + 4) as u64))?;
        let mut entries = Vec::with_capacity(header.toc_count as usize);
        for _ in 0..header.toc_count {
            entries.push(RbTocEntry::read(reader)?);
        }
        Ok(entries)
    }

    /// Port of `Reader.get_text`. Returns `""` for `flags in (1, 2)`
    /// (Python's `get_text` returns without writing anything there --
    /// `extract_content` still lists the entry as a page regardless,
    /// see its own docs).
    fn get_text(&mut self, entry: &RbTocEntry) -> Result<String> {
        if matches!(entry.flag, 1 | 2) {
            return Ok(String::new());
        }

        self.reader.seek(SeekFrom::Start(entry.offset as u64))?;
        let mut output = String::new();

        if entry.flag == 8 {
            let count = self.reader.read_u32::<LittleEndian>()?;
            let _uncompressed_size = self.reader.read_u32::<LittleEndian>()?; // Read and discarded, matching Python.
            let mut chunk_sizes = Vec::with_capacity(count as usize);
            for _ in 0..count {
                chunk_sizes.push(self.reader.read_u32::<LittleEndian>()?);
            }
            for size in chunk_sizes {
                let mut compressed = vec![0u8; size as usize];
                self.reader.read_exact(&mut compressed)?;
                let mut decoder = ZlibDecoder::new(&compressed[..]);
                let mut raw = Vec::new();
                decoder
                    .read_to_end(&mut raw)
                    .context("inflating RB text chunk")?;
                let (decoded, _, _) = self.encoding.decode(&raw);
                output.push_str(&decoded);
            }
        } else {
            let mut raw = vec![0u8; entry.length as usize];
            self.reader.read_exact(&mut raw)?;
            let (decoded, _, _) = self.encoding.decode(&raw);
            output.push_str(&decoded);
        }

        Ok(output.replace("<TITLE>", "<TITLE> "))
    }

    /// Port of `Reader.get_image`. Returns empty bytes for `flags != 0`
    /// (Python's `get_image` returns without writing anything there).
    fn get_image(&mut self, entry: &RbTocEntry) -> Result<Vec<u8>> {
        if entry.flag != 0 {
            return Ok(Vec::new());
        }
        self.reader.seek(SeekFrom::Start(entry.offset as u64))?;
        let mut data = vec![0u8; entry.length as usize];
        self.reader.read_exact(&mut data)?;
        Ok(data)
    }

    /// Port of `Reader.extract_content`: `(pages, images)`, each a list
    /// of `(toc entry name, content)` pairs, in TOC order.
    ///
    /// Matches a real quirk of every file `RBWriter` itself produces:
    /// the writer always emits an `index.hidx` entry (flag 0, content
    /// `" "`) that this method never looks at -- its name ends in
    /// neither `"html"` nor `"png"` -- so it is genuinely dead weight
    /// in every calibre-written `.rb` file. Not "fixed" here.
    pub fn extract_content(&mut self) -> Result<(RbPages, RbImages)> {
        let mut pages = Vec::new();
        let mut images = Vec::new();

        let entries: Vec<RbTocEntry> = std::mem::take(&mut self.toc);

        for entry in &entries {
            // Python: `iname.lower().endswith('html')` -- note this is
            // NOT `.endswith('.html')`, so it also matches `xhtml`.
            let lname = entry.name.to_lowercase();
            if lname.ends_with("html") {
                let text = self.get_text(entry)?;
                pages.push((entry.name.clone(), text));
            }
            if lname.ends_with("png") {
                let data = self.get_image(entry)?;
                images.push((entry.name.clone(), data));
            }
        }

        self.toc = entries;
        Ok((pages, images))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::{Cursor, Write};

    /// Builds a minimal, spec-correct RB file with one info.info entry
    /// (flag 2), one flag-8 zlib-chunked text entry, and one flag-0 PNG
    /// image entry -- enough to exercise `RbReader::extract_content`'s
    /// three real code paths end to end.
    fn build_rb_file() -> Vec<u8> {
        let info_content = b"TITLE=Test\nAUTHOR=Someone\n".to_vec();

        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(b"<html><TITLE>Hi</TITLE><body>hello</body></html>")
            .unwrap();
        let chunk = enc.finish().unwrap();
        // count(4) + uncompressed_size(4) + chunk_sizes(4) + chunk bytes
        let text_payload_size = 4 + 4 + 4 + chunk.len();

        let png_data = b"\x89PNG\r\n\x1a\nfakepngdata".to_vec();

        let entries: Vec<(&str, u32, u32)> = vec![
            ("info.info", info_content.len() as u32, 2),
            ("index.html", text_payload_size as u32, 8),
            ("index.hidx", 1, 0),
            ("0.png", png_data.len() as u32, 0),
        ];

        let toc_start = 0x128u32;
        let page_count = entries.len() as u32;
        let toc_table_size = 4 + entries.len() as u32 * 44;
        let mut content_offset = toc_start + toc_table_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(crate::rb::header::MAGIC);
        buf.write_u32::<LittleEndian>(0).unwrap();
        buf.write_u32::<LittleEndian>(0).unwrap();
        buf.write_u16::<LittleEndian>(0).unwrap();
        buf.write_u32::<LittleEndian>(toc_start).unwrap();
        buf.write_u32::<LittleEndian>(0).unwrap(); // total-size placeholder
        for _ in (0x20..0x128).step_by(4) {
            buf.write_u32::<LittleEndian>(0).unwrap();
        }
        assert_eq!(buf.len() as u32, toc_start);
        buf.write_u32::<LittleEndian>(page_count).unwrap();

        let mut offsets = Vec::new();
        for (name, size, flag) in &entries {
            let mut field = [0u8; 32];
            let bytes = name.as_bytes();
            field[..bytes.len()].copy_from_slice(bytes);
            buf.write_all(&field).unwrap();
            buf.write_u32::<LittleEndian>(*size).unwrap();
            buf.write_u32::<LittleEndian>(content_offset).unwrap();
            buf.write_u32::<LittleEndian>(*flag).unwrap();
            offsets.push(content_offset);
            content_offset += size;
        }

        buf.write_all(&info_content).unwrap();

        buf.write_u32::<LittleEndian>(1).unwrap(); // 1 chunk
        buf.write_u32::<LittleEndian>(0).unwrap(); // uncompressed size (unused by reader)
        buf.write_u32::<LittleEndian>(chunk.len() as u32).unwrap();
        buf.write_all(&chunk).unwrap();

        buf.write_all(b" ").unwrap(); // index.hidx
        buf.write_all(&png_data).unwrap();

        let total = buf.len() as u32;
        (&mut buf[28..32]).write_u32::<LittleEndian>(total).unwrap();

        buf
    }

    #[test]
    fn extract_content_reads_text_and_images_and_skips_hidx() {
        let buf = build_rb_file();
        let mut reader = RbReader::new(Cursor::new(buf)).unwrap();
        assert_eq!(reader.mi.title, "Test");
        assert_eq!(reader.mi.authors, vec!["Someone".to_string()]);

        let (pages, images) = reader.extract_content().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, "index.html");
        // The `<TITLE>` -> `<TITLE> ` quirk is preserved.
        assert!(pages[0].1.contains("<TITLE> Hi</TITLE>"), "{}", pages[0].1);
        assert!(pages[0].1.contains("hello"), "{}", pages[0].1);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].0, "0.png");
        assert_eq!(images[0].1, b"\x89PNG\r\n\x1a\nfakepngdata");

        // index.hidx never surfaces in either list.
        assert!(pages.iter().all(|(n, _)| n != "index.hidx"));
        assert!(images.iter().all(|(n, _)| n != "index.hidx"));
    }
}
