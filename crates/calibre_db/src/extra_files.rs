//! Port of `Cache.list_extra_files`/`add_extra_files`/`remove_extra_files`
//! (`old_src/src/calibre/db/cache.py`, backed by `db/backend.py`'s
//! `iter_extra_files`/`add_extra_file`/`remove_extra_files`) --
//! arbitrary files attached to a book's own directory outside its
//! standard formats (e.g. a companion PDF, or anything under the
//! book's `data/` subdirectory).
//!
//! # Scope of this pass
//!
//! Real: `list_extra_files` (walks the book's directory, or a glob
//! `pattern` if given, skipping the cover/metadata/format files
//! themselves), `add_extra_files` (write new files, replacing an
//! existing one only when `replace` is set), `remove_extra_files`
//! (delete by relpath, always permanent -- see below).
//!
//! Not ported: `rename_extra_files`/`merge_extra_files` (no HTTP
//! endpoint under issue #418 needs them), the non-permanent
//! "move to Recycle Bin" deletion mode (`recycle()`, a desktop-OS
//! concept with no server-side equivalent -- this port's
//! `remove_extra_files` always deletes permanently, matching what
//! `content.py`'s own `remove_data_files` endpoint always requests
//! anyway: `db.remove_extra_files(book_id, relpaths, permanent=True)`),
//! and [`list_extra_files`]'s format-file exclusion for an
//! unrestricted (no `/`) `pattern` (`iter_extra_files`'s `if '/' not
//! in pattern: known_files.add(...)` branch) -- every real HTTP
//! endpoint under issue #418 always passes
//! `calibre_db::constants::DATA_FILE_PATTERN` (`"data/**/*"`, which
//! contains a `/`), so upstream's own format-exclusion branch never
//! runs for them either; only a caller passing an unrestricted
//! pattern directly would notice the gap.
//!
//! # Path safety
//!
//! `relpath` is client-controlled (an HTTP path segment or JSON key)
//! and can legitimately contain subdirectories (`data/foo/bar.pdf`),
//! so it can't just be run through `sanitize_file_name` the way a
//! flat filename can (see `covers.rs`/`Cache::add_format`'s own
//! narrower sanitization) -- a real relative-path join is required,
//! and it must not be allowed to escape the book's own directory.
//! [`safe_join`] ports upstream's own containment check
//! (`normpath(dest).startswith(normpath(bookdir))` in `backend.py`)
//! via lexical (no filesystem access, so it works for paths that
//! don't exist yet) `.`/`..` resolution followed by a
//! component-wise (not string-prefix) `starts_with` check -- a plain
//! string-prefix check has a classic bypass (`/lib/A` is a string-
//! prefix of `/lib/AB/evil`), which `Path::starts_with`'s
//! component-aware comparison avoids.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::cache::Cache;
use crate::constants::{COVER_FILE_NAME, METADATA_FILE_NAME};

/// One extra file found by [`list_extra_files`] -- port of `ExtraFile`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraFile {
    /// Relative to the book's own directory, `/`-separated.
    pub relpath: String,
    pub file_path: PathBuf,
    pub size: u64,
    pub mtime_ns: i128,
}

/// Lexically resolves `.`/`..` components without touching the
/// filesystem (so it works for a path that doesn't exist yet, e.g. a
/// new file about to be created).
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Joins `relpath` onto `bookdir`, rejecting the result if it would
/// land outside `bookdir` once `.`/`..` are resolved. `None` means
/// "reject" (matching upstream's own `add_extra_file`/
/// `remove_extra_files`, which silently skip an escaping path rather
/// than erroring).
fn safe_join(bookdir: &Path, relpath: &str) -> Option<PathBuf> {
    let dest = lexically_normalize(&bookdir.join(relpath));
    let bookdir = lexically_normalize(bookdir);
    if dest.starts_with(&bookdir) {
        Some(dest)
    } else {
        None
    }
}

fn book_dir(cache: &Cache, book_id: i32) -> anyhow::Result<Option<PathBuf>> {
    let path_rel = cache.field_for(book_id, "path")?;
    Ok(path_rel.filter(|p| !p.is_empty()).map(|p| cache.backend.library_path.join(p)))
}

