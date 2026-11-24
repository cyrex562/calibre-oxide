//! Port of `calibre.utils.localunzip` (issue #469): a fallback ZIP
//! reader for files with a missing or damaged central directory --
//! historically produced in quantity by certain Barnes & Noble
//! devices. Reads only the local file headers (each entry's own
//! self-contained header immediately before its data), scanning
//! forward past any damaged/misaligned bytes to find the next one,
//! rather than trusting the end-of-file central directory index the
//! way a normal ZIP reader (e.g. the `zip` crate) requires.
//!
//! # Disclosed narrowing vs. upstream
//!
//! - `decode_arcname`'s non-UTF-8, non-ASCII fallback chain uses
//!   `chardet`-based charset detection in Python. This port tries
//!   UTF-8 (which also covers plain ASCII) and falls back to lossy
//!   UTF-8 on failure, rather than running charset detection -- a
//!   rare edge case (a legacy corrupted ZIP with non-UTF-8 filenames)
//!   for an already-low-priority fallback path.
//! - `LocalHeader` doesn't keep the raw `extra` field (upstream's own
//!   namedtuple does, but nothing in this crate consumes it).
//! - `safe_replace` (rewriting a damaged ZIP's contents in place via
//!   a real ZIP writer) is not ported -- no caller in this crate
//!   needs to *repair* a damaged ZIP yet, only read one. Add it if a
//!   real caller needs it.
//! - The zip-bomb guard is reshaped, not just translated: upstream
//!   caps the number of *consecutive* `decompress()` calls per 20KB
//!   input chunk (a heuristic against extreme expansion ratios).
//!   This port instead caps total decompressed output at
//!   `MAX_DECOMPRESSED_MULTIPLE` times the compressed size (minimum
//!   [`MIN_DECOMPRESSED_CAP`]), a simpler, equally real bound on the
//!   same failure mode.
//! - Windows-reserved-filename detection (`CON`/`PRN`/etc, only
//!   relevant when actually extracting to a Windows filesystem) is
//!   not ported -- this crate already targets multiple platforms via
//!   plain `std::fs`, and no other port in this crate has needed that
//!   check either.

use anyhow::{bail, Result};
use flate2::read::DeflateDecoder;
use indexmap::IndexMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEADER_SIG: u32 = 0x04034b50;
const DATA_DESCRIPTOR_SIG: u32 = 0x08074b50;
const FIXED_HEADER_SIZE: usize = 30;
const ZIP_STORED: u16 = 0;
const ZIP_DEFLATED: u16 = 8;
const SCAN_CHUNK: usize = 50 * 1024;
/// A compression ratio past which decompression is treated as a zip
/// bomb and aborted -- see this module's doc for how this differs
/// from (and improves on) upstream's own heuristic.
const MAX_DECOMPRESSED_MULTIPLE: u64 = 1024;
const MIN_DECOMPRESSED_CAP: u64 = 64 * 1024 * 1024;

/// A single entry's local header, resolved (streamed size/crc fields,
/// if any, already filled in from the trailing data descriptor).
#[derive(Debug, Clone)]
pub struct LocalHeader {
    pub min_version: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub mod_time: u16,
    pub mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub filename: String,
}

impl LocalHeader {
    fn has_data_descriptor(&self) -> bool {
        self.flags & (1 << 3) != 0
    }
}

/// The fixed 30-byte portion of a local header, before the
/// variable-length filename/extra fields.
struct FixedHeader {
    min_version: u16,
    flags: u16,
    compression_method: u16,
    mod_time: u16,
    mod_date: u16,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    filename_length: u16,
    extra_length: u16,
}

fn decode_arcname(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        // Upstream re-tries UTF-8 (when the language-encoding flag is
        // set) then falls back to chardet detection; the strict
        // from_utf8 above already covers the flagged case, so
        // anything left really is non-UTF-8 -- see module doc for why
        // this port takes the lossy decode instead of running charset
        // detection.
        Err(_) => String::from_utf8_lossy(raw).into_owned(),
    }
}

