//! Port of `calibre.utils.exim`'s container-format core (issue #461):
//! a multi-part, hash-verified, append-only archive format used for
//! `calibre-debug --export-library`'s full library export/import.
//!
//! # Scope of this pass
//!
//! Only the genuinely reusable, calibre-specific container primitive
//! is ported: [`Exporter`]/[`FileDest`] (write files into a sequence
//! of `part-NNNN.calibre-data` files, splitting at `part_size`
//! boundaries, tracking each file's `(part, offset, size, sha1)` in a
//! trailing JSON metadata blob) and [`Importer`]/[`FileSource`]/[`Pos`]
//! (the matching random-access reader, including reads that span a
//! part boundary).
//!
//! **Not ported, disclosed**: upstream's `export()`/`import_data()`
//! (the orchestration layer) call into `calibre.db.cache.Cache`'s own
//! `export_library`/`import_library` methods and GUI-specific state
//! (`calibre.gui2.gprefs`, `all_known_libraries`, the global
//! preferences file, `JSONConfig`) that don't exist anywhere in this
//! Rust port yet -- there is no `calibre-debug --export-library`
//! equivalent CLI in this crate to wire this into. Same call as
//! `calibre_utils::smtp`/`icu` this session: port the real,
//! independently-testable primitive; defer the higher-level glue that
//! has no real caller yet.

use anyhow::{bail, Result};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const EXT: &str = ".calibre-data";
/// `struct.calcsize('!II?')` -- part_num (u32) + version (u32) +
/// is_last (1-byte bool), big-endian.
const TAIL_SIZE: u64 = 9;
/// `struct.calcsize('!Q')` -- the trailing metadata-blob length,
/// big-endian u64.
const MDATA_SZ_SIZE: u64 = 8;

fn pack_tail(part_num: u32, is_last: bool) -> [u8; TAIL_SIZE as usize] {
    let mut out = [0u8; TAIL_SIZE as usize];
    out[0..4].copy_from_slice(&part_num.to_be_bytes());
    out[4..8].copy_from_slice(&VERSION.to_be_bytes());
    out[8] = is_last as u8;
    out
}

fn sha1_hex(hasher: Sha1) -> String {
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------- Export

/// One entry in `Exporter`'s `file_metadata` map -- port of the
/// `(start_part_number, start_pos, size, digest, mtime)` tuple.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMetaEntry {
    pub start_part: u32,
    pub start_pos: u64,
    pub size: u64,
    pub digest: String,
    pub mtime: Option<f64>,
}

/// Port of `Exporter`: writes a sequence of files into
/// `part-NNNN.calibre-data` files under `base`, splitting at
/// `part_size` boundaries, and records each file's location + SHA1
/// digest for later random-access retrieval by an [`Importer`].
pub struct Exporter {
    part_size: u64,
    base: PathBuf,
    committed_parts: Vec<PathBuf>,
    current_part: Option<File>,
    file_metadata: Map<String, Value>,
    /// Extra top-level keys (upstream's `set_metadata`/`export_dir`
    /// writing directly into `self.metadata`), e.g. `"libraries"`.
    extra_metadata: Map<String, Value>,
}

/// Default part size (1 GiB), matching upstream's own default.
pub const DEFAULT_PART_SIZE: u64 = 1 << 30;

impl Exporter {
    pub fn new(dest_dir: &Path, part_size: Option<u64>) -> Result<Self> {
        fs::create_dir_all(dest_dir)?;
        Ok(Self {
            part_size: part_size.unwrap_or(DEFAULT_PART_SIZE),
            base: dest_dir.to_path_buf(),
            committed_parts: Vec::new(),
            current_part: None,
            file_metadata: Map::new(),
            extra_metadata: Map::new(),
        })
    }

    /// Port of `set_metadata`: errors if `key` was already set,
    /// matching upstream's own `raise KeyError` guard.
    pub fn set_metadata(&mut self, key: &str, val: Value) -> Result<()> {
        if self.extra_metadata.contains_key(key) || key == "file_metadata" {
            bail!("The metadata already contains the key: {key}");
        }
        self.extra_metadata.insert(key.to_string(), val);
        Ok(())
    }

