//! SNB (Shanda Bambook) container reader.
//!
//! Port of `SNBFile.Parse`/`ParseFile`/`ParseTail`/`IsValid` from
//! `old_src/src/calibre/ebooks/snb/snbfile.py`. The container is:
//!
//! - An 8-byte magic (`SNBP000B`) followed by 9 big-endian `i32` header
//!   fields (44 bytes total): magic, `rev80`, `revA3`, `revZ1`,
//!   `fileCount`, `vfatSize`, `vfatCompressed`, `binStreamSize`,
//!   `plainStreamSizeUncompressed`, `revZ2`.
//! - A zlib-compressed "VFAT" file table right after the header:
//!   `fileCount` fixed 12-byte records (`attr`, name-table offset,
//!   file size), followed by a NUL-separated file name table. `attr`
//!   bit `0x41000000` marks a "plain" (text, bz2-block-compressed)
//!   file; bit `0x01000000` marks a "binary" (stored raw) file.
//! - The binary stream (raw file bytes concatenated), then the plain
//!   stream (each file's bytes bz2-compressed in independent
//!   `0x8000`-byte blocks).
//! - A 16-byte tail pointer at the very end of the file (`tailSize`,
//!   `tailOffset`, `tailMagic`, itself `SNBP000B`) that points at a
//!   zlib-compressed tail block: per-block file offsets, followed by
//!   each file's `blockIndex`/`contentOffset` pair.
//!
//! `vfatCompressed` and `plainStreamSizeUncompressed` are genuine
//! byte-count/size integers in the Python (`vfatCompressed` is how
//! many compressed bytes to read for the VFAT table;
//! `plainStreamSizeUncompressed` is compared against the decompressed
//! plain stream's length later) -- an earlier draft of this module
//! mistakenly typed them as `bool` on a `ParseHeader` struct that was
//! never actually read back through (the real values lived in local
//! `u32`s instead). This port stores every header field as its real
//! `u32`/byte-array type directly on `SnbReader`, mirroring the flat
//! `self.X` attributes Python's `SNBFile` uses.

use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

/// Port of `SNBFile.MAGIC`.
pub const MAGIC: &[u8] = b"SNBP000B";
/// Port of `SNBFile.REV80`.
pub(crate) const REV80: u32 = 0x0000_8000;
/// Port of `SNBFile.REVA3`. Written by `Output()` but, per `IsValid`'s
/// commented-out check, never actually validated on read -- see
/// [`SnbReader::is_valid`].
pub(crate) const REVA3: u32 = 0x00A3_A3A3;
/// Port of `SNBFile.REVZ1`.
pub(crate) const REVZ1: u32 = 0x0000_0000;
/// Port of `SNBFile.REVZ2`.
pub(crate) const REVZ2: u32 = 0x0000_0000;

/// A block below this raw byte length is assumed bz2-compressed; at or
/// above it, assumed already stored raw (a block that compressed to
/// *larger* than the uncompressed block size is stored raw instead, so
/// the writer never wastes space "compressing" already-dense data).
/// Port of the literal `32768` in `SNBFile.Parse`.
const RAW_BLOCK_THRESHOLD: usize = 32768;

/// Port of `FileStream`/`BlockData`'s attr bit constants.
pub const ATTR_PLAIN: u32 = 0x4100_0000;
pub const ATTR_BINARY: u32 = 0x0100_0000;

/// Port of `FileStream`.
#[derive(Debug, Clone, Default)]
pub struct SnbFileEntry {
    pub attr: u32,
    pub file_name_offset: u32,
    pub file_size: u32,
    pub file_name: String,

    // Filled in by `ParseTail`.
    pub block_index: i32,
    pub content_offset: i32,

    pub file_body: Vec<u8>,
}

impl SnbFileEntry {
    /// Port of `FileStream.IsBinary`.
    pub fn is_binary(&self) -> bool {
        self.attr & ATTR_PLAIN != ATTR_PLAIN
    }
}

/// Port of `calibre.ebooks.snb.snbfile.SNBFile`'s parse-related state
/// and methods. Owns the input stream for the duration of `parse()`.
pub struct SnbReader<R: Read + Seek> {
    stream: R,

