//! Port of `old_src/src/calibre/ebooks/rtf2xml/copy.py` (`Copy`).
//!
//! A small debug-snapshot helper used throughout the rtf2xml pipeline:
//! every pass writes its output to a fresh temp file, and if the
//! pipeline is running in "copy"/debug mode, that temp file is first
//! copied into a debug directory under a fixed name (e.g.
//! `tokenize.data`, `processed_tokens.data`) before being moved over
//! the pass's real input file. `copy.py` is *not* Python's standard
//! library `copy` module -- the name collides only in the Python
//! package namespace (`from calibre.ebooks.rtf2xml import copy`), and
//! this Rust module keeps that same import-time distinction from
//! `std`'s `Copy` trait by living under `rtf2xml::copy` rather than
//! being re-exported bare.
//!
//! # Preserved upstream quirk: `rename` doesn't rename
//!
//! `Copy.rename(source, dest)` is `shutil.copyfile(source, dest)` --
//! it *copies* `source`'s bytes onto `dest`, leaving `source` in place.
//! Every caller in the pipeline (see `tokenize.py`, `process_tokens.py`,
//! `replace_illegals.py`, `line_endings.py`) immediately follows it
//! with an explicit `os.remove(self.__write_to)` to clean up the
//! now-redundant source, i.e. the two calls together implement a move,
//! but `rename` alone does not. Ported as-is: [`Copy::rename`] here
//! copies too, and does not remove `source`.
//!
//! # Preserved upstream quirk: shared debug directory
//!
//! `Copy.__dir` is a Python *class* attribute, not an instance
//! attribute: `set_dir` mutates it on the class itself, so every
//! `Copy` instance across the whole pipeline run shares one debug
//! directory once any instance sets it. Ported with a
//! process-wide `OnceLock<Mutex<Option<PathBuf>>>` for the same
//! sharing behavior.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use thiserror::Error;

/// Port of the `raise self.__bug_handler(message)` calls in
/// `Copy.set_dir`.
#[derive(Debug, Error)]
pub enum CopyError {
    /// Port of `'No directory has been provided to write to in the
    /// copy.py'` (raised when `deb_dir is None`).
    #[error("No directory has been provided to write to in the copy.py")]
    NoDirectoryProvided,
    /// Port of `f'{deb_dir} is not a directory'`.
    #[error("{0} is not a directory")]
    NotADirectory(String),
    /// Any underlying filesystem failure from `copy_file`/`rename`/
    /// `remove_files`.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// `copy_file`/`remove_files` called before any `Copy` in the
    /// process has called `set_dir`. Not raised by the Python (which
    /// would instead build an empty `''` path via the `Copy.__dir = ''`
    /// class default and hit a filesystem error), but surfaced
    /// explicitly here rather than silently operating on `''`.
    #[error("no debug directory has been set; call Copy::set_dir first")]
    DirNotSet,
}

fn debug_dir_cell() -> &'static Mutex<Option<PathBuf>> {
    static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(|| Mutex::new(None))
}

/// Port of `Copy`. Holds no per-instance state (the Python's own
/// `self.__file`/`self.__bug_handler` fields are unused by every method
/// below other than `__init__` itself), so this is a zero-sized marker
/// type -- construct with [`Copy::new`] and call its methods.
#[derive(Debug, Default, Clone, Copy)]
pub struct Copy;

impl Copy {
    /// Port of `Copy.__init__`.
    pub fn new() -> Self {
        Copy
    }

    /// Port of `Copy.set_dir`. `deb_dir: None` matches the Python
    /// call site passing `None` (or omitting the argument); `Some(p)`
    /// where `p` is not a directory matches the `os.path.isdir` check.
    pub fn set_dir(&self, deb_dir: Option<&Path>) -> Result<(), CopyError> {
        let deb_dir = deb_dir.ok_or(CopyError::NoDirectoryProvided)?;
        if !deb_dir.is_dir() {
            return Err(CopyError::NotADirectory(deb_dir.display().to_string()));
        }
        *debug_dir_cell()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(deb_dir.to_path_buf());
        Ok(())
    }

    /// Port of `Copy.remove_files` (+ `__remove_the_files`): recursively
    /// removes every file (not directory) under the shared debug
    /// directory. Matches the Python's `except OSError: pass` -- a
    /// file that fails to remove is silently skipped, not an error.
    pub fn remove_files(&self) -> Result<(), CopyError> {
        let dir = current_dir()?;
        remove_files_recursive(&dir);
        Ok(())
    }

    /// Port of `Copy.copy_file`: copies `file` to
    /// `<debug_dir>/<new_file>`.
    pub fn copy_file(&self, file: impl AsRef<Path>, new_file: &str) -> Result<(), CopyError> {
        let dir = current_dir()?;
        let write_file = dir.join(new_file);
        fs::copy(file, write_file)?;
        Ok(())
    }

    /// Port of `Copy.rename`. See the module docs' "`rename` doesn't
    /// rename" quirk: this copies `source`'s bytes onto `dest` and
    /// leaves `source` in place, exactly like the Python.
    pub fn rename(
        &self,
        source: impl AsRef<Path>,
        dest: impl AsRef<Path>,
    ) -> Result<(), CopyError> {
        fs::copy(source, dest)?;
        Ok(())
    }
}

fn current_dir() -> Result<PathBuf, CopyError> {
    debug_dir_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or(CopyError::DirNotSet)
}

fn remove_files_recursive(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_files_recursive(&path);
        } else {
            // Port of `except OSError: pass`.
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // `Copy`'s debug directory is process-wide shared state (see the
    // module docs), so tests that touch it must not run concurrently
    // with each other.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn set_dir_rejects_none() {
        let _guard = TEST_LOCK.lock().unwrap();
        let err = Copy::new().set_dir(None).unwrap_err();
        assert!(matches!(err, CopyError::NoDirectoryProvided));
    }

    #[test]
    fn set_dir_rejects_a_non_directory_path() {
        let _guard = TEST_LOCK.lock().unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let err = Copy::new().set_dir(Some(tmp.path())).unwrap_err();
        assert!(matches!(err, CopyError::NotADirectory(_)));
    }

    #[test]
    fn copy_file_and_rename_and_remove_files_round_trip() {
        let _guard = TEST_LOCK.lock().unwrap();
        let debug_dir = tempfile::tempdir().unwrap();
        let copy = Copy::new();
        copy.set_dir(Some(debug_dir.path())).unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("source.data");
        fs::write(&src, b"hello world").unwrap();

        copy.copy_file(&src, "snapshot.data").unwrap();
        let snapshot = debug_dir.path().join("snapshot.data");
        assert_eq!(fs::read(&snapshot).unwrap(), b"hello world");

        let dest = src_dir.path().join("dest.data");
        copy.rename(&src, &dest).unwrap();
        // Port of the "rename doesn't rename" quirk: source still
        // exists after `rename`.
        assert!(src.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"hello world");

        copy.remove_files().unwrap();
        assert!(!snapshot.exists());
        assert!(debug_dir.path().is_dir());
    }

    #[test]
    fn copy_file_before_set_dir_errors_instead_of_using_an_empty_path() {
        let _guard = TEST_LOCK.lock().unwrap();
        *debug_dir_cell()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("source.data");
        fs::write(&src, b"x").unwrap();
        let err = Copy::new().copy_file(&src, "out.data").unwrap_err();
        assert!(matches!(err, CopyError::DirNotSet));
    }
}
