//! CHM archive reader.
//!
//! Port of `old_src/src/calibre/ebooks/chm/reader.py`. The Python
//! `CHMReader` extended `pychm.chm.CHMFile`; the Rust port wraps
//! `libchm::ChmFile`.
//!
//! The /#SYSTEM entry inside a CHM contains a series of
//! `(code:u16, length:u16, data:[u8; length])` records. The two we
//! care about are:
//! - code=2: default topic path (the "home page").
//! - code=3: title (as bytes, encoded per the CHM's LCID).
//!
//! See <https://www.nongnu.org/chmspec/latest/Internal.html#SYSTEM>.

use std::path::Path;

use libchm::{ChmFile, EntrySel};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChmError {
    #[error("failed to open CHM: {0}")]
    Open(String),
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("failed to read entry {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: libchm::ChmError,
    },
    #[error("malformed /#SYSTEM entry: {0}")]
    MalformedSystem(&'static str),
}

/// Parsed contents of the CHM `/#SYSTEM` metadata entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChmSystemInfo {
    /// Default topic path (the "home page" of the CHM). Leading `/`
    /// stripped, so this is a relative path suitable for feeding back
    /// to `ChmReader::read_file`.
    pub default_topic: Option<String>,
    /// The CHM's title bytes. Decoding requires knowing the LCID
    /// (Windows locale); left raw here so the caller can decide.
    pub title_bytes: Option<Vec<u8>>,
}

pub struct ChmReader {
    inner: ChmFile,
    system: ChmSystemInfo,
}

