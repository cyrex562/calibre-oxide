//! Port of `old_src/src/calibre/srv/books.py`'s disk-cache layer for
//! rendered book output (issue #482, part of #427's tracking epic):
//! `book_hash`/`books_cache_dir`/`abspath`/`safe_remove`/
//! `clean_final`/`rename_with_retry`. Independent of what actually
//! produces the cached content (#481's territory) or the HTTP
//! endpoints that serve it (#483's) -- this module only manages the
//! cache's own directory lifecycle and content-hash keying, and is
//! real, tested infrastructure built ahead of both, the same "build
//! real, tested, standalone infra first" pattern already used for
//! `library_broker`/`jobs` (#423/#428).
//!
//! # Layout
//!
//! `{base}/s` (staging, in-flight renders) and `{base}/f` (finished
//! renders, one subdirectory per content hash) -- `base` defaults to
//! `calibre_utils::constants::cache_dir().join("srvb")` via
//! [`BookCache::open_default`], matching upstream's own
//! `books_cache_dir()`, but every real method here takes `&self`
//! against an explicit `base` a caller supplies to [`BookCache::new`],
//! so tests use a real `tempdir()` instead of the process-wide
//! default.
//!
//! # Not byte-compatible with upstream's own hash values
//!
//! [`book_hash`] hashes the same six logical inputs upstream's own
//! `book_hash` does (`library_uuid`, `book_id`, `fmt`, `size`,
//! `mtime`, [`RENDER_VERSION`]), but via `serde_json`'s own array
//! serialization rather than Python's `json.dumps` -- the exact bytes
//! hashed can differ (separator/escaping details). That's fine: this
//! cache is entirely internal to this Rust server and never needs to
//! reproduce Python's own hash value for the same book, only to be a
//! real, deterministic, collision-resistant function of the same
//! inputs.
//!
//! # Not ported: `abspath`'s Windows long-path prefix behavior
//!
//! Upstream's `abspath` prefixes with `\\?\` on Windows to opt out of
//! the legacy `MAX_PATH` limit. Kept behind `#[cfg(windows)]` here,
//! matching upstream's own platform gate exactly -- untested on this
//! (Linux) development machine, disclosed rather than silently
//! dropped.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// Bumped whenever the (not-yet-ported) render pipeline's own output
/// shape changes, to invalidate every existing cache entry -- matches
/// upstream's `render_book.RENDER_VERSION`. #481 should reference
/// this constant once it exists rather than defining its own.
pub const RENDER_VERSION: u32 = 1;

/// The manifest file name a finished render's hash directory holds --
/// [`BookCache::clean_final`]'s own reaping sweep uses this file's
/// mtime as the "last accessed" signal, matching upstream
/// (`book_manifest`'s own `os.utime` touches this exact file on a
/// cache hit).
pub const MANIFEST_FILENAME: &str = "calibre-book-manifest.json";

/// Port of `abspath`.
pub fn abspath(path: &Path) -> io::Result<PathBuf> {
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    #[cfg(windows)]
    {
        let s = abs.to_string_lossy();
        if !s.starts_with(r"\\?\") {
            return Ok(PathBuf::from(format!(r"\\?\{s}")));
        }
    }
    Ok(abs)
}

/// Port of `book_hash`: a content-hash cache key for one book
/// format's rendered output, changing whenever the underlying format
/// file is re-imported (`size`/`mtime` change), the library changes
/// (`library_uuid`), or the render pipeline's own output shape
/// changes ([`RENDER_VERSION`]).
pub fn book_hash(library_uuid: &str, book_id: i32, fmt: &str, size: i64, mtime: i64) -> String {
    let key = serde_json::json!([library_uuid, book_id, fmt.to_uppercase(), size, mtime, RENDER_VERSION]);
    let mut hasher = Sha256::new();
    hasher.update(key.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Port of `safe_remove`: removes a file or directory tree, ignoring
/// any error (matches upstream's own best-effort cleanup -- a cache
/// entry that fails to delete is a disk-space nag, not a correctness
/// problem, and a path that's already gone isn't an error either).
pub fn safe_remove(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Port of `rename_with_retry`. On non-Windows this is exactly
/// `std::fs::rename` -- upstream itself only retries on Windows
/// (`if iswindows: retry else: raise`), so a permission error
/// propagates immediately here too, same as upstream on Linux/macOS.
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            std::thread::sleep(Duration::from_secs(1));
            std::fs::rename(from, to)
        }
        Err(e) => Err(e),
    }
}

