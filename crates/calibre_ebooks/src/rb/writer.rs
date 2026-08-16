//! Port of `calibre.ebooks.rb.writer.RBWriter`.
//!
//! Verified byte-for-byte from `write_content` -- see `header.rs`'s
//! module docs for the full layout table (this file writes exactly
//! that). Order of items written to the TOC, and thus to the file, is
//! always `info + text + hidx + images`:
//!
//! - `info.info` (note the literal name, not `"info"`): flag `2`, size
//!   = the UTF-8-encoded info string's length.
//! - `index.html` (the RBML text, zlib-chunked): flag `8`, size =
//!   `sum(chunk_sizes) + 8 + len(chunk_sizes) * 4` (the total bytes of
//!   the count header + uncompressed-size field + chunk-size array +
//!   compressed chunks -- exactly what `RbReader::get_text`'s flag-8
//!   branch reads back).
//! - `index.hidx`: a literal single space, flag `0`.
//! - every raster image `images()` embeds: flag `0`.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};

use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use encoding_rs::WINDOWS_1252;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::metadata::{authors_to_string, MetaInformation};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_RASTER_IMAGES;
use crate::oeb::stylizer::TagStylizer;
use crate::rb::header::MAGIC;
use crate::rb::rbml::{RbMlizer, RbOptions};
use crate::rb::unique_name;

const TEXT_RECORD_SIZE: usize = 4096;

struct TocItem {
    name: String,
    size: u32,
    flag: u32,
}

pub struct RbWriter {
    /// href -> assigned RB image name. Threaded into [`RbMlizer`] the
    /// same way Python threads `self.name_map` into
    /// `RBMLizer(self.log, name_map=self.name_map)`.
    pub name_map: HashMap<String, String>,
}

impl RbWriter {
    pub fn new() -> Self {
        RbWriter {
            name_map: HashMap::new(),
        }
    }

    /// Port of `RBWriter.write_content`.
    pub fn write_content<W: Write + Seek>(
        &mut self,
        oeb_book: &OEBBook,
        out_stream: &mut W,
        metadata: Option<&MetaInformation>,
        opts: &RbOptions,
    ) -> Result<()> {
        let images = self.images(oeb_book);

        let mut mlizer = RbMlizer::with_name_map(std::mem::take(&mut self.name_map));
        let markup = mlizer.extract_content(oeb_book, opts, &TagStylizer);
        self.name_map = std::mem::take(&mut mlizer.name_map);

        let (cow, _, _) = WINDOWS_1252.encode(&markup);
        let text_bytes = cow.into_owned();
        let text_size = text_bytes.len() as u32;
        let (chunks, chunk_sizes) = Self::text_chunks(&text_bytes);

        let info_content = self.info_section(oeb_book, metadata);

        let mut items = Vec::new();
        items.push(TocItem {
            name: "info.info".to_string(),
            size: info_content.len() as u32,
            flag: 2,
        });
        let text_toc_size = chunk_sizes.iter().sum::<u32>() + 8 + (chunk_sizes.len() as u32) * 4;
        items.push(TocItem {
            name: "index.html".to_string(),
            size: text_toc_size,
            flag: 8,
        });
        items.push(TocItem {
            name: "index.hidx".to_string(),
            size: 1,
            flag: 0,
        });
        for (name, data) in &images {
            items.push(TocItem {
                name: name.clone(),
                size: data.len() as u32,
                flag: 0,
            });
        }

        out_stream.write_all(MAGIC)?;
        out_stream.write_u32::<LittleEndian>(0)?;
        out_stream.write_u32::<LittleEndian>(0)?;
        out_stream.write_u16::<LittleEndian>(0)?;
        out_stream.write_u32::<LittleEndian>(0x128)?;
        out_stream.write_u32::<LittleEndian>(0)?; // total-size placeholder, patched at the end.
        for _ in (0x20..0x128).step_by(4) {
            out_stream.write_u32::<LittleEndian>(0)?;
        }
        out_stream.write_u32::<LittleEndian>(items.len() as u32)?;

        let mut offset = out_stream.stream_position()? as u32 + (items.len() as u32 * 44);
        for item in &items {
            let mut name_field = [0u8; 32];
            let bytes = item.name.as_bytes();
            let len = bytes.len().min(32);
            name_field[..len].copy_from_slice(&bytes[..len]);
            out_stream.write_all(&name_field)?;
            out_stream.write_u32::<LittleEndian>(item.size)?;
            out_stream.write_u32::<LittleEndian>(offset)?;
            out_stream.write_u32::<LittleEndian>(item.flag)?;
            offset += item.size;
        }

        out_stream.write_all(info_content.as_bytes())?;

        out_stream.write_u32::<LittleEndian>(chunks.len() as u32)?;
        out_stream.write_u32::<LittleEndian>(text_size)?;
        for size in &chunk_sizes {
            out_stream.write_u32::<LittleEndian>(*size)?;
        }
        for chunk in &chunks {
            out_stream.write_all(chunk)?;
        }

        out_stream.write_all(b" ")?; // index.hidx
        for (_, data) in &images {
            out_stream.write_all(data)?;
        }

        let total_size = out_stream.stream_position()?;
        out_stream.seek(SeekFrom::Start(0x1c))?;
        out_stream.write_u32::<LittleEndian>(total_size as u32)?;

        Ok(())
    }

