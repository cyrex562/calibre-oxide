use lazy_static::lazy_static;
use std::collections::HashSet;
use std::path::Path;
use unidecode::unidecode;

lazy_static! {
    static ref INVALID_CHARS: HashSet<char> = {
        let mut s = HashSet::new();
        // '\\', '|', '?', '*', '<', '"', ':', '>', '+', '/'
        for c in ['\\', '|', '?', '*', '<', '"', ':', '>', '+', '/'] {
            s.insert(c);
        }
        // control chars 0-31
        for i in 0..32 {
            if let Some(c) = std::char::from_u32(i) {
                s.insert(c);
            }
        }
        s
    };
}

pub fn ascii_text(orig: &str) -> String {
    unidecode(orig)
}

pub fn ascii_filename(orig: &str) -> String {
    let text = ascii_text(orig).replace('?', "_");
    // Replace invalid ascii chars (control chars already substituted by sanitization?)
    // But ascii_filename specifically does:
    // ans = ''.join(x if ord(x) >= 32 else substitute for x in orig)
    // and then calls sanitize_file_name.

    let substitute = '_';
    let filtered: String = text
        .chars()
        .map(|c| if (c as u32) >= 32 { c } else { substitute })
        .collect();
    sanitize_file_name(&filtered)
}

pub fn sanitize_file_name(name: &str) -> String {
    let substitute = '_';
    let mut chars = String::with_capacity(name.len());

    for c in name.chars() {
        if INVALID_CHARS.contains(&c) {
            chars.push(substitute);
        } else {
            chars.push(c);
        }
    }

    // Replace whitespace with space and strip
    // Replaces all whitespace with space?
    // Python: one = re.sub(r'\s', ' ', one).strip()
    // This replaces tabs/newlines with space.
    let one = chars.replace(['\t', '\n', '\r'], " "); // simpler regex equivalent
    let mut one = one.trim().to_string();

    // Split ext
    let path = Path::new(&one);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // If stem matches ^\.+$ -> _
    // i.e. stem is only dots.
    if stem.chars().all(|c| c == '.') && !stem.is_empty() {
        one = "_".to_string(); // stem becomes _
    } else {
        one = stem;
    }

    // one = one.replace("..", substitute)
    one = one.replace("..", "_");

    if !ext.is_empty() {
        one.push('.');
        one.push_str(&ext);
    } else if chars.ends_with('.') {
        // path.file_stem logic trims trailing dot?
        // Check original split logic.
        // Rust Path: "foo." -> stem "foo", ext "".
        // So reconstruction loses dot.
        // Python os.path.splitext("foo.") -> ("foo", "") on Linux?
        // No, ("foo.", "") usually?
        // Let's rely on manual split for exact parity if needed.
        // Python: bname, ext = os.path.splitext(one)
    }

    // Windows checks: ends with . or space
    if one.ends_with('.') || one.ends_with(' ') {
        one.pop();
        one.push('_');
    }

    // Leading dot -> _
    if one.starts_with('.') {
        one.insert(0, '_');
    }

    one
}

pub fn shorten_component(s: &str, by_what: usize) -> String {
    let len = s.len();
    if len < by_what {
        return s.to_string();
    }
    let l = (len - by_what) as isize / 2;
    if l <= 0 {
        return s.to_string();
    }
    let _l = l as usize;
    // s[:l] + s[-l:]
    // Be careful with unicode boundaries!
    // Python works on codepoints (str) or bytes? Str.
    // Rust chars.
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < by_what {
        return s.to_string();
    }
    let l_chars = (chars.len().saturating_sub(by_what)) / 2;
    if l_chars == 0 {
        return s.to_string();
    }

    let mut res = String::new();
    res.extend(&chars[..l_chars]);
    res.extend(&chars[chars.len() - l_chars..]);
    res
}

pub fn limit_component(x: &str, limit: usize) -> String {
    // UTF-8 length used for now.
    let mut s = x.to_string();
    while s.len() > limit {
        let delta = s.len() - limit;
        s = shorten_component(&s, std::cmp::max(2, delta / 2));
    }
    s
}

