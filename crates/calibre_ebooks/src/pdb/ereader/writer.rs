//! Port of `calibre.ebooks.pdb.ereader.writer`: serializes an OEB book
//! to eReader v2.02 bytes (compression code 10 / zlib, matching
//! `_header_record`'s hard-coded `compression = 10`).
//!
//! PML markup generation uses [`crate::pml::pmlml::PmlMlizer`] (issue
//! #47's port of `calibre.ebooks.pml.pmlml.PMLMLizer`), matching
//! Python's `write_content`, which builds one `PMLMLizer` and calls
//! `extract_content` once for the whole book rather than converting
//! each spine item separately. [`Writer::build_images`] in turn now
//! mirrors Python's `_images(oeb_book.manifest, pmlmlizer.image_hrefs)`:
//! only images the generated PML actually references (`PmlMlizer`'s
//! `image_hrefs` map) are embedded, not every raster image in the
//! manifest.

use std::io::Write;

use anyhow::{Context, Result};
use byteorder::{BigEndian, WriteBytesExt};
use encoding_rs::WINDOWS_1252;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use regex::bytes::Regex;

use crate::metadata::authors_to_string;
use crate::metadata::MetaInformation;
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_RASTER_IMAGES;
use crate::oeb::stylizer::TagStylizer;
use crate::oeb::transforms::rescale::fit_image;
use crate::pdb::formatwriter::FormatWriter;
use crate::pdb::header::PdbHeaderBuilder;
use crate::pml::pmlml::{PmlMlizer, PmlOptions};

const IDENTITY: &str = "PNRdPPrs";

/// This is an arbitrary number that is small enough to work. The actual
/// maximum record size is unknown.
const MAX_RECORD_SIZE: usize = 8192;

pub struct Writer;

impl Writer {
    pub fn new() -> Self {
        Writer
    }

    /// Port of `_text`: splits `pml` into <= [`MAX_RECORD_SIZE`]-byte
    /// chunks (space-aware only for the very first chunk -- see below),
    /// zlib-compresses each, and returns `(compressed_chunks,
    /// big-endian-u16-uncompressed-lengths)`.
    ///
    /// This deliberately mirrors an upstream quirk: Python's
    /// `pml.rfind(b' ', index, MAX_RECORD_SIZE)` searches for a split
    /// point using `MAX_RECORD_SIZE` as an *absolute* end bound rather
    /// than `index + MAX_RECORD_SIZE`. Once `index >= MAX_RECORD_SIZE`
    /// (true from the second chunk onward for any real book), that
    /// search always misses and every later chunk is a hard
    /// `MAX_RECORD_SIZE`-byte cut with no space-awareness. Ported as-is
    /// for output-format parity rather than "fixed", since nothing in
    /// this issue's scope calls for behavior-correcting the format.
    fn text_chunks(pml: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<u8>)> {
        let mut pages = Vec::new();
        let mut sizes = Vec::new();
        let mut index = 0usize;

        while index < pml.len() {
            let search_end = MAX_RECORD_SIZE.min(pml.len());
            let found = if index < search_end {
                pml[index..search_end]
                    .iter()
                    .rposition(|&b| b == b' ')
                    .map(|rel| index + rel)
            } else {
                None
            };

            let mut split_len = match found {
                Some(abs_pos) => abs_pos,
                None => {
                    let len_end = pml.len() - index;
                    if len_end > MAX_RECORD_SIZE {
                        MAX_RECORD_SIZE
                    } else {
                        len_end
                    }
                }
            };
            if split_len == 0 {
                split_len = 1;
            }

            let end = (index + split_len).min(pml.len());
            let chunk = &pml[index..end];

            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(chunk)
                .context("compressing eReader text chunk")?;
            pages.push(enc.finish().context("finishing eReader text chunk")?);
            sizes.write_u16::<BigEndian>(split_len.try_into().unwrap_or(u16::MAX))?;

            index += split_len;
        }

        Ok((pages, sizes))
    }

