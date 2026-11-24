//! DjVu container parsing and text extraction.
//!
//! Port of `old_src/src/calibre/ebooks/djvu/djvu.py` (`DjvuChunk` and
//! `DJVUFile`), which is in turn based on the *Lizardtech DjVu
//! Reference, DjVu v3, November 2005*.
//!
//! A DjVu file is IFF-85: the four magic bytes `AT&T` followed by a tree
//! of chunks, each `<4-byte id><4-byte big-endian size><size bytes>`,
//! padded to an even offset. `FORM` chunks carry an extra four-byte
//! subtype (`DJVU`, `DJVM`, ...) and contain further chunks.
//!
//! Text lives in `TXTa` (uncompressed) and `TXTz` (BZZ-compressed)
//! chunks, each holding a three-byte big-endian length followed by that
//! many bytes of UTF-8 text. Like the Python original, [`DjvuFile::text`]
//! concatenates each page's text separated by `0x1f` (ASCII unit
//! separator); the DJVU input plugin turns those into paragraph breaks.

use std::ops::Range;
use std::path::Path;

use thiserror::Error;

use super::bzz;

/// The four magic bytes that open every DjVu file.
pub const MAGIC: &[u8; 4] = b"AT&T";

/// Separator emitted between the text of consecutive chunks, matching
/// the Python `txtout.write(b'\037')`.
pub const TEXT_SEPARATOR: u8 = 0x1f;

/// Size of a chunk header: four-byte id plus four-byte length.
const HEADER_LEN: usize = 8;

/// Deepest `FORM` nesting accepted. Real documents use three levels
/// (`DJVM` > `DJVU` > chunks); the cap is what stops a crafted file of
/// nothing but `FORM` headers from recursing the parser off the stack.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Error)]
pub enum DjvuError {
    #[error("not a DjVu file: expected magic {:?}, got {0:?}", MAGIC)]
    BadMagic([u8; 4]),
    #[error("truncated DjVu file: chunk at offset {offset} needs {needed} bytes, file has {len}")]
    Truncated {
        offset: usize,
        needed: usize,
        len: usize,
    },
    #[error("failed to decode a TXTz chunk at offset {offset}: {source}")]
    Bzz {
        offset: usize,
        #[source]
        source: bzz::BzzError,
    },
    #[error("DjVu chunks nested more than {MAX_DEPTH} deep at offset {offset}")]
    TooDeeplyNested { offset: usize },
    #[error("failed to read DjVu file: {0}")]
    Io(#[from] std::io::Error),
}

/// One IFF chunk, with its position inside the file buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DjvuChunk {
    /// The four-byte chunk id, e.g. `FORM`, `TXTz`, `Sjbz`.
    pub id: [u8; 4],
    /// For `FORM` chunks, the four-byte subtype, e.g. `DJVU`.
    pub subtype: Option<[u8; 4]>,
    /// Declared payload size, as stored in the header. For a `FORM` this
    /// includes the four subtype bytes.
    pub size: usize,
    /// Payload extent in the file buffer, subtype bytes excluded.
    pub data: Range<usize>,
    /// Chunks nested inside this one (`FORM` chunks only).
    pub children: Vec<DjvuChunk>,
}

impl DjvuChunk {
    /// The chunk id as text, for display. Non-ASCII ids (a sign of a
    /// corrupt file) come back as `????`.
    pub fn id_str(&self) -> &str {
        std::str::from_utf8(&self.id).unwrap_or("????")
    }

    /// The `FORM` subtype as text, if this is a `FORM` chunk.
    pub fn subtype_str(&self) -> Option<&str> {
        self.subtype
            .as_ref()
            .map(|s| std::str::from_utf8(s).unwrap_or("????"))
    }