    fn current_pos(&mut self) -> Result<(u32, u64)> {
        let mut pos = 0u64;
        if let Some(part) = &mut self.current_part {
            pos = part.stream_position()?;
            if pos >= self.part_size.saturating_sub(TAIL_SIZE) {
                self.new_part()?;
                pos = 0;
            }
        }
        Ok((self.committed_parts.len() as u32 + 1, pos))
    }

    fn new_part(&mut self) -> Result<()> {
        self.commit_part(false)?;
        let name = format!("part-{:04}{EXT}", self.committed_parts.len() + 1);
        let path = self.base.join(name);
        self.current_part = Some(File::create(&path)?);
        Ok(())
    }

    fn commit_part(&mut self, is_last: bool) -> Result<()> {
        if let Some(mut part) = self.current_part.take() {
            let part_num = self.committed_parts.len() as u32 + 1;
            part.write_all(&pack_tail(part_num, is_last))?;
            drop(part);
            self.committed_parts.push(self.base.join(format!("part-{part_num:04}{EXT}")));
        }
        Ok(())
    }

    /// Port of `Exporter.write`: writes into the current part,
    /// rolling over to a new part when `part_size` (minus the
    /// trailing tail) would be exceeded.
    pub fn write(&mut self, mut data: &[u8]) -> Result<usize> {
        let mut written = 0usize;
        while !data.is_empty() {
            if self.current_part.is_none() {
                self.new_part()?;
            }
            let cur = self.current_part.as_mut().unwrap();
            let cur_pos = cur.stream_position()?;
            let mut max_size = self.part_size.saturating_sub(TAIL_SIZE).saturating_sub(cur_pos);
            if max_size == 0 {
                self.new_part()?;
                max_size = self.part_size.saturating_sub(TAIL_SIZE);
            }
            let take = (data.len() as u64).min(max_size) as usize;
            let chunk = &data[..take];
            let cur = self.current_part.as_mut().unwrap();
            cur.write_all(chunk)?;
            data = &data[take..];
            written += take;
        }
        Ok(written)
    }