    // Header fields, byte-for-byte from `struct.unpack('>8siiiiiiiii', vmbr)`.
    pub magic: [u8; 8],
    pub rev80: u32,
    pub rev_a3: u32,
    pub rev_z1: u32,
    pub file_count: u32,
    pub vfat_size: u32,
    /// Compressed byte count of the VFAT+filename table -- a real size,
    /// not a flag. See the module docs.
    pub vfat_compressed: u32,
    pub bin_stream_size: u32,
    /// Total decompressed length of the plain (text) stream -- a real
    /// size, not a flag. See the module docs.
    pub plain_stream_size_uncompressed: u32,
    pub rev_z2: u32,

    /// Decompressed VFAT+filename table bytes (`self.vfat`).
    pub vfat: Vec<u8>,
    pub files: Vec<SnbFileEntry>,
    /// Per-block byte offsets into the file, binary blocks first, then
    /// plain blocks (`self.blocks[i].Offset`).
    pub blocks: Vec<u32>,

    pub tail_size: u32,
    pub tail_offset: u32,
    pub tail_magic: [u8; 8],
    pub tail_size_uncompressed: u32,

    pub bin_block: u32,
    pub plain_block: u32,
}

impl<R: Read + Seek> SnbReader<R> {
    pub fn new(stream: R) -> Result<Self> {
        Ok(Self {
            stream,
            magic: [0u8; 8],
            rev80: 0,
            rev_a3: 0,
            rev_z1: 0,
            file_count: 0,
            vfat_size: 0,
            vfat_compressed: 0,
            bin_stream_size: 0,
            plain_stream_size_uncompressed: 0,
            rev_z2: 0,
            vfat: Vec::new(),
            files: Vec::new(),
            blocks: Vec::new(),
            tail_size: 0,
            tail_offset: 0,
            tail_magic: [0u8; 8],
            tail_size_uncompressed: 0,
            bin_block: 0,
            plain_block: 0,
        })
    }