/// Reads exactly [`FIXED_HEADER_SIZE`] bytes at the reader's current
/// position and parses them if they match the signature; otherwise
/// restores the position and returns `None`.
fn try_parse_fixed_header<R: Read + Seek>(f: &mut R) -> Result<Option<FixedHeader>> {
    let pos = f.stream_position()?;
    let mut raw = [0u8; FIXED_HEADER_SIZE];
    if f.read(&mut raw)? != FIXED_HEADER_SIZE {
        f.seek(SeekFrom::Start(pos))?;
        return Ok(None);
    }
    let signature = u32::from_le_bytes(raw[0..4].try_into().unwrap());
    if signature != HEADER_SIG {
        f.seek(SeekFrom::Start(pos))?;
        return Ok(None);
    }
    Ok(Some(FixedHeader {
        min_version: u16::from_le_bytes(raw[4..6].try_into().unwrap()),
        flags: u16::from_le_bytes(raw[6..8].try_into().unwrap()),
        compression_method: u16::from_le_bytes(raw[8..10].try_into().unwrap()),
        mod_time: u16::from_le_bytes(raw[10..12].try_into().unwrap()),
        mod_date: u16::from_le_bytes(raw[12..14].try_into().unwrap()),
        crc32: u32::from_le_bytes(raw[14..18].try_into().unwrap()),
        compressed_size: u32::from_le_bytes(raw[18..22].try_into().unwrap()),
        uncompressed_size: u32::from_le_bytes(raw[22..26].try_into().unwrap()),
        filename_length: u16::from_le_bytes(raw[26..28].try_into().unwrap()),
        extra_length: u16::from_le_bytes(raw[28..30].try_into().unwrap()),
    }))
}

/// Port of `find_local_header`: scan forward (in one bounded chunk,
/// matching upstream) for the next local-header signature, used to
/// resync past damaged bytes between entries.
fn find_local_header<R: Read + Seek>(f: &mut R) -> Result<Option<FixedHeader>> {
    let pos = f.stream_position()?;
    let mut raw = vec![0u8; SCAN_CHUNK];
    let n = f.read(&mut raw)?;
    raw.truncate(n);
    let sig_bytes = HEADER_SIG.to_le_bytes();
    let Some(idx) = raw.windows(4).position(|w| w == sig_bytes) else {
        f.seek(SeekFrom::Start(pos))?;
        return Ok(None);
    };
    f.seek(SeekFrom::Start(pos + idx as u64))?;
    try_parse_fixed_header(f)
}

struct DataDescriptor {
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
}

/// Port of `find_data_descriptor`: scan forward for the data
/// descriptor signature (present when the "streaming" flag bit is
/// set, meaning the local header's own size/crc fields are zeroed and
/// the real values follow the entry's compressed data instead).
/// Always restores the reader's position, matching upstream's own
/// `finally: f.seek(pos)`.
fn find_data_descriptor<R: Read + Seek>(f: &mut R) -> Result<DataDescriptor> {
    let pos = f.stream_position()?;
    let result = (|| -> Result<DataDescriptor> {
        let sig_bytes = DATA_DESCRIPTOR_SIG.to_le_bytes();
        loop {
            let chunk_start = f.stream_position()?;
            let mut raw = vec![0u8; SCAN_CHUNK];
            let n = f.read(&mut raw)?;
            raw.truncate(n);
            if raw.len() < 16 {
                bail!("Failed to find data descriptor signature. Data descriptors without signatures are not supported.");
            }
            if let Some(idx) = raw.windows(4).position(|w| w == sig_bytes) {
                f.seek(SeekFrom::Start(chunk_start + idx as u64 + 4))?;
                let mut dd = [0u8; 12];
                f.read_exact(&mut dd)?;
                return Ok(DataDescriptor {
                    crc32: u32::from_le_bytes(dd[0..4].try_into().unwrap()),
                    compressed_size: u32::from_le_bytes(dd[4..8].try_into().unwrap()),
                    uncompressed_size: u32::from_le_bytes(dd[8..12].try_into().unwrap()),
                });
            }
            // Rewind so a signature straddling this chunk boundary
            // isn't missed by the next iteration's read.
            f.seek(SeekFrom::Start(chunk_start + raw.len() as u64 - 4))?;
        }
    })();
    f.seek(SeekFrom::Start(pos))?;
    result
}