    /// Port of `start_file`/`FileDest`, collapsed into one call
    /// (Rust's borrow checker makes a separate always-must-close
    /// writer object more friction than value here -- see
    /// [`Exporter::add_file`]/[`Exporter::export_dir`] for the two
    /// real callers, both of which just want "write this whole
    /// reader's bytes under this key").
    pub fn add_file(&mut self, reader: &mut impl Read, key: &str, mtime: Option<f64>) -> Result<()> {
        let (start_part, start_pos) = self.current_pos()?;
        let mut hasher = Sha1::new();
        let mut size = 0u64;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            let w = self.write(&buf[..n])?;
            if w != n {
                bail!("Exporter failed to write all data: {n} != {w}");
            }
            size += n as u64;
        }
        let digest = sha1_hex(hasher);
        self.file_metadata.insert(key.to_string(), serde_json::to_value(FileMetaEntry { start_part, start_pos, size, digest, mtime })?);
        Ok(())
    }

    /// Port of `export_dir`: recursively adds every file under `path`,
    /// keyed `"{dir_key}:{relative/path}"` (matching upstream's own
    /// `polyglot.binary.as_hex_unicode(dir_key)` -- narrowed to using
    /// `dir_key` directly since this port has no caller passing a
    /// non-string key, and the hex-encoding is purely upstream's own
    /// key-namespacing choice, not a wire-format requirement).
    pub fn export_dir(&mut self, path: &Path, dir_key: &str) -> Result<()> {
        let mut files: Vec<Value> = Vec::new();
        for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(path)?.to_string_lossy().replace('\\', "/");
            let key = format!("{dir_key}:{rel}");
            let mtime = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64());
            let mut f = File::open(entry.path())?;
            self.add_file(&mut f, &key, mtime)?;
            files.push(Value::Array(vec![Value::String(key), Value::String(rel)]));
        }
        self.extra_metadata.insert(dir_key.to_string(), Value::Array(files));
        Ok(())
    }

    /// Port of `commit`: writes the JSON metadata blob (as its own
    /// pseudo-"file", never split across parts even if `part_size` is
    /// small -- matching upstream's own `part_size = sys.maxsize`
    /// trick) followed by its length, then finalizes the last part.
    pub fn commit(mut self) -> Result<()> {
        let mut metadata = self.extra_metadata.clone();
        metadata.insert("file_metadata".to_string(), Value::Object(self.file_metadata.clone()));
        let raw = serde_json::to_vec(&Value::Object(metadata))?;

        self.new_part()?;
        let orig_part_size = self.part_size;
        self.part_size = u64::MAX;
        self.write(&raw)?;
        self.write(&(raw.len() as u64).to_be_bytes())?;
        self.part_size = orig_part_size;
        self.commit_part(true)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- Import

/// Port of `Importer.__init__`'s directory scan: validates every
/// `part-NNNN.calibre-data` file's own trailing tail, part-number
/// contiguity, and exactly one `is_last` part, then reads the
/// trailing JSON metadata blob out of the last part.
#[derive(Debug)]
pub struct Importer {
    part_paths: std::collections::HashMap<u32, PathBuf>,
    part_sizes: std::collections::HashMap<u32, u64>,
    pub metadata: Value,
}

impl Importer {
    pub fn new(export_dir: &Path) -> Result<Self> {
        let mut part_map: std::collections::HashMap<u32, (PathBuf, bool, u64)> = std::collections::HashMap::new();
        let mut version: i64 = -1;

        for entry in fs::read_dir(export_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().ends_with(EXT) || name.starts_with("._") {
                continue;
            }
            let path = entry.path();
            let size_of_part = fs::metadata(&path)?.len();
            let mut f = File::open(&path)?;
            if size_of_part < TAIL_SIZE {
                bail!("The exported data in {name} is not valid, tail too small");
            }
            f.seek(SeekFrom::End(-(TAIL_SIZE as i64)))?;
            let mut tail = [0u8; TAIL_SIZE as usize];
            f.read_exact(&mut tail)?;
            let part_num = u32::from_be_bytes(tail[0..4].try_into().unwrap());
            let part_version = u32::from_be_bytes(tail[4..8].try_into().unwrap());
            let is_last = tail[8] != 0;
            if part_version > VERSION {
                bail!("The exported data in {name} is not valid, version ({part_version}) is higher than maximum supported version. You might need to upgrade calibre-oxide first.");
            }
            if version == -1 {
                version = part_version as i64;
            }
            if part_version as i64 != version {
                bail!("The exported data in {name} is not valid as it contains a mix of parts with versions: {version} and {part_version}");
            }
            part_map.insert(part_num, (path, is_last, size_of_part));
        }

        let mut nums: Vec<u32> = part_map.keys().copied().collect();
        nums.sort_unstable();
        if nums.is_empty() {
            bail!("No exported data found in: {}", export_dir.display());
        }
        if nums[0] != 1 {
            bail!("The first part of this exported data set is missing");
        }
        let last_num = *nums.last().unwrap();
        if !part_map[&last_num].1 {
            bail!("The last part of this exported data set is missing");
        }
        if nums.len() as u32 != last_num {
            bail!("There are some parts of the exported data set missing");
        }

        let mut part_paths = std::collections::HashMap::new();
        let mut part_sizes = std::collections::HashMap::new();
        for (num, (path, _is_last, size)) in part_map {
            part_paths.insert(num, path);
            part_sizes.insert(num, size);
        }

        let offset = TAIL_SIZE + MDATA_SZ_SIZE;
        let mut last = File::open(&part_paths[&last_num])?;
        last.seek(SeekFrom::End(-(offset as i64)))?;
        let mut sz_buf = [0u8; MDATA_SZ_SIZE as usize];
        last.read_exact(&mut sz_buf)?;
        let sz = u64::from_be_bytes(sz_buf);
        last.seek(SeekFrom::End(-((sz + offset) as i64)))?;
        let mut raw = vec![0u8; sz as usize];
        last.read_exact(&mut raw)?;
        let metadata: Value = serde_json::from_slice(&raw)?;

        Ok(Self { part_paths, part_sizes, metadata })
    }

    fn size_of_part(&self, num: u32) -> u64 {
        self.part_sizes.get(&num).copied().unwrap_or(0).saturating_sub(TAIL_SIZE)
    }

    fn open_part(&self, num: u32) -> Result<File> {
        Ok(File::open(&self.part_paths[&num])?)
    }

    /// Looks up one file's `(part, offset, size, digest, mtime)` from
    /// `metadata.file_metadata`, matching upstream's own
    /// `self.file_metadata[key]`.
    fn file_meta(&self, key: &str) -> Result<FileMetaEntry> {
        let entry = self.metadata.get("file_metadata").and_then(|m| m.get(key)).ok_or_else(|| anyhow::anyhow!("No such exported file: {key}"))?;
        Ok(serde_json::from_value(entry.clone())?)
    }

    /// Port of `Importer.start_file`: a random-access, hash-verifying
    /// reader for one exported file.
    pub fn start_file(&self, key: &str) -> Result<FileSource<'_>> {
        let meta = self.file_meta(key)?;
        Ok(FileSource::new(meta, self))
    }

    /// Port of `Importer.save_file`: extract one file to `output_path`.
    pub fn save_file(&self, key: &str, output_path: &Path) -> Result<bool> {
        let mut src = self.start_file(key)?;
        let mut dest = File::create(output_path)?;
        std::io::copy(&mut src, &mut dest)?;
        Ok(src.finish())
    }
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    part_num: u32,
    pos_in_part: u64,
    size: u64,
    pos_in_file: u64,
}