/// Check if two paths point to the same actual file on the filesystem.
///
/// Port of `calibre.utils.filenames.samefile`. On Unix this is exactly
/// `os.path.samefile`: both paths must exist and resolve to the same
/// file.
///
/// Unix compares the `(dev, ino)` pair. Windows asks for the
/// equivalent `(volume serial, file index)` via
/// `GetFileInformationByHandle`, falling back to a case-insensitive
/// comparison of canonicalized paths for anything that cannot be
/// opened as a file -- directories, most usefully. The Python only
/// ever had the path-string fallback; the handle query is stricter,
/// and needed, because the fallback cannot see two hard links to one
/// file as the same file.
///
/// Returns `false` (rather than erroring) when either path doesn't
/// exist, matching the Python's `os.path.samefile` behavior of raising
/// `OSError` — which every caller in `worker.py` treats as "not the
/// same file".
pub fn samefile(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
            return false;
        };
        ma.dev() == mb.dev() && ma.ino() == mb.ino()
    }
    #[cfg(windows)]
    {
        // Ask the filesystem for the real identity first. A
        // canonicalized-path comparison cannot see that two different
        // names are hard links to one file, and `copy_files` uses this
        // to decide whether to skip a copy -- a false "different"
        // there means calling `fs::copy` with a source and destination
        // that are the same bytes on disk.
        if let (Some(ia), Some(ib)) = (windows_file_identity(a), windows_file_identity(b)) {
            return ia == ib;
        }
        // Directories, or anything we could not open, fall back to the
        // path comparison this used to do everywhere.
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca
                .to_string_lossy()
                .eq_ignore_ascii_case(&cb.to_string_lossy()),
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca
                .to_string_lossy()
                .eq_ignore_ascii_case(&cb.to_string_lossy()),
            _ => false,
        }
    }
}

/// `(volume serial, file index)` -- Windows' answer to a `(dev, ino)`
/// pair, and the only way to tell two hard links to one file apart
/// from two genuinely separate files.
///
/// `None` when the path cannot be opened as a file at all, which
/// includes every directory: opening one needs
/// `FILE_FLAG_BACKUP_SEMANTICS`, which [`std::fs::File::open`] does not
/// pass. Callers fall back to comparing canonical paths there.
#[cfg(windows)]
fn windows_file_identity(path: &Path) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    let file = std::fs::File::open(path).ok()?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // Safety: `file` owns a live handle for the duration of the call,
    // and `info` is a correctly-sized, writable output buffer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return None;
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((info.dwVolumeSerialNumber, index))
}


/// A file's identity as the filesystem knows it, independent of what it
/// is called or where it sits.
///
/// `(dev, ino)` on Unix, `(volume serial, file index)` on Windows --
/// the same concept under two names. Two paths with equal identities are
/// one file; a file that moved or was renamed keeps its identity, which
/// is what makes this the cheapest possible rename detector: one `stat`,
/// no reading of content.
///
/// Only comparable **within one volume**. `volume` is included so that
/// two files with the same inode number on different mounts are not
/// mistaken for each other, but an identity does not survive a file
/// being copied to another disk -- that needs the content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileIdentity {
    pub volume: u64,
    pub index: u64,
}

/// What one `stat` can tell us about a tracked file.
///
/// `size` and `mtime` together are the cheap "has this changed?" gate:
/// when both match what was recorded, the content is assumed unchanged
/// and rehashing is skipped. That assumption is wrong for an edit that
/// preserves both, which is why it gates rehashing rather than
/// replacing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub identity: FileIdentity,
    pub size: u64,
    /// Milliseconds since the Unix epoch, or `None` where the platform
    /// or filesystem does not report one.
    pub mtime_ms: Option<u64>,
}

impl FileFacts {
    /// Whether the content can be assumed unchanged since these facts
    /// were recorded.
    ///
    /// Deliberately conservative: an unknown mtime on either side
    /// answers "no", so a filesystem that does not report modification
    /// times gets rehashed rather than silently trusted.
    pub fn looks_unchanged_from(&self, recorded: &FileFacts) -> bool {
        match (self.mtime_ms, recorded.mtime_ms) {
            (Some(a), Some(b)) => self.size == recorded.size && a == b,
            _ => false,
        }
    }
}

/// Reads a file's identity and stat facts in one go.
///
/// `None` for anything that cannot be opened or stat'd, which includes
/// directories on Windows -- opening one needs
/// `FILE_FLAG_BACKUP_SEMANTICS`, which `std::fs::File::open` does not
/// pass. Callers wanting to compare directories fall back to canonical
/// paths.
pub fn file_facts(path: &Path) -> Option<FileFacts> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    Some(FileFacts { identity: identity_of(path, &metadata)?, size: metadata.len(), mtime_ms })
}

