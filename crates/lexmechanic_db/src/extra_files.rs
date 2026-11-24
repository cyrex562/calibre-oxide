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
//! `lexmechanic_db::constants::DATA_FILE_PATTERN` (`"data/**/*"`, which
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

use anyhow::Context;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::cache::Cache;
use crate::constants::DATA_DIR_NAME;

/// The `data/` prefix every relpath in this module's API carries.
const DATA_DIR_PREFIX: &str = "data/";

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

/// The directory holding a book's data files.
///
/// Every relpath in this module's API is relative to it, with the
/// leading `data/` that callers use (`DATA_FILE_PATTERN` is
/// `"data/**/*"`) naming this directory rather than a component inside
/// it -- [`on_disk`] strips it and [`api_relpath`] puts it back.
///
/// Two layouts:
///
/// - A book with a folder of its own keeps its data files in that
///   folder's `data/` subdirectory, where they have always been, so a
///   library in the old layout reads exactly as before.
/// - A book with an empty `path` lives in the library root (#893) and
///   gets `<library>/data/<uuid>/`. It cannot keep its data files in the
///   root itself: every root-level book has the same empty path, so they
///   all resolved to one shared `<library>/data/` directory. One book's
///   attached file was listed for every book, `add_extra_files`'s
///   "already exists" check fired between different books rather than
///   within one, and removing the second book's data file silently
///   deleted the first book's while reporting success (#954).
///
/// Keyed by **uuid** for the reason `covers::sidecar_cover_path` and
/// `backup::sidecar_opf_path` are: a local autoincrement id would
/// collide across synced machines. Deliberately under `<library>/data/`
/// rather than inside `.lexmechanic/` -- these are files the user
/// attached themselves, not derived state, so they stay visible and
/// manageable in the library folder.
///
/// `None` means the book does not exist at all, which is the distinction
/// callers actually branch on.
fn data_dir(cache: &Cache, book_id: i32) -> anyhow::Result<Option<PathBuf>> {
    let Some(path) = cache.field_for(book_id, "path")? else {
        return Ok(None);
    };
    if !path.is_empty() {
        return Ok(Some(cache.backend.library_path.join(path).join(DATA_DIR_NAME)));
    }
    let uuid = cache
        .book_uuid(book_id)?
        .filter(|u| !u.is_empty())
        .with_context(|| format!("book {book_id} has no uuid to key its data files by"))?;
    Ok(Some(cache.backend.library_path.join(DATA_DIR_NAME).join(uuid)))
}

/// Which paths under a book's data directory belong to some other book.
///
/// A path does when it is a tracked format file, or when it sits anywhere below another book's
/// folder: a source tree with `Reports/report.pdf` and `Reports/data/dataset.pdf` gives two books
/// the paths `Reports` and `Reports/data`, and the first book's `data/` directory is then the
/// second book's folder -- its files, and those of its own `data/` below, are not attached data
/// files, however deep the second book is nested.
///
/// Loaded once per call (two queries over the whole library), not asked per file: a book with
/// hundreds of attachments in a library of 100k books would otherwise scan the table twice each.
struct Claims {
    library: PathBuf,
    /// The data directory relative to the library, lower-cased: only folders at or below it count.
    scope_len: usize,
    formats: std::collections::HashSet<String>,
    folders: std::collections::HashSet<String>,
}