/// Port of `Pos`: a byte range that may span several parts,
/// pre-resolved into a list of per-part chunks once at construction.
struct Pos<'a> {
    size: u64,
    pos_in_file: u64,
    chunks: Vec<Chunk>,
    importer: &'a Importer,
    open: Option<(u32, File)>,
}

impl<'a> Pos<'a> {
    fn new(start_part: u32, start_pos: u64, size: u64, importer: &'a Importer) -> Self {
        let mut chunks = Vec::new();
        let mut part = start_part;
        let mut pos_in_part = start_pos;
        let mut remaining = size;
        let mut pos = 0u64;
        while remaining > 0 {
            let part_size = importer.size_of_part(part);
            let chunk_size = remaining.min(part_size.saturating_sub(pos_in_part));
            if chunk_size > 0 {
                chunks.push(Chunk { part_num: part, pos_in_part, size: chunk_size, pos_in_file: pos });
                remaining -= chunk_size;
                pos += chunk_size;
            }
            part += 1;
            pos_in_part = 0;
        }
        Self { size, pos_in_file: 0, chunks, importer, open: None }
    }

    fn seek(&mut self, pos: i64, whence: SeekFrom) -> u64 {
        let new_pos = match whence {
            SeekFrom::Start(_) => pos,
            SeekFrom::End(_) => self.size as i64 + pos,
            SeekFrom::Current(_) => self.pos_in_file as i64 + pos,
        };
        self.pos_in_file = new_pos.max(0).min(self.size as i64) as u64;
        self.pos_in_file
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let amt_left = self.size.saturating_sub(self.pos_in_file);
        let amt_to_read = (buf.len() as u64).min(amt_left) as usize;
        if amt_to_read == 0 {
            return Ok(0);
        }
        let Some(mut chunk_idx) = self.chunks.iter().position(|c| c.pos_in_file <= self.pos_in_file && self.pos_in_file < c.pos_in_file + c.size) else {
            bail!("No chunk found containing pos_in_file={}", self.pos_in_file);
        };
        let mut written = 0usize;
        while written < amt_to_read {
            let Some(&chunk) = self.chunks.get(chunk_idx) else { break };
            let n = self.read_chunk(&chunk, amt_to_read - written, &mut buf[written..amt_to_read])?;
            if n == 0 {
                break;
            }
            written += n;
            chunk_idx += 1;
        }
        Ok(written)
    }