/// Port of `read_local_file_header`: parse one full local header
/// (fixed portion + filename + extra field), resolving streamed
/// (data-descriptor) size/crc fields if the streaming flag is set.
/// Returns `Ok(None)` at a clean end of data (no more headers found).
fn read_local_file_header<R: Read + Seek>(f: &mut R) -> Result<Option<LocalHeader>> {
    let pos = f.stream_position()?;
    let fixed = match try_parse_fixed_header(f)? {
        Some(h) => h,
        None => {
            f.seek(SeekFrom::Start(pos))?;
            match find_local_header(f)? {
                Some(h) => h,
                None => return Ok(None),
            }
        }
    };
    if fixed.min_version > 20 {
        bail!("This ZIP file uses unsupported features");
    }
    if fixed.flags & 0b1 != 0 {
        bail!("This ZIP file is encrypted");
    }
    if fixed.flags & (1 << 13) != 0 {
        bail!("This ZIP file uses masking, unsupported.");
    }
    if fixed.compression_method != ZIP_STORED && fixed.compression_method != ZIP_DEFLATED {
        bail!("This ZIP file uses an unsupported compression method");
    }

    let mut filename = String::new();
    if fixed.filename_length > 0 {
        let mut raw = vec![0u8; fixed.filename_length as usize];
        if f.read(&mut raw)? != raw.len() {
            return Ok(None);
        }
        filename = decode_arcname(&raw).replace('\\', "/");
    }
    if fixed.extra_length > 0 {
        let mut raw = vec![0u8; fixed.extra_length as usize];
        if f.read(&mut raw)? != raw.len() {
            return Ok(None);
        }
    }

    let (crc32, compressed_size, uncompressed_size) = if fixed.flags & (1 << 3) != 0 {
        let dd = find_data_descriptor(f)?;
        (dd.crc32, dd.compressed_size, dd.uncompressed_size)
    } else {
        (fixed.crc32, fixed.compressed_size, fixed.uncompressed_size)
    };

    Ok(Some(LocalHeader {
        min_version: fixed.min_version,
        flags: fixed.flags,
        compression_method: fixed.compression_method,
        mod_time: fixed.mod_time,
        mod_date: fixed.mod_date,
        crc32,
        compressed_size,
        uncompressed_size,
        filename,
    }))
}

fn copy_stored<R: Read, W: Write>(src: &mut R, size: u32, dest: &mut W) -> Result<()> {
    let mut limited = src.take(size as u64);
    let n = std::io::copy(&mut limited, dest)?;
    if n != size as u64 {
        bail!("Premature end of file");
    }
    Ok(())
}

/// Port of `copy_compressed_file`: raw-deflate (no zlib header)
/// decompress exactly `compressed_size` bytes of input. See module
/// doc for how the zip-bomb guard here differs from upstream's.
fn copy_compressed<R: Read, W: Write>(src: &mut R, compressed_size: u32, dest: &mut W) -> Result<()> {
    let mut compressed = vec![0u8; compressed_size as usize];
    let mut read = 0usize;
    while read < compressed.len() {
        let n = src.read(&mut compressed[read..])?;
        if n == 0 {
            bail!("Invalid ZIP file, local header is damaged");
        }
        read += n;
    }
    let cap = (compressed_size as u64 * MAX_DECOMPRESSED_MULTIPLE).max(MIN_DECOMPRESSED_CAP);
    let decoder = DeflateDecoder::new(&compressed[..]);
    let mut limited = decoder.take(cap + 1);
    let mut out = Vec::new();
    limited.read_to_end(&mut out)?;
    if out.len() as u64 > cap {
        bail!("This ZIP file contains a zip bomb");
    }
    dest.write_all(&out)?;
    Ok(())
}

