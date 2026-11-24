//! SNB (Shanda Bambook) container writer.
//!
//! Port of `SNBFile.Output`/`FromDir`/`AppendPlain`/`AppendBinary` from
//! `old_src/src/calibre/ebooks/snb/snbfile.py`. This is a hand-written
//! mirror of [`crate::snb::reader::SnbReader`]'s read path, not a
//! reuse of it -- the Python's `Output` builds the VFAT table, binary
//! stream, plain stream, and tail block independently of `Parse`, and
//! this port keeps that same independence (verified against the
//! Python method-by-method, not assumed symmetric).

use crate::snb::reader::{ATTR_BINARY, ATTR_PLAIN, MAGIC, REV80, REVA3};
use anyhow::{bail, Result};
use std::io::Write;
use std::path::Path;

const BLOCK_SIZE: usize = 0x8000;
/// `0x2C` = 44, the fixed header size (`>8siiiiiiiii`).
const HEADER_SIZE: usize = 44;

/// Port of `FileStream`, as used on the write side (before
/// `Output()` assigns `contentOffset`).
#[derive(Debug, Clone)]
pub struct SnbOutputFile {
    pub attr: u32,
    /// Port of `f.fileName` after `.replace(os.sep, '/')` and
    /// `.encode('ascii', 'ignore')`. Stored as `String` here since the
    /// 'ignore' error handler drops any non-ASCII byte, leaving only
    /// bytes that are valid ASCII (and therefore valid UTF-8) either
    /// way.
    pub file_name: String,
    pub file_body: Vec<u8>,
    /// Filled in by `output()`, mirroring Python's `f.contentOffset`.
    pub content_offset: u32,
}

impl SnbOutputFile {
    /// Port of `AppendPlain`'s `FileStream` construction (attr
    /// `0x41000000`).
    pub fn plain(file_name: impl Into<String>, file_body: Vec<u8>) -> Self {
        Self {
            attr: ATTR_PLAIN,
            file_name: ascii_ignore(&normalize_sep(&file_name.into())),
            file_body,
            content_offset: 0,
        }
    }

    /// Port of `AppendBinary`'s `FileStream` construction (attr
    /// `0x01000000`).
    pub fn binary(file_name: impl Into<String>, file_body: Vec<u8>) -> Self {
        Self {
            attr: ATTR_BINARY,
            file_name: ascii_ignore(&normalize_sep(&file_name.into())),
            file_body,
            content_offset: 0,
        }
    }

    /// Port of `FileStream.IsBinary`.
    pub fn is_binary(&self) -> bool {
        self.attr & ATTR_PLAIN != ATTR_PLAIN
    }
}

/// `fileName.replace(os.sep, '/')`. On a POSIX build `os.sep` is
/// already `/`, so this is a no-op there; kept for parity with the
/// Python (and in case this ever runs cross-compiled for Windows,
/// where `std::path::MAIN_SEPARATOR` is `\\`).
fn normalize_sep(name: &str) -> String {
    name.replace(std::path::MAIN_SEPARATOR, "/")
}

/// Port of `.encode('ascii', 'ignore')`: drop every byte that isn't
/// plain ASCII, rather than erroring or replacing it.
fn ascii_ignore(name: &str) -> String {
    name.chars().filter(char::is_ascii).collect()
}

/// Port of `calibre.ebooks.snb.snbfile.SNBFile`'s write-side methods.
#[derive(Debug, Default)]
pub struct SnbWriter {
    pub files: Vec<SnbOutputFile>,
}

impl SnbWriter {
    pub fn new(files: Vec<SnbOutputFile>) -> Self {
        Self { files }
    }