    fn read_chunk(&mut self, chunk: &Chunk, want: usize, out: &mut [u8]) -> Result<usize> {
        let need_reopen = match &self.open {
            Some((num, _)) => *num != chunk.part_num,
            None => true,
        };
        if need_reopen {
            self.open = Some((chunk.part_num, self.importer.open_part(chunk.part_num)?));
        }
        let (_, file) = self.open.as_mut().unwrap();
        let offset_from_chunk_start = self.pos_in_file - chunk.pos_in_file;
        file.seek(SeekFrom::Start(chunk.pos_in_part + offset_from_chunk_start))?;
        let size = want.min((chunk.size - offset_from_chunk_start) as usize);
        let n = file.read(&mut out[..size])?;
        self.pos_in_file += n as u64;
        Ok(n)
    }
}

/// Port of `FileSource`: a `Read`+`Seek` view over one exported
/// file's bytes, verifying its SHA1 digest against what [`Exporter`]
/// recorded once fully read. Call [`FileSource::finish`] after use to
/// get the verification result -- matching upstream's own
/// "record as corrupted, don't raise" behavior (see this module's own
/// doc) rather than erroring mid-read.
pub struct FileSource<'a> {
    pos: Pos<'a>,
    hasher: Sha1,
    expected_digest: String,
    pub size: u64,
    pub mtime: Option<f64>,
}

impl<'a> FileSource<'a> {
    fn new(meta: FileMetaEntry, importer: &'a Importer) -> Self {
        Self { pos: Pos::new(meta.start_part, meta.start_pos, meta.size, importer), hasher: Sha1::new(), expected_digest: meta.digest, size: meta.size, mtime: meta.mtime }
    }

    /// Returns `true` if every byte read so far hashes to the
    /// recorded digest (only meaningful after reading the whole
    /// file -- matches upstream's own end-of-read check).
    pub fn finish(self) -> bool {
        sha1_hex(self.hasher) == self.expected_digest
    }
}

impl Read for FileSource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.pos.read(buf).map_err(std::io::Error::other)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