fn copy_entry<R: Read, W: Write>(src: &mut R, header: &LocalHeader, dest: &mut W) -> Result<()> {
    if header.compression_method == ZIP_STORED {
        copy_stored(src, header.compressed_size, dest)
    } else {
        copy_compressed(src, header.compressed_size, dest)
    }
}

/// Sanitizes an archive-internal path the same way upstream's
/// `_extractall` does: normalize `\` to `/`, drop any drive letter,
/// and filter out empty/`.`/`..` components -- a real path-traversal
/// guard (this project has a documented precedent for taking this
/// seriously, see the srv `/conversion/start` path-traversal fix,
/// issue #429/#497), not just upstream fidelity.
fn sanitize_parts(filename: &str) -> Vec<String> {
    let normalized = filename.replace('\\', "/");
    let without_drive = normalized.split_once(':').map(|(_, rest)| rest).unwrap_or(&normalized);
    without_drive.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").map(|s| s.to_string()).collect()
}

/// Core scan loop shared by every public entry point: walks every
/// local header from the reader's current position, optionally
/// extracting file contents to `dest_dir` and/or recording each
/// entry's (filename, header, data-offset) into `collect_info`.
fn scan<R: Read + Seek>(f: &mut R, dest_dir: Option<&Path>, mut collect_info: Option<&mut IndexMap<String, (u64, LocalHeader)>>) -> Result<()> {
    let mut found_any = false;
    loop {
        let Some(header) = read_local_file_header(f)? else { break };
        found_any = true;
        let data_offset = f.stream_position()?;
        let seek_forward = header.compressed_size as u64 + if header.has_data_descriptor() { 16 } else { 0 };

        let parts = sanitize_parts(&header.filename);
        if parts.is_empty() {
            f.seek(SeekFrom::Start(data_offset + seek_forward))?;
            continue;
        }

        if header.uncompressed_size == 0 {
            // Directory entry.
            if let Some(dir) = dest_dir {
                fs::create_dir_all(dir.join(parts.join("/")))?;
            }
            f.seek(SeekFrom::Start(data_offset + seek_forward))?;
            if let Some(info) = collect_info.as_deref_mut() {
                info.insert(header.filename.clone(), (data_offset, header));
            }
            continue;
        }

        if let Some(dir) = dest_dir {
            let rel: PathBuf = parts.iter().collect();
            let dest_path = dir.join(&rel);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&dest_path)?;
            copy_entry(f, &header, &mut out)?;
        } else {
            f.seek(SeekFrom::Start(data_offset + seek_forward))?;
        }

        if let Some(info) = collect_info.as_deref_mut() {
            info.insert(header.filename.clone(), (data_offset, header));
        }
    }
    if !found_any {
        bail!("Not a ZIP file");
    }
    Ok(())
}

/// Port of `extractall`: extract every recoverable entry from a
/// stream into `dest` (creating it if it's a fresh reader position;
/// the reader's position is restored afterward, matching upstream).
pub fn extract_all<R: Read + Seek>(f: &mut R, dest: &Path) -> Result<()> {
    let pos = f.stream_position()?;
    let result = scan(f, Some(dest), None);
    f.seek(SeekFrom::Start(pos))?;
    result
}

/// Convenience wrapper matching upstream's `extractall(path_or_stream, path)`
/// taking a filesystem path to the damaged ZIP directly.
pub fn extract_all_from_path(zip_path: &Path, dest: &Path) -> Result<()> {
    let mut f = fs::File::open(zip_path)?;
    extract_all(&mut f, dest)
}

/// Port of `LocalZipFile`: an in-memory index of every recoverable
/// entry's local header and byte offset, built once, allowing
/// individual entries to be read back out on demand without
/// re-scanning the whole stream each time.
#[derive(Debug)]
pub struct LocalZipFile<R> {
    stream: R,
    file_info: IndexMap<String, (u64, LocalHeader)>,
}

impl<R: Read + Seek> LocalZipFile<R> {
    pub fn new(mut stream: R) -> Result<Self> {
        let mut file_info = IndexMap::new();
        scan(&mut stream, None, Some(&mut file_info))?;
        Ok(Self { stream, file_info })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.file_info.keys().map(|s| s.as_str())
    }