    /// Port of `SNBFile.Parse`. Unlike the earlier draft, this does
    /// *not* itself reject a bad magic/revision -- Python's `Parse`
    /// happily reads whatever is there and leaves validation to
    /// `is_valid()`, called separately by the caller. This port keeps
    /// that split so `is_valid()` can report *why* a file is invalid
    /// rather than the first `parse()` call just erroring out.
    ///
    /// `parse()` still does error (via `?`/`bail!`) on conditions the
    /// Python itself raises on: I/O failures, a VFAT/tail zlib stream
    /// that won't decompress, an unrecognized file `attr`, and a plain
    /// stream whose reconstructed length doesn't match
    /// `plain_stream_size_uncompressed` (Python's `raise Exception()`).
    pub fn parse(&mut self) -> Result<()> {
        // -- Header (44 bytes: `>8siiiiiiiii`) -----------------------
        self.stream.read_exact(&mut self.magic)?;
        self.rev80 = self.stream.read_u32::<BigEndian>()?;
        self.rev_a3 = self.stream.read_u32::<BigEndian>()?;
        self.rev_z1 = self.stream.read_u32::<BigEndian>()?;
        self.file_count = self.stream.read_u32::<BigEndian>()?;
        self.vfat_size = self.stream.read_u32::<BigEndian>()?;
        self.vfat_compressed = self.stream.read_u32::<BigEndian>()?;
        self.bin_stream_size = self.stream.read_u32::<BigEndian>()?;
        self.plain_stream_size_uncompressed = self.stream.read_u32::<BigEndian>()?;
        self.rev_z2 = self.stream.read_u32::<BigEndian>()?;

        // -- VFAT (file table) ----------------------------------------
        let mut vfat_comp = vec![0u8; self.vfat_compressed as usize];
        self.stream.read_exact(&mut vfat_comp)?;
        self.vfat = zlib_decompress(&vfat_comp)?;
        self.parse_file()?;

        // -- Tail pointer (last 16 bytes: `>ii8s`) --------------------
        self.stream.seek(SeekFrom::End(-16))?;
        self.tail_size = self.stream.read_u32::<BigEndian>()?;
        self.tail_offset = self.stream.read_u32::<BigEndian>()?;
        self.stream.read_exact(&mut self.tail_magic)?;

        self.stream.seek(SeekFrom::Start(self.tail_offset as u64))?;
        let mut tail_comp = vec![0u8; self.tail_size as usize];
        self.stream.read_exact(&mut tail_comp)?;
        let tail_uncompressed = zlib_decompress(&tail_comp)?;
        self.tail_size_uncompressed = tail_uncompressed.len() as u32;
        self.parse_tail(&tail_uncompressed)?;

        // -- File bodies ------------------------------------------------
        let mut uncompressed_plain: Option<Vec<u8>> = None;
        let mut plain_pos: usize = 0;
        let mut bin_pos: u32 = 0;
        let start_bin = 44 + self.vfat_compressed;

        for i in 0..self.files.len() {
            let attr = self.files[i].attr;
            if attr & ATTR_PLAIN == ATTR_PLAIN {
                if uncompressed_plain.is_none() {
                    uncompressed_plain = Some(self.decompress_plain_stream()?);
                }
                let acc = uncompressed_plain.as_ref().expect("just set above");
                if acc.len() as u32 != self.plain_stream_size_uncompressed {
                    // Port of `if len(uncompressedData) != self.plainStreamSizeUncompressed: raise Exception()`.
                    bail!(
                        "SNB plain stream length mismatch: got {}, expected {}",
                        acc.len(),
                        self.plain_stream_size_uncompressed
                    );
                }
                let size = self.files[i].file_size as usize;
                let end = plain_pos + size;
                if end > acc.len() {
                    bail!(
                        "SNB plain stream too short for file {} ({} bytes at offset {}, stream is {} bytes)",
                        self.files[i].file_name,
                        size,
                        plain_pos,
                        acc.len()
                    );
                }
                self.files[i].file_body = acc[plain_pos..end].to_vec();
                plain_pos = end;
            } else if attr & ATTR_BINARY == ATTR_BINARY {
                self.stream
                    .seek(SeekFrom::Start((start_bin + bin_pos) as u64))?;
                let mut buf = vec![0u8; self.files[i].file_size as usize];
                self.stream.read_exact(&mut buf)?;
                self.files[i].file_body = buf;
                bin_pos += self.files[i].file_size;
            } else {
                // Port of `raise ValueError(f'Invalid file: {f.attr} {f.fileName}')`.
                bail!(
                    "Invalid SNB file entry: attr={:#x} name={}",
                    attr,
                    self.files[i].file_name
                );
            }
        }

        Ok(())
    }

    /// Port of the plain-stream reconstruction loop inside `Parse`.
    ///
    /// For each plain block: compute its byte range from consecutive
    /// block offsets (falling back to `tailOffset` for the last
    /// block), read that range, and bz2-decompress it *only* if it's
    /// shorter than [`RAW_BLOCK_THRESHOLD`] -- otherwise the block was
    /// stored raw by the writer (see that constant's docs) and is
    /// appended as-is.
    ///
    /// Python wraps the read+decompress in a bare
    /// `except Exception: print(traceback...)` that swallows the
    /// error and moves on to the next block, silently leaving
    /// `uncompressedData` short for this block. This port preserves
    /// that "warn and continue" behavior (via `eprintln!`, matching
    /// Python's `print` destination) rather than aborting the whole
    /// parse on one bad block -- the length check right after this
    /// function returns (`plain_stream_size_uncompressed` comparison)
    /// is what actually surfaces the resulting corruption as a real
    /// error, exactly as it does in the Python.
    fn decompress_plain_stream(&mut self) -> Result<Vec<u8>> {
        let mut acc = Vec::new();
        for j in 0..self.plain_block {
            let idx = (self.bin_block + j) as usize;
            let Some(&start_off) = self.blocks.get(idx) else {
                eprintln!("SNB: missing block offset for plain block {j}, skipping");
                continue;
            };
            let end_off = if j + 1 < self.plain_block {
                self.blocks.get(idx + 1).copied().unwrap_or(start_off)
            } else {
                self.tail_offset
            };
            let size = end_off.saturating_sub(start_off) as usize;

            let block_result = (|| -> Result<Vec<u8>> {
                self.stream.seek(SeekFrom::Start(start_off as u64))?;
                let mut chunk = vec![0u8; size];
                self.stream.read_exact(&mut chunk)?;
                if chunk.len() < RAW_BLOCK_THRESHOLD {
                    Ok(bz2_decompress(&chunk)?)
                } else {
                    Ok(chunk)
                }
            })();

            match block_result {
                Ok(mut bytes) => acc.append(&mut bytes),
                Err(e) => {
                    // Port of Python's bare `except Exception: print(traceback...)`:
                    // the block contributes nothing, and the loop moves on.
                    eprintln!("SNB: failed to read/decompress plain block {j}: {e}");
                }
            }
        }
        Ok(acc)
    }