/// Port of `books_cache_dir`'s directory layout + `clean_final`, as a
/// real, testable type -- upstream's own module-level globals
/// (`_books_cache_dir`, `last_final_clean_time`) become `self`/caller
/// state instead.
pub struct BookCache {
    base: PathBuf,
}

impl BookCache {
    /// Opens (creating if needed) the `s`/`f` subdirectories under
    /// `base`.
    pub fn new(base: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(base.join("s"))?;
        std::fs::create_dir_all(base.join("f"))?;
        Ok(Self { base })
    }

    /// `base` = `calibre_utils::constants::cache_dir().join("srvb")`,
    /// matching upstream's own `books_cache_dir()` default location.
    pub fn open_default() -> io::Result<Self> {
        Self::new(calibre_utils::constants::cache_dir().join("srvb"))
    }

    /// A throwaway cache backed by a real, process-lifetime temp
    /// directory -- for tests (mirrors `ProfileStore::new_in_memory`'s
    /// role; there's no true in-memory option here since this cache is
    /// inherently filesystem-based).
    pub fn open_temp() -> Self {
        let base = tempfile::tempdir().expect("failed to create a temp dir for BookCache::open_temp").keep();
        Self::new(base).expect("failed to initialize a temp BookCache")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.base.join("s")
    }

    pub fn final_dir(&self) -> PathBuf {
        self.base.join("f")
    }

    /// The hash-named directory a finished render for `hash` lives
    /// (or would live) in.
    pub fn hash_dir(&self, hash: &str) -> PathBuf {
        self.final_dir().join(hash)
    }

    /// Port of `clean_final`: removes every entry under the `f`
    /// directory whose [`MANIFEST_FILENAME`] is older than `interval`
    /// -- an entry with no manifest at all is left alone (matches
    /// upstream's own `except OSError: continue`, not a delete).
    /// Upstream calls this with its own module-level last-run
    /// timestamp to rate-limit the sweep to once per `interval`
    /// server-wide; that rate-limiting is a caller concern (#483's,
    /// once it exists), not this method's own -- every call here
    /// really sweeps.
    pub fn clean_final(&self, interval: Duration) -> io::Result<()> {
        let now = SystemTime::now();
        let entries = match std::fs::read_dir(self.final_dir()) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let manifest = entry.path().join(MANIFEST_FILENAME);
            let Ok(meta) = std::fs::metadata(&manifest) else { continue };
            let Ok(modified) = meta.modified() else { continue };
            let Ok(age) = now.duration_since(modified) else { continue };
            if age >= interval {
                safe_remove(&entry.path());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_hash_is_deterministic_for_the_same_inputs() {
        let a = book_hash("lib-uuid", 1, "epub", 100, 200);
        let b = book_hash("lib-uuid", 1, "epub", 100, 200);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "sha256 hex digest is 64 chars");
    }

    #[test]
    fn book_hash_normalizes_format_case() {
        let lower = book_hash("lib-uuid", 1, "epub", 100, 200);
        let upper = book_hash("lib-uuid", 1, "EPUB", 100, 200);
        assert_eq!(lower, upper);
    }

    #[test]
    fn book_hash_changes_when_any_input_changes() {
        let base = book_hash("lib-uuid", 1, "epub", 100, 200);
        assert_ne!(base, book_hash("other-uuid", 1, "epub", 100, 200), "library_uuid");
        assert_ne!(base, book_hash("lib-uuid", 2, "epub", 100, 200), "book_id");
        assert_ne!(base, book_hash("lib-uuid", 1, "azw3", 100, 200), "fmt");
        assert_ne!(base, book_hash("lib-uuid", 1, "epub", 101, 200), "size");
        assert_ne!(base, book_hash("lib-uuid", 1, "epub", 100, 201), "mtime");
    }

    #[test]
    fn new_creates_the_staging_and_final_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("srvb");
        let cache = BookCache::new(base.clone()).unwrap();
        assert!(cache.staging_dir().is_dir());
        assert!(cache.final_dir().is_dir());
        assert_eq!(cache.staging_dir(), base.join("s"));
        assert_eq!(cache.final_dir(), base.join("f"));
    }