    /// Port of `_index_item`: for every regex match in `pml`, emits a
    /// `<4-byte BE match offset><cleaned text>\x00` entry. `has_val`
    /// mirrors the Python's `if 'val' in mo.groupdict()` branch (chapter
    /// heading level, indented by `4 * val` spaces).
    fn index_item(re: &Regex, pml: &[u8], has_val: bool) -> Vec<Vec<u8>> {
        let strip_u = Regex::new(r"\\U[0-9a-z]{4}").expect("static regex is valid");
        let strip_a = Regex::new(r"\\a\d{3}").expect("static regex is valid");
        let strip_any = Regex::new(r"\\.").expect("static regex is valid");

        let mut out = Vec::new();
        for caps in re.captures_iter(pml) {
            let Some(whole) = caps.get(0) else { continue };
            let Some(text_m) = caps.name("text") else {
                continue;
            };

            let mut item = Vec::new();
            item.extend_from_slice(&(whole.start() as u32).to_be_bytes());

            let mut text = text_m.as_bytes().to_vec();
            text = strip_u.replace_all(&text, &b""[..]).into_owned();
            text = strip_a.replace_all(&text, &b""[..]).into_owned();
            text = strip_any.replace_all(&text, &b""[..]).into_owned();

            if has_val {
                let val: usize = caps
                    .name("val")
                    .and_then(|m| std::str::from_utf8(m.as_bytes()).ok())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let mut prefixed = vec![b' '; 4 * val];
                prefixed.extend_from_slice(&text);
                text = prefixed;
            }

            item.extend_from_slice(&text);
            item.push(0);
            out.push(item);
        }
        out
    }

    /// Port of `_images`: embeds only the images `PmlMlizer` actually
    /// referenced (`image_hrefs`, href -> assigned PML name, e.g.
    /// `"cover.png"`/`"3.png"`), using that already-assigned name
    /// directly as the header's name field -- matching Python, which
    /// does not call `image_name` a second time here (only
    /// `PmlMlizer::dump_text` does, while generating the PML).
    /// Returns `(header, data)` pairs, port of the Python's tuple list.
    fn build_images(
        &self,
        oeb_book: &OEBBook,
        image_hrefs: &std::collections::HashMap<String, String>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut images = Vec::new();

        for item in oeb_book.manifest.iter() {
            if !OEB_RASTER_IMAGES.contains(&item.media_type.as_str()) {
                continue;
            }
            let Some(assigned_name) = image_hrefs.get(&item.href) else {
                continue;
            };
            let Ok(raw) = oeb_book.container.read(&item.href) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&raw) else {
                continue;
            };

            let (w, h) = (img.width() as f64, img.height() as f64);
            let (_, new_w, new_h) = fit_image(w, h, 300.0, 300.0);
            let thumb = img.resize(
                new_w.max(1) as u32,
                new_h.max(1) as u32,
                image::imageops::FilterType::Lanczos3,
            );

            let mut buf = std::io::Cursor::new(Vec::new());
            if thumb.write_to(&mut buf, image::ImageFormat::Png).is_err() {
                continue;
            }
            let data = buf.into_inner();

            let mut name_field = [0u8; 32];
            let bytes = assigned_name.as_bytes();
            let len = bytes.len().min(32);
            name_field[..len].copy_from_slice(&bytes[..len]);

            let mut header = b"PNG ".to_vec();
            header.extend_from_slice(&name_field);
            header.resize(58, 0);
            header
                .write_u16::<BigEndian>(thumb.width() as u16)
                .expect("writing to a Vec never fails");
            header
                .write_u16::<BigEndian>(thumb.height() as u16)
                .expect("writing to a Vec never fails");

            if header.len() + data.len() < 65505 {
                images.push((header, data));
            }
        }