    /// Port of `_text`'s chunking/compression half (the RBML generation
    /// itself lives in `write_content`, matching Python's `_text`
    /// calling `RBMLizer` then chunking its output in one function --
    /// split here only because `write_content` needs the markup for
    /// other things first). Splits `text` into <= [`TEXT_RECORD_SIZE`]
    /// -byte chunks and zlib-compresses each *independently* (not as
    /// one continuous stream), matching what `RbReader::get_text`'s
    /// flag-8 branch decompresses chunk-by-chunk.
    fn text_chunks(text: &[u8]) -> (Vec<Vec<u8>>, Vec<u32>) {
        let mut chunks = Vec::new();
        let mut sizes = Vec::new();
        for chunk in text.chunks(TEXT_RECORD_SIZE) {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(9));
            enc.write_all(chunk).expect("writing to a Vec never fails");
            let compressed = enc
                .finish()
                .expect("finishing a Vec zlib stream never fails");
            sizes.push(compressed.len() as u32);
            chunks.push(compressed);
        }
        (chunks, sizes)
    }

    /// Port of `_images`: converts every raster manifest image to
    /// 8-bit grayscale PNG, names it `{n}.png` (deduped via
    /// [`unique_name`]), and records `href -> name` in
    /// [`RbWriter::name_map`]. An image this crate's `image` crate
    /// cannot decode, or cannot re-encode as PNG, is silently skipped
    /// -- matching Python's `except Exception: log.error(...)` (which
    /// likewise just moves on to the next manifest item).
    fn images(&mut self, oeb_book: &OEBBook) -> Vec<(String, Vec<u8>)> {
        let mut images = Vec::new();
        let mut used_names: Vec<String> = Vec::new();

        for item in oeb_book.manifest.iter() {
            if !OEB_RASTER_IMAGES.contains(&item.media_type.as_str()) {
                continue;
            }
            let Ok(raw) = oeb_book.container.read(&item.href) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&raw) else {
                continue;
            };
            let gray = image::DynamicImage::ImageLuma8(img.to_luma8());

            let mut buf = std::io::Cursor::new(Vec::new());
            if gray.write_to(&mut buf, image::ImageFormat::Png).is_err() {
                continue;
            }
            let data = buf.into_inner();

            let candidate = format!("{}.png", used_names.len());
            let name = unique_name(&candidate, &used_names);
            used_names.push(name.clone());
            self.name_map.insert(item.href.clone(), name.clone());
            images.push((name, data));
        }

        images
    }

    /// Port of `_info_section`. `metadata` mirrors this trait's usual
    /// "overrides the book's own metadata" contract (see
    /// `crate::pdb::formatwriter`'s docs, and
    /// `crate::pdb::ereader::writer::Writer::build_metadata` for the
    /// same fallback shape): an explicit non-empty override wins, else
    /// `oeb_book.metadata` is consulted.
    fn info_section(&self, oeb_book: &OEBBook, metadata: Option<&MetaInformation>) -> String {
        let mut text = String::from("TYPE=2\n");

        let title = metadata
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| oeb_book.metadata.first("title").map(|i| i.value.clone()));
        if let Some(title) = title {
            text.push_str(&format!("TITLE={title}\n"));
        }

        let authors: Vec<String> = match metadata.filter(|m| !m.authors.is_empty()) {
            Some(m) => m.authors.clone(),
            None => oeb_book
                .metadata
                .get("creator")
                .iter()
                .map(|i| i.value.clone())
                .collect(),
        };
        if !authors.is_empty() {
            text.push_str(&format!("AUTHOR={}\n", authors_to_string(&authors)));
        }

        text.push_str(&format!(
            "GENERATOR={} - {}\n",
            calibre_utils::constants::APP_NAME,
            calibre_utils::constants::VERSION
        ));
        text.push_str("PARSE=1\n");
        text.push_str("OUTPUT=1\n");
        text.push_str("BODY=index.html\n");
        text
    }
}