impl Claims {
    fn load(cache: &Cache, book_id: i32, dir: &Path) -> Self {
        let library = cache.backend.library_path.clone();
        let scope = dir.strip_prefix(&library).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let mut formats = std::collections::HashSet::new();
        let mut folders = std::collections::HashSet::new();
        let conn = cache.backend.conn.lock().unwrap();
        if let Ok(mut stmt) = conn.prepare("SELECT lower(CASE WHEN b.path = '' THEN '' ELSE b.path || '/' END || d.name || '.' || d.format) FROM data d JOIN books b ON b.id = d.book") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                formats.extend(rows.flatten());
            }
        }
        if let Ok(mut stmt) = conn.prepare("SELECT lower(path) FROM books WHERE id != ?1 AND path != ''") {
            if let Ok(rows) = stmt.query_map([book_id], |row| row.get::<_, String>(0)) {
                folders.extend(rows.flatten());
            }
        }
        Claims { library, scope_len: scope.to_lowercase().len(), formats, folders }
    }

    fn claims(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.library) else { return false };
        let rel = rel.to_string_lossy().replace('\\', "/").to_lowercase();
        if self.formats.contains(&rel) {
            return true;
        }
        let mut parent = rel.as_str();
        while let Some((up, _)) = parent.rsplit_once('/') {
            if up.len() < self.scope_len {
                break;
            }
            if self.folders.contains(up) {
                return true;
            }
            parent = up;
        }
        false
    }
}

/// Resolves one API relpath to a real path inside `dir`, rejecting
/// anything that would escape it.
fn on_disk(dir: &Path, relpath: &str) -> Option<PathBuf> {
    let inside = relpath.strip_prefix(DATA_DIR_PREFIX).unwrap_or(relpath);
    safe_join(dir, inside)
}