/// Just the identity, for callers that already have the size elsewhere.
pub fn identity(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    identity_of(path, &metadata)
}

#[cfg(unix)]
fn identity_of(_path: &Path, metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(FileIdentity { volume: metadata.dev(), index: metadata.ino() })
}

#[cfg(windows)]
fn identity_of(path: &Path, _metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    // Windows exposes this only through an open handle, not through
    // `Metadata` -- hence the path parameter that the Unix arm ignores.
    windows_file_identity(path).map(|(volume, index)| FileIdentity { volume: u64::from(volume), index })
}

#[cfg(not(any(unix, windows)))]
fn identity_of(_path: &Path, _metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    None
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn a_file_has_an_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, b"content").unwrap();

        let facts = file_facts(&path).expect("a real file has facts");
        assert_eq!(facts.size, 7);
        assert!(facts.mtime_ms.is_some());
    }

    #[test]
    fn a_missing_file_has_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(file_facts(&dir.path().join("nope.pdf")).is_none());
        assert!(identity(&dir.path().join("nope.pdf")).is_none());
    }

    /// The property that makes this the cheapest rename detector there
    /// is: the identity is a property of the file, not of its name.
    #[test]
    fn renaming_a_file_keeps_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let before = dir.path().join("before.pdf");
        std::fs::write(&before, b"content").unwrap();
        let original = identity(&before).unwrap();

        let after = dir.path().join("after.pdf");
        std::fs::rename(&before, &after).unwrap();

        assert_eq!(identity(&after).unwrap(), original);
    }

    #[test]
    fn moving_a_file_within_a_volume_keeps_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let before = dir.path().join("book.pdf");
        std::fs::write(&before, b"content").unwrap();
        let original = identity(&before).unwrap();

        let after = sub.join("book.pdf");
        std::fs::rename(&before, &after).unwrap();
        assert_eq!(identity(&after).unwrap(), original);
    }

    /// A copy is a different file even with identical bytes -- which is
    /// exactly why the content hash is still needed alongside this.
    #[test]
    fn a_copy_is_a_different_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        std::fs::write(&a, b"identical").unwrap();
        std::fs::copy(&a, &b).unwrap();

        assert_ne!(identity(&a).unwrap(), identity(&b).unwrap());
    }

    #[test]
    fn two_files_have_different_identities() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"two").unwrap();
        assert_ne!(identity(&a).unwrap(), identity(&b).unwrap());
    }

    #[test]
    fn unchanged_size_and_mtime_reads_as_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, b"content").unwrap();

        let first = file_facts(&path).unwrap();
        let second = file_facts(&path).unwrap();
        assert!(second.looks_unchanged_from(&first));
    }

    #[test]
    fn a_different_size_reads_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, b"content").unwrap();
        let before = file_facts(&path).unwrap();

        std::fs::write(&path, b"content and more").unwrap();
        let after = file_facts(&path).unwrap();
        assert!(!after.looks_unchanged_from(&before));
    }

    /// Conservative on purpose: a filesystem that reports no
    /// modification time gets rehashed rather than silently trusted.
    #[test]
    fn an_unknown_mtime_never_reads_as_unchanged() {
        let known = FileFacts { identity: FileIdentity { volume: 1, index: 2 }, size: 10, mtime_ms: Some(5) };
        let unknown = FileFacts { mtime_ms: None, ..known };
        assert!(!unknown.looks_unchanged_from(&known));
        assert!(!known.looks_unchanged_from(&unknown));
        assert!(!unknown.looks_unchanged_from(&unknown));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samefile_is_true_for_a_path_and_itself() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        assert!(samefile(tmp.path(), tmp.path()));
    }

    #[test]
    fn samefile_is_true_for_a_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"content").expect("write");
        std::fs::hard_link(&a, &b).expect("hardlink");
        assert!(samefile(&a, &b));
    }

    #[test]
    fn samefile_is_false_for_two_distinct_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"content").expect("write");
        std::fs::write(&b, b"content").expect("write");
        assert!(!samefile(&a, &b));
    }

    #[test]
    fn samefile_is_false_when_a_path_does_not_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("real.txt");
        std::fs::write(&a, b"content").expect("write");
        let missing = dir.path().join("missing.txt");
        assert!(!samefile(&a, &missing));
        assert!(!samefile(&missing, &a));
    }
}