/// Port of `list_extra_files`. `pattern` matches
/// `calibre_db::constants::DATA_FILE_PATTERN` (`"data/**/*"`) to
/// restrict to the `data/` subdirectory, or `""` for everything in
/// the book's directory. Only a glob-style `pattern` is supported
/// (matching the one real caller under issue #418); an empty pattern
/// walks the whole tree, matching upstream's own `os.walk` fallback.
pub fn list_extra_files(cache: &Cache, book_id: i32, pattern: &str) -> anyhow::Result<Vec<ExtraFile>> {
    let Some(bookdir) = book_dir(cache, book_id)? else {
        return Ok(Vec::new());
    };
    if !bookdir.exists() {
        return Ok(Vec::new());
    }

    let known_files: std::collections::HashSet<&str> = [COVER_FILE_NAME, METADATA_FILE_NAME].into_iter().collect();
    let mut out = Vec::new();

    let candidates: Vec<PathBuf> = if pattern.is_empty() {
        walk_dir(&bookdir)
    } else {
        glob_match(&bookdir, pattern)
    };

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let Ok(relpath) = path.strip_prefix(&bookdir) else { continue };
        let relpath = relpath.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        if known_files.contains(relpath.as_str()) {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else { continue };
        out.push(ExtraFile {
            relpath,
            file_path: path,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
        });
    }
    Ok(out)
}

#[cfg(unix)]
fn mtime_ns(meta: &fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128
}

#[cfg(not(unix))]
fn mtime_ns(meta: &fs::Metadata) -> i128 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i128).unwrap_or(0)
}

fn walk_dir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// A narrow glob matcher covering exactly `DATA_FILE_PATTERN`'s shape
/// (`"data/**/*"` -- everything under one named subdirectory,
/// recursively) rather than a general glob engine, since that's the
/// only pattern any real caller under issue #418 uses.
fn glob_match(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let Some(subdir) = pattern.split('/').next() else { return Vec::new() };
    walk_dir(&root.join(subdir))
}

/// Port of `add_extra_files`. Returns, per relpath, whether it was
/// actually written (`false` when `replace` is `false` and a file
/// already exists there -- matching upstream's own
/// `added[relpath] = bool(...)`).
pub fn add_extra_files(cache: &Cache, book_id: i32, files: &HashMap<String, Vec<u8>>, replace: bool) -> anyhow::Result<HashMap<String, bool>> {
    let Some(bookdir) = book_dir(cache, book_id)? else {
        anyhow::bail!("Book {book_id} not found");
    };
    let mut added = HashMap::new();
    for (relpath, data) in files {
        let Some(dest) = safe_join(&bookdir, relpath) else {
            added.insert(relpath.clone(), false);
            continue;
        };
        if !replace && dest.exists() {
            added.insert(relpath.clone(), false);
            continue;
        }
        cache.backend.write_handle()?.write_atomic(&dest, data)?;
        added.insert(relpath.clone(), true);
    }
    Ok(added)
}