/// The inverse: the relpath callers see for a file found under `dir`.
fn api_relpath(dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    Some(format!("{DATA_DIR_PREFIX}{}", rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")))
}

/// Port of `list_extra_files`. `pattern` matches
/// `lexmechanic_db::constants::DATA_FILE_PATTERN` (`"data/**/*"`) to
/// restrict to the `data/` subdirectory, or `""` for everything in
/// the book's directory. Only a glob-style `pattern` is supported
/// (matching the one real caller under issue #418); an empty pattern
/// walks the whole tree, matching upstream's own `os.walk` fallback.
pub fn list_extra_files(cache: &Cache, book_id: i32, pattern: &str) -> anyhow::Result<Vec<ExtraFile>> {
    let Some(dir) = data_dir(cache, book_id)? else {
        return Ok(Vec::new());
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }

    // `pattern` selects nothing finer than "everything under the data
    // directory" for either of its two real values, now that the
    // directory *is* the `data/` scope: `DATA_FILE_PATTERN`'s leading
    // component names it, and an empty pattern is upstream's
    // unrestricted `os.walk`. The narrow glob matcher this used to need
    // went with it.
    let _ = pattern;

    let claims = Claims::load(cache, book_id, &dir);
    let mut out = Vec::new();
    for path in walk_dir(&dir) {
        if !path.is_file() || claims.claims(&path) {
            continue;
        }
        let Some(relpath) = api_relpath(&dir, &path) else { continue };
        let Ok(meta) = fs::metadata(&path) else { continue };
        out.push(ExtraFile { relpath, file_path: path, size: meta.len(), mtime_ns: mtime_ns(&meta) });
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


/// Port of `add_extra_files`. Returns, per relpath, whether it was
/// actually written (`false` when `replace` is `false` and a file
/// already exists there -- matching upstream's own
/// `added[relpath] = bool(...)`).
pub fn add_extra_files(cache: &Cache, book_id: i32, files: &HashMap<String, Vec<u8>>, replace: bool) -> anyhow::Result<HashMap<String, bool>> {
    let Some(dir) = data_dir(cache, book_id)? else {
        anyhow::bail!("Book {book_id} not found");
    };
    let mut added = HashMap::new();
    for (relpath, data) in files {
        let Some(dest) = on_disk(&dir, relpath) else {
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

/// Copies the file at `source` into the book's data directory under
/// `relpath` (the `data/` prefix optional), streaming it through the
/// journaled handle rather than buffering it. A name already taken is
/// auto-renamed `stem (1).ext`, `stem (2).ext`... -- calibre's
/// `add_extra_file(..., auto_rename=True)`, which `merge_extra_files`
/// uses. Returns the relpath actually written and the content's BLAKE3.
pub fn copy_extra_file_in(cache: &Cache, book_id: i32, source: &Path, relpath: &str) -> anyhow::Result<(String, String)> {
    let Some(dir) = data_dir(cache, book_id)? else {
        anyhow::bail!("Book {book_id} not found");
    };
    let inside = relpath.strip_prefix(DATA_DIR_PREFIX).unwrap_or(relpath);
    let wanted = Path::new(inside);
    let stem = wanted.file_stem().and_then(|s| s.to_str()).unwrap_or("file").to_string();
    let ext = wanted.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    let parent = wanted.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    for n in 0..1000 {
        let name = if n == 0 { format!("{stem}{ext}") } else { format!("{stem} ({n}){ext}") };
        let candidate = if parent.is_empty() { name } else { format!("{parent}/{name}") };
        let Some(dest) = safe_join(&dir, &candidate) else {
            anyhow::bail!("{relpath:?} does not name a place inside the book's data directory");
        };
        if dest.exists() {
            continue;
        }
        let hash = cache.backend.write_handle()?.copy_atomic(source, &dest)?;
        return Ok((format!("{DATA_DIR_PREFIX}{candidate}"), hash));
    }
    anyhow::bail!("could not find an unused name for {relpath:?} in book {book_id}'s data directory")
}

/// Port of `remove_extra_files`, always permanent (see module doc).
/// Returns, per relpath, `None` on success or `Some(error message)`
/// on failure -- matching upstream's `{relpath: Exception|None}`.
pub fn remove_extra_files(cache: &Cache, book_id: i32, relpaths: &[String], _permanent: bool) -> anyhow::Result<HashMap<String, Option<String>>> {
    let Some(dir) = data_dir(cache, book_id)? else {
        return Ok(relpaths.iter().map(|r| (r.clone(), None)).collect());
    };
    let mut errors = HashMap::new();
    let claims = Claims::load(cache, book_id, &dir);
    for relpath in relpaths {
        let Some(path) = on_disk(&dir, relpath) else {
            continue; // matches upstream: an escaping path is silently skipped, not an error
        };
        if claims.claims(&path) {
            errors.insert(relpath.clone(), Some("that file belongs to another book".to_string()));
            continue;
        }
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
    use crate::constants::DATA_FILE_PATTERN;
    use lexmechanic_ebooks::metadata::MetaInformation;
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
        // The cover cannot appear structurally rather than by being
        // filtered out: it lives at `.lexmechanic/covers/<uuid>.jpg`,
        // and a legacy book's `cover.jpg`/`metadata.opf` sit in the book
        // folder rather than inside its `data/` subdirectory. Neither is
        // under the directory this walks, so the `known_files` exclusion
        // list this used to need is gone.
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
    fn a_relpath_is_always_reported_under_data_whichever_pattern_is_passed() {
        // Both real values of `pattern` mean "everything the book has",
        // now that the data directory *is* the `data/` scope:
        // `DATA_FILE_PATTERN`'s leading component names that directory,
        // and an empty pattern is upstream's unrestricted `os.walk`.
        //
        // This also closes a gap that was previously disclosed rather
        // than fixed: upstream excludes a book's own format files from an
        // unrestricted walk (`if '/' not in pattern:
        // known_files.add(...)`), a branch never ported here, and this
        // test used to assert the resulting wart -- the book's own
        // `.epub` listed as one of its extra files. With data files in
        // their own directory there is nothing to exclude.
        let (_dir, cache, book_id) = open_test_cache_with_book();
        let mut files = HashMap::new();
        files.insert("data/readme.txt".to_string(), b"hello".to_vec());
        add_extra_files(&cache, book_id, &files, true).unwrap();

        for pattern in ["", DATA_FILE_PATTERN] {
            let listed = list_extra_files(&cache, book_id, pattern).unwrap();
            let relpaths: Vec<&str> = listed.iter().map(|f| f.relpath.as_str()).collect();
            assert_eq!(relpaths, vec!["data/readme.txt"], "pattern {pattern:?}");
        }
    }

    /// The directory is visible in the library folder, not hidden in
    /// `.lexmechanic/`: these are files the user attached themselves,
    /// not derived state.
    #[test]
    fn a_root_level_books_data_files_are_visible_in_the_library_folder() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "Only".to_string();
        let book_id = cache.add_book_db_entry(&meta, "").unwrap();
        let uuid = cache.book_uuid(book_id).unwrap().unwrap();

        let mut files = HashMap::new();
        files.insert("data/notes.txt".to_string(), b"hello".to_vec());
        add_extra_files(&cache, book_id, &files, false).unwrap();

        let expected = dir.path().join("data").join(&uuid).join("notes.txt");
        assert!(expected.is_file(), "expected the file at {}", expected.display());
        assert!(!dir.path().join(".lexmechanic").join("books").exists(), "data files must not be hidden in the state directory");
    }

    /// #954: every root-level book had the same empty `path`, so they all
    /// shared one `<library>/data/` directory.
    #[test]
    fn data_files_are_not_shared_between_books_in_the_library_root() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let mut first_meta = MetaInformation::default();
        first_meta.title = "First".to_string();
        let first = cache.add_book_db_entry(&first_meta, "").unwrap();
        let mut second_meta = MetaInformation::default();
        second_meta.title = "Second".to_string();
        let second = cache.add_book_db_entry(&second_meta, "").unwrap();

        let mut files = HashMap::new();
        files.insert("data/notes.txt".to_string(), b"the first book's notes".to_vec());
        add_extra_files(&cache, first, &files, false).unwrap();

        assert_eq!(list_extra_files(&cache, first, DATA_FILE_PATTERN).unwrap().len(), 1);
        assert!(
            list_extra_files(&cache, second, DATA_FILE_PATTERN).unwrap().is_empty(),
            "the second book sees the first book's data file"
        );
    }

    /// The destructive half: a user tidying up one book's data files must
    /// not delete another book's. This reported success while doing it.
    #[test]
    fn removing_one_root_level_books_data_file_leaves_anothers_alone() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let mut first_meta = MetaInformation::default();
        first_meta.title = "First".to_string();
        let first = cache.add_book_db_entry(&first_meta, "").unwrap();
        let mut second_meta = MetaInformation::default();
        second_meta.title = "Second".to_string();
        let second = cache.add_book_db_entry(&second_meta, "").unwrap();

        let mut files = HashMap::new();
        files.insert("data/notes.txt".to_string(), b"the first book's notes".to_vec());
        add_extra_files(&cache, first, &files, false).unwrap();

        remove_extra_files(&cache, second, &["data/notes.txt".to_string()], true).unwrap();

        assert_eq!(
            list_extra_files(&cache, first, DATA_FILE_PATTERN).unwrap().len(),
            1,
            "removing the second book's data file destroyed the first book's"
        );
    }

    /// Two books can hold a data file of the same name without one
    /// overwriting the other -- the "already exists" check used to fire
    /// between different books rather than within one.
    #[test]
    fn two_books_can_each_hold_a_data_file_of_the_same_name() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let mut first_meta = MetaInformation::default();
        first_meta.title = "First".to_string();
        let first = cache.add_book_db_entry(&first_meta, "").unwrap();
        let mut second_meta = MetaInformation::default();
        second_meta.title = "Second".to_string();
        let second = cache.add_book_db_entry(&second_meta, "").unwrap();

        let mut a = HashMap::new();
        a.insert("data/notes.txt".to_string(), b"first".to_vec());
        let added_first = add_extra_files(&cache, first, &a, false).unwrap();
        let mut b = HashMap::new();
        b.insert("data/notes.txt".to_string(), b"second".to_vec());
        let added_second = add_extra_files(&cache, second, &b, false).unwrap();

        assert_eq!(added_first.get("data/notes.txt"), Some(&true));
        assert_eq!(
            added_second.get("data/notes.txt"),
            Some(&true),
            "the second book's file was refused as an existing one"
        );

        let read_back = |id| {
            let listed = list_extra_files(&cache, id, DATA_FILE_PATTERN).unwrap();
            std::fs::read(&listed[0].file_path).unwrap()
        };
        assert_eq!(read_back(first), b"first");
        assert_eq!(read_back(second), b"second");
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