    /// Port of `SNBFile.ParseFile`.
    fn parse_file(&mut self) -> Result<()> {
        let file_count = self.file_count as usize;
        let header_len = file_count * 12;
        if self.vfat.len() < header_len {
            bail!(
                "SNB VFAT table too short: {} bytes for {} files",
                self.vfat.len(),
                file_count
            );
        }
        let names_block = &self.vfat[header_len..];
        let names: Vec<&[u8]> = names_block.split(|&b| b == 0).collect();

        let mut cursor = Cursor::new(&self.vfat[..header_len]);
        for i in 0..file_count {
            let attr = cursor.read_u32::<BigEndian>()?;
            let file_name_offset = cursor.read_u32::<BigEndian>()?;
            let file_size = cursor.read_u32::<BigEndian>()?;
            let file_name = names
                .get(i)
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            self.files.push(SnbFileEntry {
                attr,
                file_name_offset,
                file_size,
                file_name,
                block_index: 0,
                content_offset: 0,
                file_body: Vec::new(),
            });
        }
        Ok(())
    }

    /// Port of `SNBFile.ParseTail`.
    fn parse_tail(&mut self, vtail: &[u8]) -> Result<()> {
        self.bin_block = self.bin_stream_size.div_ceil(0x8000);
        self.plain_block = self.plain_stream_size_uncompressed.div_ceil(0x8000);

        let total_blocks = (self.bin_block + self.plain_block) as usize;
        let mut cursor = Cursor::new(vtail);
        for _ in 0..total_blocks {
            self.blocks.push(cursor.read_u32::<BigEndian>()?);
        }
        for file in self.files.iter_mut() {
            file.block_index = cursor.read_i32::<BigEndian>()?;
            file.content_offset = cursor.read_i32::<BigEndian>()?;
        }
        Ok(())
    }

    /// Port of `SNBFile.IsValid`. `rev_a3` is deliberately *not*
    /// checked -- Python comments that check out
    /// (`# if self.revA3 != SNBFile.REVA3: return False`), so this is
    /// genuinely unvalidated upstream, not an oversight.
    pub fn is_valid(&self) -> bool {
        if self.magic != MAGIC {
            return false;
        }
        if self.rev80 != REV80 {
            return false;
        }
        if self.rev_z1 != REVZ1 {
            return false;
        }
        if self.rev_z2 != REVZ2 {
            return false;
        }
        if self.vfat_size != self.vfat.len() as u32 {
            return false;
        }
        if self.file_count != self.files.len() as u32 {
            return false;
        }
        if (self.bin_block + self.plain_block) * 4 + self.file_count * 8
            != self.tail_size_uncompressed
        {
            return false;
        }
        if self.tail_magic != MAGIC {
            return false;
        }
        true
    }

    /// Port of `SNBFile.GetFileStream`.
    pub fn get_file(&self, name: &str) -> Option<Vec<u8>> {
        self.files
            .iter()
            .find(|f| f.file_name == name)
            .map(|f| f.file_body.clone())
    }
}