/// Port of `remove_extra_files`, always permanent (see module doc).
/// Returns, per relpath, `None` on success or `Some(error message)`
/// on failure -- matching upstream's `{relpath: Exception|None}`.
pub fn remove_extra_files(cache: &Cache, book_id: i32, relpaths: &[String], _permanent: bool) -> anyhow::Result<HashMap<String, Option<String>>> {
    let Some(bookdir) = book_dir(cache, book_id)? else {
        return Ok(relpaths.iter().map(|r| (r.clone(), None)).collect());
    };
    let mut errors = HashMap::new();
    for relpath in relpaths {
        let Some(path) = safe_join(&bookdir, relpath) else {
            continue; // matches upstream: an escaping path is silently skipped, not an error
        };
        match cache.backend.write_handle().and_then(|h| h.remove_atomic(&path).map_err(Into::into)) {
            Ok(()) => {}
            Err(e) if path.exists() => {
                errors.insert(relpath.clone(), Some(e.to_string()));
            }
            Err(_) => {} // already gone -- matches upstream's own `except FileNotFoundError: pass`
        }
    }
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;
    use std::fs;
    use tempfile::tempdir;

    fn open_test_cache_with_book() -> (tempfile::TempDir, Cache, i32) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("src.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["A".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        (dir, cache, book_id)
    }

    #[test]
    fn add_then_list_then_remove_round_trips_a_data_file() {
        let (dir, cache, book_id) = open_test_cache_with_book();
        let mut files = HashMap::new();
        files.insert("data/notes.pdf".to_string(), b"pdf bytes".to_vec());
        let added = add_extra_files(&cache, book_id, &files, true).unwrap();
        assert_eq!(added.get("data/notes.pdf"), Some(&true));

        let listed = list_extra_files(&cache, book_id, "data/**/*").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].relpath, "data/notes.pdf");
        assert_eq!(listed[0].size, 9);
        assert!(fs::read(&listed[0].file_path).unwrap() == b"pdf bytes");
        let _ = &dir;

        let errors = remove_extra_files(&cache, book_id, &["data/notes.pdf".to_string()], true).unwrap();
        assert!(errors.is_empty());
        assert!(list_extra_files(&cache, book_id, "data/**/*").unwrap().is_empty());
    }

    #[test]
    fn list_extra_files_excludes_the_cover_file() {
        // Uses the DATA_FILE_PATTERN shape (the only pattern any real
        // HTTP endpoint under issue #418 uses) -- cover.jpg lives
        // outside data/ anyway, so this also confirms the pattern
        // restriction itself works, not just the exclusion list.
        let (_dir, cache, book_id) = open_test_cache_with_book();
        crate::covers::set_cover(&cache, book_id, b"fake cover bytes").unwrap();
        let mut files = HashMap::new();
        files.insert("data/readme.txt".to_string(), b"hello".to_vec());
        add_extra_files(&cache, book_id, &files, true).unwrap();

        let listed = list_extra_files(&cache, book_id, "data/**/*").unwrap();
        let relpaths: Vec<&str> = listed.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(relpaths, vec!["data/readme.txt"]);
    }

    #[test]
    fn list_extra_files_with_an_empty_pattern_walks_the_whole_book_directory() {
        // Disclosed simplification (see module doc): upstream also
        // excludes the book's own format files from an unrestricted
        // walk (`if '/' not in pattern: known_files.add(...)`) -- not
        // ported here, since every real HTTP endpoint under issue
        // #418 always passes DATA_FILE_PATTERN (which contains a `/`,
        // so upstream's own format-exclusion branch never runs for
        // them either). This test documents the real, current
        // behavior rather than asserting the unported exclusion.
        let (_dir, cache, book_id) = open_test_cache_with_book();
        let mut files = HashMap::new();
        files.insert("readme.txt".to_string(), b"hello".to_vec());
        add_extra_files(&cache, book_id, &files, true).unwrap();

        let listed = list_extra_files(&cache, book_id, "").unwrap();
        let relpaths: Vec<&str> = listed.iter().map(|f| f.relpath.as_str()).collect();
        assert!(relpaths.contains(&"readme.txt"), "got: {relpaths:?}");
        assert!(relpaths.iter().any(|r| r.ends_with(".epub")), "expected the book's own format file to appear too (disclosed gap), got: {relpaths:?}");
    }

    #[test]
    fn add_extra_files_does_not_overwrite_when_replace_is_false() {
        let (_dir, cache, book_id) = open_test_cache_with_book();
        let mut first = HashMap::new();
        first.insert("data/x.txt".to_string(), b"first".to_vec());
        add_extra_files(&cache, book_id, &first, true).unwrap();

        let mut second = HashMap::new();
        second.insert("data/x.txt".to_string(), b"second".to_vec());
        let added = add_extra_files(&cache, book_id, &second, false).unwrap();
        assert_eq!(added.get("data/x.txt"), Some(&false));

        let listed = list_extra_files(&cache, book_id, "data/**/*").unwrap();
        assert_eq!(fs::read(&listed[0].file_path).unwrap(), b"first");
    }

    #[test]
    fn add_extra_files_rejects_a_path_traversal_relpath() {
        let (_dir, cache, book_id) = open_test_cache_with_book();
        let mut files = HashMap::new();
        files.insert("../../../../../../../tmp/extra-files-traversal-poc".to_string(), b"pwned".to_vec());
        let added = add_extra_files(&cache, book_id, &files, true).unwrap();
        assert_eq!(added.values().next(), Some(&false));
        assert!(!std::path::Path::new("/tmp/extra-files-traversal-poc").exists());
    }

    #[test]
    fn remove_extra_files_ignores_an_escaping_relpath_without_erroring() {
        let (_dir, cache, book_id) = open_test_cache_with_book();
        let errors = remove_extra_files(&cache, book_id, &["../../../../../../../etc/passwd".to_string()], true).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn safe_join_rejects_a_sibling_directory_with_a_shared_prefix() {
        // The classic string-prefix-check bypass: "/lib/A" is a
        // string-prefix of "/lib/AB/evil" but not a real path
        // ancestor of it -- Path::starts_with must not be fooled.
        let bookdir = Path::new("/lib/A");
        assert!(safe_join(bookdir, "../AB/evil").is_none());
    }

    #[test]
    fn list_extra_files_on_a_book_with_no_path_returns_empty() {
        let (_dir, cache) = {
            let dir = tempdir().unwrap();
            let cache = Cache::new(dir.path()).unwrap();
            (dir, cache)
        };
        assert!(list_extra_files(&cache, 999, "").unwrap().is_empty());
    }
}
