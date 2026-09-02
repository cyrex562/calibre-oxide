//! Port of `old_src/src/calibre/srv/library_broker.py`'s
//! `LibraryBroker` (issue #423, first slice) -- a pool of opened
//! libraries, addressed by a `library_id` key, with a "default
//! library" concept for requests that don't name one.
//!
//! # Scope: wired into `AppState`, one handler switches on it for real
//!
//! `AppState::cache: Arc<Cache>` is still read directly by every
//! handler in this crate except [`crate::content::get`] (dozens of
//! call sites across `ajax`/`opds`/`books`/`notes`/`cdb`/
//! `data_files`/`reader_profiles`/`fts`). `AppState::libraries:
//! Option<Arc<LibraryBroker>>` and `AppState::cache_for` are the real
//! multi-library entry points this first slice adds; `content::get`
//! (which already accepted a `library_id` URL segment, previously
//! ignored) is threaded through `cache_for` as this slice's one
//! real, tested demonstration that `library_id` actually switches
//! libraries end-to-end over HTTP (see `content::tests::
//! library_id_in_the_url_switches_between_libraries`). Migrating
//! every other handler to real `library_id` routing, and exposing
//! `library_map` for real (`ajax::library_info`'s hardcoded single
//! entry, OPDS per-library nav entries, `cdb`'s copy-to-library) is
//! substantial enough on its own to be its own follow-up issue,
//! matching #423's own text ("expect this to itself need
//! splitting... as a first slice before... land as separate
//! follow-ups").
//!
//! # Not ported: `GuiLibraryBroker`
//!
//! Upstream's `GuiLibraryBroker` subclass (lazy per-GUI-usage loading
//! with LRU eviction, `gui_library_changed`, event-listener wiring)
//! is exclusively for calibre's desktop GUI switching between
//! libraries interactively -- this crate has no GUI, so none of that
//! applies. Only the base `LibraryBroker` (a server-side pool with no
//! GUI concept) is in scope.
//!
//! # Narrowed: eager loading, no `samefile`-based OS-dedup subtlety
//!
//! Every library in the pool is opened immediately in
//! [`LibraryBroker::new`], rather than lazily on first
//! [`LibraryBroker::get`] the way upstream's own `loaded_dbs` cache
//! does. A handful of libraries opened once at server startup doesn't
//! need lazy loading to stay responsive, and eager loading surfaces a
//! bad library path as a startup error instead of a later per-request
//! one -- a reasonable, disclosed simplification for this first
//! slice. [`canonicalize_path`] also skips upstream's
//! `os.path.normcase` (a no-op on POSIX, Windows-only lowercasing)
//! since [`calibre_utils::filenames::samefile`] already needs no help
//! distinguishing two Linux paths.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use calibre_db::cache::Cache;
use indexmap::IndexMap;