/// zlib-decompress (Python's bare `zlib.decompress`).
///
/// `flate2` is more lenient than Python's `zlib` module on empty
/// input: `ZlibDecoder::read_to_end` on a zero-byte slice returns
/// `Ok(0)` (an empty result), whereas Python's `zlib.decompress(b'')`
/// raises `zlib.error: ... incomplete or truncated stream` (a valid
/// zlib stream needs at least a 2-byte header and a trailing Adler-32,
/// so empty input is never valid). This matters for real inputs: a
/// corrupt/truncated `.snb` file can genuinely have
/// `vfatCompressed == 0`. This port rejects empty input explicitly to
/// match; it does not attempt to replicate every other truncation case
/// zlib itself would reject (`flate2` already errors on most of those).
fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        bail!("SNB: empty zlib stream (incomplete or truncated)");
    }
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// bz2-decompress one block (Python's `bz2.BZ2Decompressor().decompress(data)`,
/// a fresh decompressor per block).
fn bz2_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = bzip2::read::BzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snb::writer::{SnbOutputFile, SnbWriter};
    use std::io::Cursor;

    fn plain(name: &str, body: &[u8]) -> SnbOutputFile {
        SnbOutputFile::plain(name, body.to_vec())
    }

    fn binary(name: &str, body: &[u8]) -> SnbOutputFile {
        SnbOutputFile::binary(name, body.to_vec())
    }

    fn write_snb(files: Vec<SnbOutputFile>) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        SnbWriter::new(files).output(&mut buf).unwrap();
        buf.into_inner()
    }

    #[test]
    fn rejects_bad_magic() {
        let mut r = SnbReader::new(Cursor::new(b"BADMAGIC".to_vec())).unwrap();
        assert!(r.parse().is_err());
    }

    #[test]
    fn parses_and_validates_a_written_container() {
        let bytes = write_snb(vec![
            plain("snbf/book.snbf", b"<book/>"),
            binary("snbc/images/cover.jpg", &[0xFFu8, 0xD8, 0xFF, 0xD9]),
        ]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        assert!(r.is_valid());
        assert_eq!(r.get_file("snbf/book.snbf").unwrap(), b"<book/>");
        assert_eq!(
            r.get_file("snbc/images/cover.jpg").unwrap(),
            vec![0xFFu8, 0xD8, 0xFF, 0xD9]
        );
    }

    #[test]
    fn is_valid_rejects_a_bad_rev80() {
        let bytes = write_snb(vec![plain("a.snbc", b"hi")]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        r.rev80 = 0xDEAD_BEEF;
        assert!(!r.is_valid());
    }

    #[test]
    fn is_valid_rejects_a_tampered_vfat_size() {
        let bytes = write_snb(vec![plain("a.snbc", b"hi")]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        r.vfat_size += 1;
        assert!(!r.is_valid());
    }

    #[test]
    fn large_plain_blocks_are_stored_and_read_back_raw() {
        // A block whose bz2-compressed form would be >= 32768 bytes is
        // written raw by `SnbWriter` (see its docs) -- verify the
        // reader's `< 32768` branch correctly treats it as
        // already-uncompressed rather than trying (and failing) to
        // bz2-decompress it.
        let body = vec![0u8; 0x9000]; // > one 0x8000 block, incompressible-ish content below
        let bytes = write_snb(vec![plain("big.snbc", &body)]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        assert_eq!(r.get_file("big.snbc").unwrap(), body);
    }

    #[test]
    fn empty_file_list_round_trips() {
        let bytes = write_snb(vec![]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        assert!(r.is_valid());
        assert_eq!(r.files.len(), 0);
    }

    #[test]
    fn multiple_plain_files_spanning_several_blocks_round_trip() {
        let a = vec![b'a'; 40_000]; // spans multiple 0x8000 blocks
        let b = vec![b'b'; 100];
        let bytes = write_snb(vec![plain("a.snbc", &a), plain("b.snbc", &b)]);
        let mut r = SnbReader::new(Cursor::new(bytes)).unwrap();
        r.parse().unwrap();
        assert_eq!(r.get_file("a.snbc").unwrap(), a);
        assert_eq!(r.get_file("b.snbc").unwrap(), b);
    }
}