        images
    }

    /// Port of `_metadata`. `oeb_book.metadata` (the `crate::oeb::metadata::Metadata`
    /// term/value list) is this trait's stand-in for what Python's real
    /// call site always passes as the `metadata` parameter
    /// (`oeb_book.metadata`, an `OEBMetadata` object with the same
    /// `.title`/`.creator`/`.rights`/`.publisher` shape); the
    /// `metadata: Option<&MetaInformation>` override this trait defines
    /// (see `crate::pdb::formatwriter`'s docs) takes precedence when
    /// present, matching that trait's documented "overrides the book's
    /// own metadata" contract.
    fn build_metadata(
        &self,
        oeb_book: &OEBBook,
        metadata: Option<&MetaInformation>,
    ) -> (Vec<u8>, String) {
        let title = metadata
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| oeb_book.metadata.first("title").map(|i| i.value.clone()))
            .unwrap_or_else(|| "Unknown".to_string());

        let author = metadata
            .filter(|m| !m.authors.is_empty())
            .map(|m| authors_to_string(&m.authors))
            .or_else(|| {
                let creators: Vec<String> = oeb_book
                    .metadata
                    .get("creator")
                    .iter()
                    .map(|i| i.value.clone())
                    .collect();
                if creators.is_empty() {
                    None
                } else {
                    Some(authors_to_string(&creators))
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let copyright = oeb_book
            .metadata
            .first("rights")
            .map(|i| i.value.clone())
            .unwrap_or_default();

        let publisher = metadata
            .and_then(|m| m.publisher.clone())
            .or_else(|| {
                oeb_book
                    .metadata
                    .first("publisher")
                    .map(|i| i.value.clone())
            })
            .unwrap_or_default();

        let isbn = metadata
            .and_then(|m| m.identifiers.get("isbn").cloned())
            .unwrap_or_default();

        let text = format!("{title}\x00{author}\x00{copyright}\x00{publisher}\x00{isbn}\x00");
        let (cow, _, _) = WINDOWS_1252.encode(&text);
        (cow.into_owned(), title)
    }

    /// Port of `_header_record`: the 132-byte eReader record-0 layout
    /// (compression 10 = zlib, matching this writer's `_text`).
    #[allow(clippy::too_many_arguments)]
    fn header_record(
        text_count: u16,
        chapter_count: u16,
        link_count: u16,
        image_count: u16,
    ) -> Vec<u8> {
        let compression: u16 = 10; // zlib compression.
        let non_text_offset = text_count + 1;

        let mut chapter_offset = non_text_offset;
        let mut link_offset = chapter_offset + chapter_count;

        let image_data_offset;
        let meta_data_offset;
        let last_data_offset;
        if image_count > 0 {
            image_data_offset = link_offset + link_count;
            meta_data_offset = image_data_offset + image_count;
            last_data_offset = meta_data_offset + 1;
        } else {
            meta_data_offset = link_offset + link_count;
            last_data_offset = meta_data_offset + 1;
            image_data_offset = last_data_offset;
        }

        if chapter_count == 0 {
            chapter_offset = last_data_offset;
        }
        if link_count == 0 {
            link_offset = last_data_offset;
        }

        let mut record = Vec::with_capacity(132);
        let push16 = |record: &mut Vec<u8>, v: u16| {
            record
                .write_u16::<BigEndian>(v)
                .expect("Vec write never fails");
        };
        push16(&mut record, compression); // [0:2]
        push16(&mut record, 0); // [2:4] Unknown.
        push16(&mut record, 0); // [4:6] Unknown.
        push16(&mut record, 25152); // [6:8] MAGIC (cp1252 marker).
        push16(&mut record, 0); // [8:10] small font pages.
        push16(&mut record, 0); // [10:12] large font pages.
        push16(&mut record, non_text_offset); // [12:14]
        push16(&mut record, chapter_count); // [14:16]
        push16(&mut record, 0); // [16:18]
        push16(&mut record, 0); // [18:20]
        push16(&mut record, image_count); // [20:22]
        push16(&mut record, link_count); // [22:24]
        push16(&mut record, 1); // [24:26] has metadata.
        push16(&mut record, 0); // [26:28]
        push16(&mut record, 0); // [28:30] footnotes.
        push16(&mut record, 0); // [30:32] sidebars.
        push16(&mut record, chapter_offset); // [32:34]
        push16(&mut record, 2560); // [34:36] MAGIC.
        push16(&mut record, last_data_offset); // [36:38]
        push16(&mut record, last_data_offset); // [38:40]
        push16(&mut record, image_data_offset); // [40:42]
        push16(&mut record, link_offset); // [42:44]
        push16(&mut record, meta_data_offset); // [44:46]
        push16(&mut record, 0); // [46:48]
        push16(&mut record, last_data_offset); // [48:50]
        push16(&mut record, last_data_offset); // [50:52]
        push16(&mut record, last_data_offset); // [52:54]

        record.resize(132, 0);
        record
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatWriter for Writer {
    fn write_content(
        &self,
        oeb_book: &OEBBook,
        output_stream: &mut dyn Write,
        metadata: Option<&MetaInformation>,
    ) -> Result<()> {
        // Port of Python's `PMLMLizer(self.log); pml =
        // str(pmlmlizer.extract_content(oeb_book, self.opts))`: one
        // whole-book pass (cover, spine, image/anchor bookkeeping),
        // not a per-spine-item conversion.
        let mut mlizer = PmlMlizer::new();
        let full_pml = mlizer.extract_content(oeb_book, &PmlOptions::default(), &TagStylizer);
        let (cow, _, _) = WINDOWS_1252.encode(&full_pml);
        let pml = cow.into_owned();

        let (text, text_sizes) = Self::text_chunks(&pml)?;

        let re_c = Regex::new(r#"(?s)\\C(?P<val>[0-4])="(?P<text>.+?)""#).expect("static regex");
        let re_x = Regex::new(r"(?s)\\X(?P<val>[0-4])(?P<text>.+?)\\X[0-4]").expect("static regex");
        let re_lower_x = Regex::new(r"(?s)\\x(?P<text>.+?)\\x").expect("static regex");
        let re_link = Regex::new(r#"(?s)\\Q="(?P<text>.+?)""#).expect("static regex");

        let mut chapter_index = Self::index_item(&re_c, &pml, true);
        chapter_index.extend(Self::index_item(&re_x, &pml, true));
        chapter_index.extend(Self::index_item(&re_lower_x, &pml, false));
        let link_index = Self::index_item(&re_link, &pml, false);

        let images = self.build_images(oeb_book, &mlizer.image_hrefs);
        let (metadata_bytes, title) = self.build_metadata(oeb_book, metadata);

        let hr = Self::header_record(
            text.len() as u16,
            chapter_index.len() as u16,
            link_index.len() as u16,
            images.len() as u16,
        );

        // Record order, matching Dropbook/writer.py:
        //   1. eReader Header
        //   2. Compressed text
        //   3. Chapter index
        //   4. Links index
        //   5. Images
        //   6. Metadata
        //   7. Text block size record
        //   8. "MeTaInFo\x00" word record
        let mut lengths = Vec::new();
        lengths.push(hr.len());
        lengths.extend(text.iter().map(|t| t.len()));
        lengths.extend(chapter_index.iter().map(|t| t.len()));
        lengths.extend(link_index.iter().map(|t| t.len()));
        lengths.extend(images.iter().map(|(h, d)| h.len() + d.len()));
        lengths.push(metadata_bytes.len());
        lengths.push(text_sizes.len());
        lengths.push(b"MeTaInFo\x00".len());

        let builder = PdbHeaderBuilder::new(IDENTITY, &title);
        let mut pdb_header_bytes = Vec::new();
        builder.build_header(&lengths, &mut pdb_header_bytes)?;
        output_stream.write_all(&pdb_header_bytes)?;

        output_stream.write_all(&hr)?;
        for chunk in &text {
            output_stream.write_all(chunk)?;
        }
        for entry in &chapter_index {
            output_stream.write_all(entry)?;
        }
        for entry in &link_index {
            output_stream.write_all(entry)?;
        }
        for (header, data) in &images {
            output_stream.write_all(header)?;
            output_stream.write_all(data)?;
        }
        output_stream.write_all(&metadata_bytes)?;
        output_stream.write_all(&text_sizes)?;
        output_stream.write_all(b"MeTaInFo\x00")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    #[test]
    fn text_chunks_round_trips_through_zlib() {
        // Note: this deliberately does *not* assert that each chunk's
        // declared `text_sizes` entry equals that chunk's actual
        // decompressed length. Per `text_chunks`'s docs, Python's
        // upstream chunker has a quirk where a chunk boundary landing
        // exactly on a matched space reuses that space's *absolute*
        // file offset as if it were a chunk length -- e.g. for this very
        // string, chunk 2's declared size is 44 (the offset the leading
        // space was found at) even though the chunk itself is 4 bytes
        // (" ten"). Neither `Reader132`/`Reader202` (this crate's own
        // readers) nor Python's `reader132.py`/`reader202.py` ever
        // consult `text_sizes` when reading -- it's written purely for
        // real eReader devices' page-navigation UI -- so this quirk
        // doesn't affect this crate's own round trip; what matters is
        // that concatenating every decompressed chunk reconstructs the
        // original PML exactly, which this test asserts instead.
        let pml = b"one two three four five six seven eight nine ten";
        let (chunks, sizes) = Writer::text_chunks(pml).unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(sizes.len(), chunks.len() * 2);

        let mut reconstructed = Vec::new();
        for chunk in &chunks {
            let mut dec = flate2::read::ZlibDecoder::new(&chunk[..]);
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut dec, &mut out).unwrap();
            reconstructed.extend_from_slice(&out);
        }
        assert_eq!(reconstructed, pml);
    }

    #[test]
    fn text_chunks_splits_long_text_into_multiple_records() {
        let pml = vec![b'a'; MAX_RECORD_SIZE * 3];
        let (chunks, _) = Writer::text_chunks(&pml).unwrap();
        assert!(chunks.len() >= 3, "{}", chunks.len());
    }

    #[test]
    fn index_item_extracts_chapter_headings() {
        let pml = br#"\C0="Chapter One"Chapter One text\x more \x"#;
        let re_c = Regex::new(r#"(?s)\\C(?P<val>[0-4])="(?P<text>.+?)""#).unwrap();
        let items = Writer::index_item(&re_c, pml, true);
        assert_eq!(items.len(), 1);
        assert!(items[0].ends_with(b"Chapter One\x00"));
    }

    #[test]
    fn write_content_produces_a_readable_pdb_header() {
        let tmp = tempfile::tempdir().unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(tmp.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        let _ = book.container.write(
            "index.html",
            b"<html><body><p>Hello world</p></body></html>",
        );

        let mi = MetaInformation::new("My Title", vec!["Author One".to_string()]);

        let writer = Writer::new();
        let mut out = Vec::new();
        writer.write_content(&book, &mut out, Some(&mi)).unwrap();

        // PDB fixed header is 78 bytes; identity lives at offset 60.
        assert!(out.len() > 78);
        assert_eq!(&out[60..68], b"PNRdPPrs");
    }

    /// Cross-validation per `docs/HARNESS.md`'s fallback policy (no
    /// `calibre-debug` available in this environment): write a book with
    /// `Writer`, then read it back with `crate::pdb::ereader::reader::Reader`
    /// (the same dispatcher a real `.pdb` file goes through) and check the
    /// original text survived the round trip. `Writer` always emits a
    /// 132-byte record 0 with `compression = 10` (zlib), so this should
    /// dispatch to `Reader132`.
    #[test]
    fn write_then_read_round_trips_text_content() {
        use crate::pdb::ereader::reader::Reader;
        use crate::pdb::formatreader::FormatReader;
        use crate::pdb::header::PdbHeader;

        let tmp = tempfile::tempdir().unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(tmp.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        let _ = book.container.write(
            "index.html",
            b"<html><body><p>Hello round trip world</p></body></html>",
        );

        let mi = MetaInformation::new("Round Trip Title", vec!["Round Trip Author".to_string()]);

        let writer = Writer::new();
        let mut out = Vec::new();
        writer.write_content(&book, &mut out, Some(&mi)).unwrap();

        let mut cursor = std::io::Cursor::new(out);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        assert_eq!(&header.type_id, b"PNRd");
        assert_eq!(&header.creator_id, b"PPrs");

        let reader = Reader::new(&header, &mut cursor, None).unwrap();
        assert!(matches!(reader, Reader::V132(_)));

        let pml = reader.dump_pml().unwrap();
        assert!(pml.contains("Hello round trip world"), "{pml}");

        let out_dir = tempfile::tempdir().unwrap();
        reader.extract_content(out_dir.path()).unwrap();
        let html = std::fs::read_to_string(out_dir.path().join("index.html")).unwrap();
        assert!(html.contains("Hello round trip world"), "{html}");
        assert!(out_dir.path().join("metadata.opf").exists());
    }
}