/// Port of `canonicalize_path`: an absolute, `/`-separated,
/// trailing-slash-stripped form of `path`, for comparing two paths
/// that might be spelled differently but name the same location.
pub fn canonicalize_path(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Port of `basename`: `path`'s final path component, falling back
/// to `"Library"` for a path with none (upstream's own fallback,
/// reachable on Windows for a bare drive path -- kept for parity even
/// though this port's own [`canonicalize_path`] wouldn't produce one).
pub fn basename(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("Library").to_string()
}

/// Port of `make_library_id_unique` + `library_id_from_path`:
/// `path`'s basename, spaces replaced with underscores, suffixed with
/// an incrementing counter if it collides with a key already in `existing`.
pub fn library_id_from_path<V>(path: &Path, existing: &IndexMap<String, V>) -> String {
    let base = basename(path).replace(' ', "_");
    if !existing.contains_key(&base) {
        return base;
    }
    let mut n = 1u32;
    loop {
        let candidate = format!("{base}{n}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Whether `path` looks like a real calibre library -- matching
/// upstream's own `LibraryDatabase.exists_at` check (a real
/// `metadata.db` file), so a bad path is skipped rather than causing
/// [`LibraryBroker::new`] to fail entirely for every other library.
fn library_exists_at(path: &Path) -> bool {
    path.join("metadata.db").is_file()
}

/// Port of `LibraryBroker` (the base, non-GUI class -- see the module
/// doc for what's out of scope).
pub struct LibraryBroker {
    /// library_id -> opened library, in the order libraries were
    /// given to [`LibraryBroker::new`] -- the *first* entry is the
    /// default library, matching upstream's own
    /// `next(iter(self.lmap))` over an `OrderedDict`.
    libraries: IndexMap<String, Arc<Cache>>,
    /// library_id -> display name (`basename` of the library's path).
    names: IndexMap<String, String>,
}

use std::sync::Arc;

impl LibraryBroker {
    /// Opens every library under `paths`, skipping any that don't
    /// exist or duplicate an already-added path (by canonical path or
    /// [`calibre_utils::filenames::samefile`], matching upstream's own
    /// dedup). Errors only if opening a real, existing, non-duplicate
    /// library fails, or if `paths` is empty (there is no library to
    /// use as the default).
    pub fn new(paths: &[PathBuf]) -> Result<Self> {
        let mut libraries = IndexMap::new();
        let mut names = IndexMap::new();
        let mut seen: Vec<PathBuf> = Vec::new();

        for original_path in paths {
            let canonical = canonicalize_path(original_path);
            let is_duplicate = seen.iter().any(|s| s == &canonical || calibre_utils::filenames::samefile(s, &canonical));
            if is_duplicate {
                continue;
            }
            seen.push(canonical.clone());
            if !library_exists_at(&canonical) {
                continue;
            }

            let library_id = library_id_from_path(&canonical, &libraries);
            let cache = Cache::new(&canonical)
                .with_context(|| format!("failed to open library at {}", canonical.display()))?;
            names.insert(library_id.clone(), basename(&canonical));
            libraries.insert(library_id, Arc::new(cache));
        }

        if libraries.is_empty() {
            anyhow::bail!("no valid library found among the given paths");
        }

        Ok(Self { libraries, names })
    }

    /// Port of `default_library` (a property upstream): the first
    /// library's id.
    pub fn default_library_id(&self) -> &str {
        // Safe: `new` never returns an empty broker.
        self.libraries.keys().next().expect("LibraryBroker is never empty")
    }

    /// Port of `get`: the library for `library_id`, or the default
    /// library if `library_id` is `None` or empty. `None` if
    /// `library_id` names a library that isn't in this broker.
    pub fn get(&self, library_id: Option<&str>) -> Option<Arc<Cache>> {
        let id = match library_id {
            Some(id) if !id.is_empty() => id,
            _ => self.default_library_id(),
        };
        self.libraries.get(id).cloned()
    }

    /// Port of the `library_map` property: library_id -> display name,
    /// in the same order as [`LibraryBroker::new`]'s `paths`.
    pub fn library_map(&self) -> IndexMap<String, String> {
        self.names.clone()
    }

    /// Every known library id, in order.
    pub fn library_ids(&self) -> impl Iterator<Item = &str> {
        self.libraries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_library(dir: &Path, name: &str) -> PathBuf {
        let lib_dir = dir.join(name);
        fs::create_dir_all(&lib_dir).unwrap();
        Cache::new(&lib_dir).unwrap(); // creates metadata.db
        lib_dir
    }

    #[test]
    fn the_first_library_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "Library A");
        let b = make_library(dir.path(), "Library B");
        let broker = LibraryBroker::new(&[a, b]).unwrap();
        assert_eq!(broker.default_library_id(), "Library_A");
    }

    #[test]
    fn get_with_no_id_returns_the_default_library() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "Only");
        let broker = LibraryBroker::new(&[a]).unwrap();
        assert!(broker.get(None).is_some());
        assert!(broker.get(Some("")).is_some());
    }

    #[test]
    fn get_with_an_unknown_id_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "Only");
        let broker = LibraryBroker::new(&[a]).unwrap();
        assert!(broker.get(Some("nonexistent")).is_none());
    }

    #[test]
    fn get_switches_between_libraries_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "A");
        let b = make_library(dir.path(), "B");
        let broker = LibraryBroker::new(&[a, b]).unwrap();

        let default_cache = broker.get(None).unwrap();
        let b_cache = broker.get(Some("B")).unwrap();
        assert!(!Arc::ptr_eq(&default_cache, &b_cache), "A and B should be genuinely different opened libraries");
    }

    #[test]
    fn nonexistent_paths_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "Real");
        let fake = dir.path().join("does-not-exist");
        let broker = LibraryBroker::new(&[fake, a]).unwrap();
        assert_eq!(broker.library_ids().count(), 1);
    }

    #[test]
    fn an_all_nonexistent_path_list_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("does-not-exist");
        assert!(LibraryBroker::new(&[fake]).is_err());
    }

    #[test]
    fn duplicate_paths_are_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "A");
        let broker = LibraryBroker::new(&[a.clone(), a]).unwrap();
        assert_eq!(broker.library_ids().count(), 1);
    }

    #[test]
    fn colliding_display_names_get_a_disambiguating_suffix() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        // Two different directories that happen to share a basename.
        let a = make_library(dir1.path(), "Books");
        let b = make_library(dir2.path(), "Books");
        let broker = LibraryBroker::new(&[a, b]).unwrap();
        let ids: Vec<&str> = broker.library_ids().collect();
        assert_eq!(ids, vec!["Books", "Books1"]);
    }

    #[test]
    fn library_map_reports_display_names_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_library(dir.path(), "My Library");
        let broker = LibraryBroker::new(&[a]).unwrap();
        let map = broker.library_map();
        assert_eq!(map.get("My_Library").map(String::as_str), Some("My Library"));
    }

    #[test]
    fn empty_paths_list_is_an_error() {
        assert!(LibraryBroker::new(&[]).is_err());
    }
}