impl Default for RbWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;
    use crate::rb::header::{RbHeader, RbTocEntry};
    use std::io::Cursor;

    // Returns the `TempDir` guard alongside the book -- the caller must
    // keep it alive for as long as the book is used, or the backing
    // directory (and everything written into it) is deleted on drop.
    fn simple_book() -> (OEBBook, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(tmp.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        let _ = book.container.write(
            "index.html",
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Hello RB world</p></body></html>",
        );
        (book, tmp)
    }

    #[test]
    fn header_layout_matches_the_verified_byte_table() {
        let (book, _tmp) = simple_book();
        let mi = MetaInformation::new("Title", vec!["Author".to_string()]);
        let mut out = Cursor::new(Vec::new());
        RbWriter::new()
            .write_content(&book, &mut out, Some(&mi), &RbOptions::default())
            .unwrap();
        let bytes = out.into_inner();

        assert_eq!(&bytes[0..14], MAGIC);
        // toc_offset field at byte 24 is always 0x128.
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 0x128);
        // The byte-28 total-size field equals the real file length --
        // the placeholder written during the header pass and the final
        // `seek(0x1c)` patch are the SAME field, not two different ones.
        let declared_size = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(declared_size as usize, bytes.len());
        // Bytes 32..296 are all zero.
        assert!(bytes[32..296].iter().all(|&b| b == 0));
        // page_count at byte 296: info + text + hidx = 3 (no images).
        assert_eq!(u32::from_le_bytes(bytes[296..300].try_into().unwrap()), 3);
    }

    #[test]
    fn toc_items_are_written_in_info_text_hidx_images_order_with_the_right_flags() {
        let (book, _tmp) = simple_book();
        let mut out = Cursor::new(Vec::new());
        RbWriter::new()
            .write_content(&book, &mut out, None, &RbOptions::default())
            .unwrap();
        let bytes = out.into_inner();

        let mut cursor = Cursor::new(bytes);
        let header = RbHeader::parse(&mut cursor).unwrap();
        cursor
            .seek(SeekFrom::Start((header.toc_offset + 4) as u64))
            .unwrap();
        let mut entries = Vec::new();
        for _ in 0..header.toc_count {
            entries.push(RbTocEntry::read(&mut cursor).unwrap());
        }

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "info.info");
        assert_eq!(entries[0].flag, 2);
        assert_eq!(entries[1].name, "index.html");
        assert_eq!(entries[1].flag, 8);
        assert_eq!(entries[2].name, "index.hidx");
        assert_eq!(entries[2].flag, 0);
        assert_eq!(entries[2].length, 1);
    }

    #[test]
    fn write_then_read_round_trips_text_via_rb_reader() {
        use crate::rb::reader::RbReader;

        let (book, _tmp) = simple_book();
        let mi = MetaInformation::new("Round Trip", vec!["A. Uthor".to_string()]);
        let mut out = Cursor::new(Vec::new());
        RbWriter::new()
            .write_content(&book, &mut out, Some(&mi), &RbOptions::default())
            .unwrap();

        let mut cursor = Cursor::new(out.into_inner());
        let mut reader = RbReader::new(&mut cursor).unwrap();
        assert_eq!(reader.mi.title, "Round Trip");
        assert_eq!(reader.mi.authors, vec!["A. Uthor".to_string()]);

        let (pages, _images) = reader.extract_content().unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].1.contains("Hello RB world"), "{}", pages[0].1);
    }
}