    pub fn getinfo(&self, name: &str) -> Result<&LocalHeader> {
        self.file_info.get(name).map(|(_, h)| h).ok_or_else(|| anyhow::anyhow!("This ZIP container has no file named: {name}"))
    }

    /// Port of `LocalZipFile.read`: extract one entry's decompressed
    /// contents into memory.
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let (offset, header) = self.file_info.get(name).ok_or_else(|| anyhow::anyhow!("This ZIP container has no file named: {name}"))?.clone();
        self.stream.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::new();
        copy_entry(&mut self.stream, &header, &mut out)?;
        Ok(out)
    }

    /// Port of `LocalZipFile.extractall`: re-scan and extract every
    /// entry from the beginning of the stream (matching upstream,
    /// which reseeks to 0 rather than reusing the already-collected
    /// `file_info` offsets).
    pub fn extract_all(&mut self, dest: &Path) -> Result<()> {
        self.stream.seek(SeekFrom::Start(0))?;
        scan(&mut self.stream, Some(dest), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn local_header_bytes(flags: u16, method: u16, crc32: u32, compressed_size: u32, uncompressed_size: u32, filename: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&HEADER_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // min_version
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod_time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod_date
        out.extend_from_slice(&crc32.to_le_bytes());
        out.extend_from_slice(&compressed_size.to_le_bytes());
        out.extend_from_slice(&uncompressed_size.to_le_bytes());
        out.extend_from_slice(&(filename.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra_length
        out.extend_from_slice(filename.as_bytes());
        out
    }

    fn stored_entry(filename: &str, data: &[u8]) -> Vec<u8> {
        let crc = crc32fast_stub(data);
        let mut out = local_header_bytes(0, ZIP_STORED, crc, data.len() as u32, data.len() as u32, filename);
        out.extend_from_slice(data);
        out
    }

    fn deflated_entry(filename: &str, data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        let crc = crc32fast_stub(data);
        let mut out = local_header_bytes(0, ZIP_DEFLATED, crc, compressed.len() as u32, data.len() as u32, filename);
        out.extend_from_slice(&compressed);
        out
    }

    /// A tiny standalone CRC32 (IEEE 802.3 polynomial) so this test
    /// module doesn't need its own crc32 dependency -- correctness of
    /// the CRC value itself isn't exercised by any of these tests
    /// (this port doesn't validate crc32 on read, matching upstream,
    /// which also never checks it), only its presence in the header.
    fn crc32fast_stub(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn streamed_entry(filename: &str, data: &[u8]) -> Vec<u8> {
        // Streaming (data-descriptor) mode: header's own size/crc are
        // zeroed, the real values follow the compressed data.
        let mut out = local_header_bytes(1 << 3, ZIP_STORED, 0, 0, 0, filename);
        out.extend_from_slice(data);
        out.extend_from_slice(&DATA_DESCRIPTOR_SIG.to_le_bytes());
        out.extend_from_slice(&crc32fast_stub(data).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn reads_a_stored_entry_with_no_central_directory_at_all() {
        let mut bytes = stored_entry("hello.txt", b"hello world");
        // No central directory follows -- that's the whole point.
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        assert_eq!(zip.names().collect::<Vec<_>>(), vec!["hello.txt"]);
        assert_eq!(zip.read("hello.txt").unwrap(), b"hello world");
    }

    #[test]
    fn reads_a_deflated_entry() {
        let data = b"the quick brown fox jumps over the lazy dog, repeated. ".repeat(20);
        let mut bytes = deflated_entry("book.txt", &data);
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        assert_eq!(zip.read("book.txt").unwrap(), data);
    }

    #[test]
    fn reads_a_streamed_entry_with_a_trailing_data_descriptor() {
        let mut bytes = streamed_entry("stream.txt", b"streamed data");
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        let header = zip.getinfo("stream.txt").unwrap();
        assert_eq!(header.uncompressed_size, 13);
        assert_eq!(zip.read("stream.txt").unwrap(), b"streamed data");
    }

    #[test]
    fn reads_multiple_entries_in_order() {
        let mut bytes = stored_entry("a.txt", b"AAA");
        bytes.extend(stored_entry("b.txt", b"BBBBB"));
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        assert_eq!(zip.names().collect::<Vec<_>>(), vec!["a.txt", "b.txt"]);
        assert_eq!(zip.read("a.txt").unwrap(), b"AAA");
        assert_eq!(zip.read("b.txt").unwrap(), b"BBBBB");
    }

    #[test]
    fn resyncs_past_garbage_bytes_preceding_the_first_real_header() {
        // Simulates the actual B&N damage scenario: bytes before the
        // first entry are corrupted, but the entry itself is intact
        // and findable by scanning forward.
        let mut bytes = vec![0xDEu8; 4096];
        bytes.extend(stored_entry("recovered.txt", b"still here"));
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        assert_eq!(zip.read("recovered.txt").unwrap(), b"still here");
    }

    #[test]
    fn errors_with_not_a_zip_file_when_no_header_is_found_anywhere() {
        let mut bytes = vec![0xAAu8; 1024];
        let err = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap_err();
        assert!(err.to_string().contains("Not a ZIP file"));
    }

    #[test]
    fn extract_all_creates_real_files_on_disk_with_correct_contents() {
        let mut bytes = stored_entry("out.txt", b"disk contents");
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = Cursor::new(&mut bytes);
        extract_all(&mut cursor, dir.path()).unwrap();
        let written = std::fs::read(dir.path().join("out.txt")).unwrap();
        assert_eq!(written, b"disk contents");
    }

    #[test]
    fn extract_all_creates_empty_directories_for_zero_size_entries() {
        let mut bytes = local_header_bytes(0, ZIP_STORED, 0, 0, 0, "empty_dir/");
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = Cursor::new(&mut bytes);
        extract_all(&mut cursor, dir.path()).unwrap();
        assert!(dir.path().join("empty_dir").is_dir());
    }

    #[test]
    fn path_traversal_components_are_stripped_not_followed() {
        // A malicious/corrupted entry name shouldn't be able to escape
        // the extraction directory -- see this module's own doc on
        // why this is treated as a real security property, not just
        // upstream fidelity.
        let mut bytes = stored_entry("../../etc/evil.txt", b"pwned");
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = Cursor::new(&mut bytes);
        extract_all(&mut cursor, dir.path()).unwrap();
        assert!(!dir.path().parent().unwrap().join("etc").exists(), "traversal escaped the extraction directory");
        assert_eq!(std::fs::read(dir.path().join("etc").join("evil.txt")).unwrap(), b"pwned", "sanitized path should still land inside the dest dir");
    }

    #[test]
    fn getinfo_errors_for_an_unknown_name() {
        let mut bytes = stored_entry("only.txt", b"x");
        let zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        assert!(zip.getinfo("missing.txt").is_err());
    }

    #[test]
    fn rejects_an_encrypted_entry() {
        let mut bytes = local_header_bytes(0b1, ZIP_STORED, 0, 3, 3, "secret.txt");
        bytes.extend_from_slice(b"abc");
        let err = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap_err();
        assert!(err.to_string().contains("encrypted"));
    }

    #[test]
    fn a_massively_expanding_deflate_stream_is_rejected_as_a_zip_bomb() {
        use std::io::Write as _;
        // A long run of zero bytes compresses extremely well -- a
        // tiny compressed_size expanding to well past this module's
        // cap is exactly the shape a real zip-bomb attack takes.
        let huge = vec![0u8; 200 * 1024 * 1024];
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&huge).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 1024 * 1024, "test setup assumption: compressed size should stay small, got {}", compressed.len());

        let mut bytes = local_header_bytes(0, ZIP_DEFLATED, 0, compressed.len() as u32, huge.len() as u32, "bomb.bin");
        bytes.extend_from_slice(&compressed);
        let mut zip = LocalZipFile::new(Cursor::new(&mut bytes)).unwrap();
        let err = zip.read("bomb.bin").unwrap_err();
        assert!(err.to_string().contains("zip bomb"), "expected a zip-bomb error, got: {err}");
    }
}