    #[test]
    fn new_is_idempotent_against_an_already_populated_base() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("srvb");
        BookCache::new(base.clone()).unwrap();
        // Should not error the second time even though both
        // subdirectories already exist.
        BookCache::new(base).unwrap();
    }

    #[test]
    fn hash_dir_is_under_the_final_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = BookCache::new(dir.path().join("srvb")).unwrap();
        assert_eq!(cache.hash_dir("abc123"), cache.final_dir().join("abc123"));
    }

    #[test]
    fn safe_remove_deletes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, b"hi").unwrap();
        safe_remove(&path);
        assert!(!path.exists());
    }

    #[test]
    fn safe_remove_deletes_a_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("d");
        std::fs::create_dir_all(sub.join("nested")).unwrap();
        std::fs::write(sub.join("nested/x.txt"), b"hi").unwrap();
        safe_remove(&sub);
        assert!(!sub.exists());
    }

    #[test]
    fn safe_remove_does_not_panic_on_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        safe_remove(&dir.path().join("does-not-exist"));
    }

    #[test]
    fn rename_with_retry_renames_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        std::fs::write(&from, b"hi").unwrap();
        rename_with_retry(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "hi");
    }

    #[test]
    fn rename_with_retry_renames_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("staged");
        let to = dir.path().join("final");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join(MANIFEST_FILENAME), b"{}").unwrap();
        rename_with_retry(&from, &to).unwrap();
        assert!(to.join(MANIFEST_FILENAME).is_file());
    }

    fn seed_hash_entry(cache: &BookCache, hash: &str, manifest_age: Duration) {
        let dir = cache.hash_dir(hash);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(MANIFEST_FILENAME);
        std::fs::write(&manifest, b"{}").unwrap();
        let stamp = filetime::FileTime::from_system_time(SystemTime::now() - manifest_age);
        filetime::set_file_mtime(&manifest, stamp).unwrap();
    }

    #[test]
    fn clean_final_removes_entries_older_than_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let cache = BookCache::new(dir.path().join("srvb")).unwrap();
        seed_hash_entry(&cache, "stale", Duration::from_secs(2 * 24 * 60 * 60));
        seed_hash_entry(&cache, "fresh", Duration::from_secs(60));

        cache.clean_final(Duration::from_secs(24 * 60 * 60)).unwrap();

        assert!(!cache.hash_dir("stale").exists());
        assert!(cache.hash_dir("fresh").exists());
    }

    #[test]
    fn clean_final_leaves_an_entry_with_no_manifest_alone() {
        let dir = tempfile::tempdir().unwrap();
        let cache = BookCache::new(dir.path().join("srvb")).unwrap();
        std::fs::create_dir_all(cache.hash_dir("no-manifest")).unwrap();

        cache.clean_final(Duration::from_secs(0)).unwrap();

        assert!(cache.hash_dir("no-manifest").exists(), "matches upstream: a getmtime failure is skipped, not deleted");
    }

    #[test]
    fn clean_final_is_a_no_op_on_a_freshly_created_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = BookCache::new(dir.path().join("srvb")).unwrap();
        cache.clean_final(Duration::from_secs(0)).unwrap();
    }

    #[test]
    fn abspath_of_a_relative_path_is_absolute() {
        let out = abspath(Path::new("relative/path")).unwrap();
        assert!(out.is_absolute());
        assert!(out.ends_with("relative/path"));
    }

    #[test]
    fn abspath_of_an_already_absolute_path_is_unchanged_on_non_windows() {
        let dir = tempfile::tempdir().unwrap();
        let out = abspath(dir.path()).unwrap();
        assert_eq!(out, dir.path());
    }
}