impl Seek for FileSource<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        Ok(match pos {
            SeekFrom::Start(p) => self.pos.seek(p as i64, SeekFrom::Start(0)),
            SeekFrom::End(p) => self.pos.seek(p, SeekFrom::End(0)),
            SeekFrom::Current(p) => self.pos.seek(p, SeekFrom::Current(0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_single_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), None).unwrap();
        exporter.add_file(&mut &b"hello world"[..], "greeting", Some(12345.0)).unwrap();
        exporter.commit().unwrap();

        let importer = Importer::new(dir.path()).unwrap();
        let mut src = importer.start_file("greeting").unwrap();
        assert_eq!(src.size, 11);
        assert_eq!(src.mtime, Some(12345.0));
        let mut buf = Vec::new();
        src.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"hello world");
        assert!(src.finish(), "digest should verify for an intact export");
    }

    #[test]
    fn round_trips_multiple_files_and_extra_top_level_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), None).unwrap();
        exporter.add_file(&mut &b"AAA"[..], "a", None).unwrap();
        exporter.add_file(&mut &b"BBBBBBBBBB"[..], "b", None).unwrap();
        exporter.set_metadata("libraries", serde_json::json!({"lib1": 5})).unwrap();
        exporter.commit().unwrap();

        let importer = Importer::new(dir.path()).unwrap();
        assert_eq!(importer.metadata["libraries"]["lib1"], 5);
        let mut a = Vec::new();
        importer.start_file("a").unwrap().read_to_end(&mut a).unwrap();
        assert_eq!(a, b"AAA");
        let mut b = Vec::new();
        importer.start_file("b").unwrap().read_to_end(&mut b).unwrap();
        assert_eq!(b, b"BBBBBBBBBB");
    }

    #[test]
    fn set_metadata_rejects_a_duplicate_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), None).unwrap();
        exporter.set_metadata("x", serde_json::json!(1)).unwrap();
        assert!(exporter.set_metadata("x", serde_json::json!(2)).is_err());
    }

    #[test]
    fn a_file_spanning_several_parts_reads_back_correctly_with_a_tiny_part_size() {
        // Forces frequent part rollover so a single file's bytes are
        // guaranteed to span 3+ parts -- exercises Pos's chunk-
        // spanning logic, the trickiest part of this port.
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), Some(32)).unwrap();
        let data: Vec<u8> = (0u16..2000).map(|i| (i % 256) as u8).collect();
        exporter.add_file(&mut &data[..], "big", None).unwrap();
        exporter.commit().unwrap();

        let part_count = fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(EXT)).count();
        assert!(part_count >= 3, "test setup assumption: expected several parts, got {part_count}");

        let importer = Importer::new(dir.path()).unwrap();
        let mut src = importer.start_file("big").unwrap();
        let mut buf = Vec::new();
        src.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, data);
        assert!(src.finish());
    }

    #[test]
    fn seeking_within_a_multi_part_file_lands_on_the_correct_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), Some(32)).unwrap();
        let data: Vec<u8> = (0u16..500).map(|i| (i % 256) as u8).collect();
        exporter.add_file(&mut &data[..], "f", None).unwrap();
        exporter.commit().unwrap();

        let importer = Importer::new(dir.path()).unwrap();
        let mut src = importer.start_file("f").unwrap();
        src.seek(SeekFrom::Start(300)).unwrap();
        let mut buf = [0u8; 50];
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[300..350]);
    }

    #[test]
    fn detects_a_corrupted_file_via_digest_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), None).unwrap();
        exporter.add_file(&mut &b"original bytes"[..], "f", None).unwrap();
        exporter.commit().unwrap();

        // Corrupt one byte inside part-0001 (well before the tail).
        let part_path = dir.path().join("part-0001.calibre-data");
        let mut bytes = fs::read(&part_path).unwrap();
        bytes[0] ^= 0xFF;
        fs::write(&part_path, &bytes).unwrap();

        let importer = Importer::new(dir.path()).unwrap();
        let mut src = importer.start_file("f").unwrap();
        let mut buf = Vec::new();
        src.read_to_end(&mut buf).unwrap();
        assert!(!src.finish(), "corrupted data should fail digest verification");
    }

    #[test]
    fn export_dir_recursively_captures_every_file_with_relative_paths() {
        let src_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(src_dir.path().join("sub")).unwrap();
        fs::write(src_dir.path().join("top.txt"), b"top").unwrap();
        fs::write(src_dir.path().join("sub/nested.txt"), b"nested").unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(out_dir.path(), None).unwrap();
        exporter.export_dir(src_dir.path(), "config_dir").unwrap();
        exporter.commit().unwrap();

        let importer = Importer::new(out_dir.path()).unwrap();
        let entries = importer.metadata["config_dir"].as_array().unwrap();
        let rels: Vec<String> = entries.iter().map(|e| e[1].as_str().unwrap().to_string()).collect();
        assert!(rels.contains(&"top.txt".to_string()));
        assert!(rels.contains(&"sub/nested.txt".to_string()));

        let mut buf = Vec::new();
        importer.start_file("config_dir:top.txt").unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"top");
    }

    #[test]
    fn importer_rejects_a_directory_with_no_exported_parts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Importer::new(dir.path()).is_err());
    }

    #[test]
    fn importer_rejects_a_directory_missing_the_last_part() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), Some(32)).unwrap();
        let data: Vec<u8> = (0u16..500).map(|i| (i % 256) as u8).collect();
        exporter.add_file(&mut &data[..], "f", None).unwrap();
        exporter.commit().unwrap();

        // Delete the highest-numbered (is_last) part.
        let mut parts: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().ends_with(EXT)).collect();
        parts.sort();
        fs::remove_file(parts.last().unwrap()).unwrap();

        let err = Importer::new(dir.path()).unwrap_err();
        assert!(err.to_string().contains("last part"), "got: {err}");
    }

    #[test]
    fn save_file_writes_the_correct_bytes_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut exporter = Exporter::new(dir.path(), None).unwrap();
        exporter.add_file(&mut &b"disk contents"[..], "f", None).unwrap();
        exporter.commit().unwrap();

        let importer = Importer::new(dir.path()).unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        let ok = importer.save_file("f", out.path()).unwrap();
        assert!(ok);
        assert_eq!(fs::read(out.path()).unwrap(), b"disk contents");
    }
}