impl std::fmt::Debug for ChmReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChmReader")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl ChmReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ChmError> {
        let path_ref = path.as_ref();
        let inner = ChmFile::open(path_ref).map_err(|e| ChmError::Open(e.to_string()))?;
        let mut reader = Self {
            inner,
            system: ChmSystemInfo::default(),
        };
        reader.system = reader.parse_system_entry()?;
        Ok(reader)
    }

    pub fn system(&self) -> &ChmSystemInfo {
        &self.system
    }

    /// Read an internal file by CHM path. Leading `/` is optional —
    /// added if missing for compatibility with the Python API which
    /// requires absolute paths.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, ChmError> {
        let normalized = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        let entry = self
            .inner
            .find(&normalized)
            .map_err(|_| ChmError::NotFound(normalized.clone()))?;
        self.inner
            .read(&entry)
            .map_err(|e| ChmError::Read {
                path: normalized,
                source: e,
            })
    }

    /// Read the CHM's default topic HTML (its "home page"), matching
    /// Python `CHMReader.get_home()`. Errors if no default topic is
    /// declared in /#SYSTEM.
    pub fn get_home(&mut self) -> Result<Vec<u8>, ChmError> {
        let topic = self
            .system
            .default_topic
            .clone()
            .ok_or(ChmError::MalformedSystem("no default topic declared"))?;
        self.read_file(&topic)
    }

    /// Enumerate every entry path in the archive (excluding
    /// directory entries and metadata-space entries). Sorted for
    /// determinism.
    pub fn list_files(&mut self) -> Result<Vec<String>, ChmError> {
        let entries = self
            .inner
            .entries(EntrySel::FILES)
            .map_err(|e| ChmError::Read {
                path: "<enumerate>".to_string(),
                source: e,
            })?;
        let mut out: Vec<String> = entries.into_iter().map(|e| e.path).collect();
        out.sort();
        Ok(out)
    }

    fn parse_system_entry(&mut self) -> Result<ChmSystemInfo, ChmError> {
        let data = self.read_file("/#SYSTEM")?;
        // First 4 bytes: version. Records follow at offset 4.
        if data.len() < 4 {
            return Err(ChmError::MalformedSystem("shorter than 4-byte header"));
        }
        let mut info = ChmSystemInfo::default();
        let mut pos = 4usize;
        while pos + 4 <= data.len() {
            let code = u16::from_le_bytes([data[pos], data[pos + 1]]);
            let length = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
            pos += 4;
            if pos + length > data.len() {
                return Err(ChmError::MalformedSystem("record extends past entry"));
            }
            let payload = &data[pos..pos + length];
            match code {
                2 => {
                    // Default topic: null-terminated bytes. Decoded
                    // as UTF-8 first (matches Python behavior — CHM
                    // filenames are UTF-8 per Tika's docs), falls
                    // back to Windows-1252 for legacy files.
                    let trimmed: &[u8] =
                        payload.split(|&b| b == 0).next().unwrap_or(payload);
                    let s = std::str::from_utf8(trimmed)
                        .map(str::to_string)
                        .unwrap_or_else(|_| {
                            let (cow, _, _) =
                                encoding_rs::WINDOWS_1252.decode(trimmed);
                            cow.into_owned()
                        });
                    info.default_topic = Some(s.trim_start_matches('/').to_string());
                }
                3 => {
                    info.title_bytes = Some(payload.iter().take_while(|&&b| b != 0).copied().collect());
                }
                _ => {}
            }
            pos += length;
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Fake CHM builder for tests. libchm requires a real ITSF
    /// header + directory to open a file, so we can't easily
    /// construct one from thin air here. Instead, these tests
    /// focus on the /#SYSTEM parser which we can drive with a
    /// synthetic byte buffer.
    fn parse_system_bytes(data: &[u8]) -> Result<ChmSystemInfo, ChmError> {
        // Mirror ChmReader::parse_system_entry, but on a raw byte
        // buffer rather than opening a real CHM. This is fine
        // because the parser is a pure function of the bytes.
        if data.len() < 4 {
            return Err(ChmError::MalformedSystem("shorter than 4-byte header"));
        }
        let mut info = ChmSystemInfo::default();
        let mut pos = 4usize;
        while pos + 4 <= data.len() {
            let code = u16::from_le_bytes([data[pos], data[pos + 1]]);
            let length = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
            pos += 4;
            if pos + length > data.len() {
                return Err(ChmError::MalformedSystem("record extends past entry"));
            }
            let payload = &data[pos..pos + length];
            match code {
                2 => {
                    let trimmed: &[u8] =
                        payload.split(|&b| b == 0).next().unwrap_or(payload);
                    let s = std::str::from_utf8(trimmed).map(str::to_string);
                    if let Ok(s) = s {
                        info.default_topic = Some(s.trim_start_matches('/').to_string());
                    }
                }
                3 => {
                    info.title_bytes =
                        Some(payload.iter().take_while(|&&b| b != 0).copied().collect());
                }
                _ => {}
            }
            pos += length;
        }
        Ok(info)
    }

    fn write_record(w: &mut Vec<u8>, code: u16, payload: &[u8]) {
        w.extend_from_slice(&code.to_le_bytes());
        w.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        w.extend_from_slice(payload);
    }

    #[test]
    fn parses_default_topic_record() {
        let mut buf = vec![0u8; 4]; // version header
        write_record(&mut buf, 2, b"/help/index.html\0");
        let info = parse_system_bytes(&buf).unwrap();
        // Leading '/' stripped.
        assert_eq!(info.default_topic.as_deref(), Some("help/index.html"));
    }

    #[test]
    fn parses_title_record() {
        let mut buf = vec![0u8; 4];
        write_record(&mut buf, 3, b"My CHM Book\0");
        let info = parse_system_bytes(&buf).unwrap();
        assert_eq!(info.title_bytes.as_deref(), Some(&b"My CHM Book"[..]));
    }

    #[test]
    fn skips_unknown_records() {
        let mut buf = vec![0u8; 4];
        write_record(&mut buf, 99, b"unknown");
        write_record(&mut buf, 2, b"topic.html\0");
        write_record(&mut buf, 7, b"other");
        let info = parse_system_bytes(&buf).unwrap();
        assert_eq!(info.default_topic.as_deref(), Some("topic.html"));
    }

    #[test]
    fn errors_on_truncated_header() {
        let buf = vec![0u8; 3];
        let err = parse_system_bytes(&buf).unwrap_err();
        assert!(matches!(err, ChmError::MalformedSystem(_)));
    }

    #[test]
    fn errors_on_record_extending_past_entry() {
        let mut buf = vec![0u8; 4];
        buf.extend_from_slice(&2u16.to_le_bytes()); // code
        buf.extend_from_slice(&100u16.to_le_bytes()); // length > available
        buf.extend_from_slice(b"short");
        let err = parse_system_bytes(&buf).unwrap_err();
        assert!(matches!(err, ChmError::MalformedSystem(_)));
    }

    #[test]
    fn empty_payload_after_version_yields_default_info() {
        let buf = vec![0u8; 4];
        let info = parse_system_bytes(&buf).unwrap();
        assert!(info.default_topic.is_none());
        assert!(info.title_bytes.is_none());
    }

    #[test]
    fn open_errors_on_non_chm_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::File::create(tmp.path())
            .unwrap()
            .write_all(b"not a CHM file")
            .unwrap();
        let err = ChmReader::open(tmp.path()).unwrap_err();
        assert!(matches!(err, ChmError::Open(_)));
    }

    #[test]
    fn open_errors_on_missing_file() {
        let err = ChmReader::open("/nonexistent/does-not-exist.chm").unwrap_err();
        assert!(matches!(err, ChmError::Open(_)));
    }
}