    /// Depth-first iterator over this chunk and all its descendants.
    pub fn iter(&self) -> impl Iterator<Item = &DjvuChunk> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let chunk = stack.pop()?;
            // Push in reverse so children come out in file order.
            stack.extend(chunk.children.iter().rev());
            Some(chunk)
        })
    }

    /// Parse the chunk starting at `offset`, recursing into `FORM`s.
    fn parse(buf: &[u8], offset: usize, depth: usize) -> Result<Self, DjvuError> {
        if depth > MAX_DEPTH {
            return Err(DjvuError::TooDeeplyNested { offset });
        }
        let header = buf
            .get(offset..offset + HEADER_LEN)
            .ok_or(DjvuError::Truncated {
                offset,
                needed: HEADER_LEN,
                len: buf.len().saturating_sub(offset),
            })?;
        let id: [u8; 4] = header[..4].try_into().expect("4-byte slice");
        let size = u32::from_be_bytes(header[4..].try_into().expect("4-byte slice")) as usize;

        let end = offset + HEADER_LEN + size;
        if end > buf.len() {
            return Err(DjvuError::Truncated {
                offset,
                needed: HEADER_LEN + size,
                len: buf.len() - offset,
            });
        }

        let mut data_start = offset + HEADER_LEN;
        let mut subtype = None;
        let is_form = &id == b"FORM";
        if is_form {
            let raw = buf
                .get(data_start..data_start + 4)
                .ok_or(DjvuError::Truncated {
                    offset,
                    needed: HEADER_LEN + 4,
                    len: buf.len() - offset,
                })?;
            subtype = Some(raw.try_into().expect("4-byte slice"));
            data_start += 4;
        }

        let mut children = Vec::new();
        if is_form {
            let mut pos = data_start;
            while pos < end {
                let child = Self::parse(buf, pos, depth + 1)?;
                // Chunks are padded to an even offset.
                pos += child.size + HEADER_LEN + (child.size % 2);
                children.push(child);
            }
        }

        Ok(Self {
            id,
            subtype,
            size,
            data: data_start..end,
            children,
        })
    }

    /// Append this chunk's text (and its descendants') to `out`, using
    /// `buf` for payload bytes.
    fn append_text(&self, buf: &[u8], out: &mut Vec<u8>) -> Result<(), DjvuError> {
        match &self.id {
            b"TXTz" => {
                let raw = &buf[self.data.clone()];
                let text = bzz::decompress(raw).map_err(|source| DjvuError::Bzz {
                    offset: self.data.start,
                    source,
                })?;
                out.extend_from_slice(&text);
                out.push(TEXT_SEPARATOR);
            }
            b"TXTa" => {
                out.extend_from_slice(text_record(&buf[self.data.clone()]));
                out.push(TEXT_SEPARATOR);
            }
            _ => {}
        }
        for child in &self.children {
            child.append_text(buf, out)?;
        }
        Ok(())
    }

    fn append_dump(&self, out: &mut String, indent: usize, max_level: usize) {
        out.push_str(&"  ".repeat(indent));
        out.push_str(self.id_str());
        if let Some(sub) = self.subtype_str() {
            out.push(':');
            out.push_str(sub);
        }
        out.push_str(&format!(" [{}]\n", self.size));
        if indent >= max_level {
            return;
        }
        for child in &self.children {
            child.append_dump(out, indent + 1, max_level);
        }
    }
}

/// Strip the three-byte big-endian length prefix from an uncompressed
/// text record, clamping to what is actually present.
fn text_record(raw: &[u8]) -> &[u8] {
    if raw.len() < 3 {
        return &[];
    }
    let declared = raw[..3]
        .iter()
        .fold(0usize, |acc, &b| (acc << 8) | b as usize);
    &raw[3..(3 + declared).min(raw.len())]
}

/// A parsed DjVu file: the magic-stripped buffer plus its chunk tree.
///
/// Port of the Python `DJVUFile` class.
#[derive(Debug, Clone)]
pub struct DjvuFile {
    /// File contents with the four magic bytes removed, so offsets match
    /// the Python implementation's.
    buf: Vec<u8>,
    root: DjvuChunk,
}

impl DjvuFile {
    /// Parse a whole DjVu file held in memory.
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> Result<Self, DjvuError> {
        let data = data.into();
        let magic: [u8; 4] =
            data.get(..4)
                .and_then(|m| m.try_into().ok())
                .ok_or(DjvuError::Truncated {
                    offset: 0,
                    needed: 4,
                    len: data.len(),
                })?;
        if &magic != MAGIC {
            return Err(DjvuError::BadMagic(magic));
        }
        let buf = data[4..].to_vec();
        let root = DjvuChunk::parse(&buf, 0, 0)?;
        Ok(Self { buf, root })
    }

    /// Read and parse a DjVu file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DjvuError> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// The outermost chunk, normally `FORM:DJVU` or `FORM:DJVM`.
    pub fn root(&self) -> &DjvuChunk {
        &self.root
    }

    /// Extract the OCR text layer, one `0x1f`-separated record per text
    /// chunk. Files that are pure page scans have no text chunks and
    /// yield an empty result.
    ///
    /// Port of `DJVUFile.get_text`.
    pub fn text(&self) -> Result<Vec<u8>, DjvuError> {
        let mut out = Vec::new();
        self.root.append_text(&self.buf, &mut out)?;
        Ok(out)
    }