    /// Port of `FromDir`: walks `tdir`, treating `.snbf`/`.snbc` files
    /// as "plain" and everything else as "binary".
    pub fn from_dir(tdir: &Path) -> Result<Self> {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(tdir) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(tdir)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let body = std::fs::read(entry.path())?;
            if ext == "snbf" || ext == "snbc" {
                files.push(SnbOutputFile::plain(rel, body));
            } else {
                files.push(SnbOutputFile::binary(rel, body));
            }
        }
        Ok(Self { files })
    }

    /// Port of `AppendPlain`.
    pub fn append_plain(&mut self, file_name: impl Into<String>, file_body: Vec<u8>) {
        self.files.push(SnbOutputFile::plain(file_name, file_body));
    }

    /// Port of `AppendBinary`.
    pub fn append_binary(&mut self, file_name: impl Into<String>, file_body: Vec<u8>) {
        self.files.push(SnbOutputFile::binary(file_name, file_body));
    }

    /// Port of `GetFileStream`.
    pub fn get_file_stream(&self, file_name: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|f| f.file_name == file_name)
            .map(|f| f.file_body.as_slice())
    }

    /// Port of `SNBFile.Output`.
    pub fn output<W: Write>(&mut self, out: &mut W) -> Result<()> {
        // Sort the files in the file buffer, required by the SNB file
        // format (Python's own comment).
        self.files.sort_by(|a, b| a.file_name.cmp(&b.file_name));

        // -- Build VFAT + filename table + bin/plain streams ---------
        let mut vfat = Vec::new();
        let mut file_name_table = Vec::new();
        let mut plain_stream = Vec::new();
        let mut bin_stream = Vec::new();

        for f in self.files.iter_mut() {
            vfat.extend_from_slice(&f.attr.to_be_bytes());
            vfat.extend_from_slice(&(file_name_table.len() as u32).to_be_bytes());
            vfat.extend_from_slice(&(f.file_body.len() as u32).to_be_bytes());
            file_name_table.extend_from_slice(f.file_name.as_bytes());
            file_name_table.push(0);

            if f.attr & ATTR_PLAIN == ATTR_PLAIN {
                f.content_offset = plain_stream.len() as u32;
                plain_stream.extend_from_slice(&f.file_body);
            } else if f.attr & ATTR_BINARY == ATTR_BINARY {
                f.content_offset = bin_stream.len() as u32;
                bin_stream.extend_from_slice(&f.file_body);
            } else {
                bail!(
                    "Unknown SNB file type: attr={:#x} name={}",
                    f.attr,
                    f.file_name
                );
            }
        }

        let mut vfat_and_names = vfat.clone();
        vfat_and_names.extend_from_slice(&file_name_table);
        let vfat_compressed = zlib_compress(&vfat_and_names)?;

        // -- Header ----------------------------------------------------
        out.write_all(MAGIC)?;
        out.write_all(&REV80.to_be_bytes())?;
        out.write_all(&REVA3.to_be_bytes())?;
        out.write_all(&0u32.to_be_bytes())?; // revZ1
        out.write_all(&(self.files.len() as u32).to_be_bytes())?;
        out.write_all(&(vfat_and_names.len() as u32).to_be_bytes())?; // vfatSize
        out.write_all(&(vfat_compressed.len() as u32).to_be_bytes())?; // vfatCompressed
        out.write_all(&(bin_stream.len() as u32).to_be_bytes())?; // binStreamSize
        out.write_all(&(plain_stream.len() as u32).to_be_bytes())?; // plainStreamSizeUncompressed
        out.write_all(&0u32.to_be_bytes())?; // revZ2

        // -- VFAT --------------------------------------------------------
        out.write_all(&vfat_compressed)?;

        // -- Block bookkeeping -------------------------------------------
        let bin_block_offset = HEADER_SIZE + vfat_compressed.len();
        let plain_block_offset = bin_block_offset + bin_stream.len();
        let bin_block = bin_stream.len().div_ceil(BLOCK_SIZE) as u32;

        let mut tail_block = Vec::new();
        let mut offset = 0usize;
        for _ in 0..bin_block {
            tail_block.extend_from_slice(&((bin_block_offset + offset) as u32).to_be_bytes());
            offset += BLOCK_SIZE;
        }

        let mut tail_rec = Vec::new();
        for f in &self.files {
            let t = if f.is_binary() { 0 } else { bin_block };
            let block_idx = f.content_offset / BLOCK_SIZE as u32 + t;
            let block_off = f.content_offset % BLOCK_SIZE as u32;
            tail_rec.extend_from_slice(&block_idx.to_be_bytes());
            tail_rec.extend_from_slice(&block_off.to_be_bytes());
        }

        // -- Binary stream -------------------------------------------------
        out.write_all(&bin_stream)?;

        // -- Plain stream: independently bz2-compressed 0x8000 blocks ------
        let mut pos = 0usize;
        let mut offset = 0usize;
        while pos < plain_stream.len() {
            tail_block.extend_from_slice(&((plain_block_offset + offset) as u32).to_be_bytes());
            let end = (pos + BLOCK_SIZE).min(plain_stream.len());
            let block = &plain_stream[pos..end];
            let compressed = bz2_compress(block)?;
            out.write_all(&compressed)?;
            offset += compressed.len();
            pos += BLOCK_SIZE;
        }

        // -- Tail block ------------------------------------------------------
        let mut tail_payload = tail_block;
        tail_payload.extend_from_slice(&tail_rec);
        let compressed_tail = zlib_compress(&tail_payload)?;
        out.write_all(&compressed_tail)?;

        // -- Tail pointer (8 bytes: size, offset) -----------------------------
        // Port of `veom = struct.pack('>ii', len(compressedTail),
        // plainBlockOffset + offset)`: *not* 16 bytes -- `'>ii'` is
        // only two ints. The remaining 8 bytes of the 16-byte tail
        // pointer `Parse()` reads (`tailMagic`) come from the
        // *separately*-written file-end marker immediately below,
        // which just happens to land right after this.
        out.write_all(&(compressed_tail.len() as u32).to_be_bytes())?;
        out.write_all(&((plain_block_offset + offset) as u32).to_be_bytes())?;

        // -- File-end marker (8 bytes) -- also serves as `tailMagic` -----------
        out.write_all(MAGIC)?;

        Ok(())
    }
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn bz2_compress(data: &[u8]) -> Result<Vec<u8>> {
    use bzip2::write::BzEncoder;
    use bzip2::Compression;
    let mut enc = BzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}