    /// Render the chunk tree, one line per chunk, as
    /// `ID[:SUBTYPE] [size]`. `max_level` bounds the recursion depth the
    /// way the Python `dump(maxlevel=...)` argument does.
    ///
    /// Port of `DJVUFile.dump`.
    pub fn dump(&self, max_level: usize) -> String {
        let mut out = String::new();
        self.root.append_dump(&mut out, 1, max_level);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an IFF chunk with even-offset padding, as a DjVu writer
    /// would.
    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn form(subtype: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut payload = Vec::from(*subtype);
        payload.extend_from_slice(body);
        chunk(b"FORM", &payload)
    }

    fn text_payload(text: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(text.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(text);
        out
    }

    /// A single-page DjVu file whose page carries one TXTa chunk.
    fn file_with_txta(pages: &[&[u8]]) -> Vec<u8> {
        let mut form_body = b"DJVU".to_vec();
        for page in pages {
            form_body.extend_from_slice(&chunk(b"TXTa", &text_payload(page)));
        }
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&chunk(b"FORM", &form_body));
        out
    }

    #[test]
    fn rejects_non_djvu_input() {
        let err = DjvuFile::from_bytes(b"%PDF-1.4 and then some".to_vec()).unwrap_err();
        assert!(matches!(err, DjvuError::BadMagic(_)), "got {err:?}");
    }

    #[test]
    fn rejects_input_shorter_than_the_magic() {
        let err = DjvuFile::from_bytes(b"AT".to_vec()).unwrap_err();
        assert!(matches!(err, DjvuError::Truncated { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_a_chunk_that_runs_past_the_end() {
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(b"TXTa");
        raw.extend_from_slice(&1000u32.to_be_bytes());
        raw.extend_from_slice(b"short");
        let err = DjvuFile::from_bytes(raw).unwrap_err();
        assert!(matches!(err, DjvuError::Truncated { .. }), "got {err:?}");
    }

    #[test]
    fn parses_the_chunk_tree() {
        let file = DjvuFile::from_bytes(file_with_txta(&[b"hello"])).unwrap();
        let root = file.root();
        assert_eq!(root.id_str(), "FORM");
        assert_eq!(root.subtype_str(), Some("DJVU"));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].id_str(), "TXTa");
        assert_eq!(file.dump(10), "  FORM:DJVU [20]\n    TXTa [8]\n");
    }

    #[test]
    fn extracts_uncompressed_text_with_separators() {
        let file = DjvuFile::from_bytes(file_with_txta(&[b"page one", b"page two"])).unwrap();
        assert_eq!(file.text().unwrap(), b"page one\x1fpage two\x1f".to_vec());
    }

    #[test]
    fn odd_sized_chunks_are_padded_to_even_offsets() {
        // "odd" is 3 bytes, so its record is 6 bytes and the chunk gets
        // a pad byte; the following chunk must still be found.
        let file = DjvuFile::from_bytes(file_with_txta(&[b"odd", b"next"])).unwrap();
        assert_eq!(file.root().children.len(), 2);
        assert_eq!(file.text().unwrap(), b"odd\x1fnext\x1f".to_vec());
    }

    #[test]
    fn image_only_file_yields_no_text() {
        let mut form_body = b"DJVU".to_vec();
        form_body.extend_from_slice(&chunk(b"Sjbz", &[0xde, 0xad, 0xbe, 0xef]));
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&chunk(b"FORM", &form_body));
        let file = DjvuFile::from_bytes(raw).unwrap();
        assert!(file.text().unwrap().is_empty());
    }

    #[test]
    fn text_record_clamps_an_overlong_length() {
        // A record claiming more text than the chunk holds must not
        // panic; it yields what is there.
        assert_eq!(text_record(&[0x00, 0xff, 0xff, b'a', b'b']), b"ab");
        assert_eq!(text_record(&[0x00, 0x00]), b"");
    }

    #[test]
    fn refuses_pathologically_nested_forms() {
        // A file that is nothing but FORM headers would otherwise
        // recurse the parser off the stack, which aborts the process
        // rather than raising — so it is capped.
        let depth = MAX_DEPTH + 10;
        let mut body = Vec::new();
        for _ in 0..depth {
            body = form(b"DJVI", &body);
        }
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&body);

        let err = DjvuFile::from_bytes(raw).unwrap_err();
        assert!(
            matches!(err, DjvuError::TooDeeplyNested { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn iter_walks_depth_first_in_file_order() {
        let file = DjvuFile::from_bytes(file_with_txta(&[b"a", b"b"])).unwrap();
        let ids: Vec<&str> = file.root().iter().map(|c| c.id_str()).collect();
        assert_eq!(ids, vec!["FORM", "TXTa", "TXTa"]);
    }
}
