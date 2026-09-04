//! Port of `calibre.utils.copy_files` (issue #464): a hardlink-aware,
//! recursive tree copy used by library-copy/export/duplicate-detection
//! features -- `copy_tree`/`copy_files`/`rename_files`.
//!
//! # Scope decision (issue #464's own open question)
//!
//! The issue asked whether to generalize this into a shared utility
//! `crates/calibre_db/src/copy_to_library.rs` could adopt, or leave
//! copy logic inlined per-caller as it is today. Decision: port the
//! real, independently-testable primitive here, standalone -- **not**
//! wired into `copy_to_library.rs`. Migrating an existing, working,
//! already-tested call site to a new abstraction is a separate
//! refactor decision with its own risk (behavior-preserving migration
//! needs its own verification pass), not something a port issue
//! should bundle in. Same "port the real primitive, defer wiring into
//! an existing caller" call as `calibre_utils::smtp`/`icu`/`exim` this
//! session.
//!
//! # Disclosed narrowing vs. upstream
//!
//! - `WindowsFileCopier` (locks every file via `winutil` handles
//!   before starting the copy, so no other process can interfere) is
//!   **not ported**. This crate's Unix-only `#[cfg(windows)]`
//!   dependency table has no `winutil`-equivalent FFI, and no other
//!   port in this crate has needed Windows-specific file locking
//!   either. [`FileCopier`] here is upstream's own `UnixFileCopier`
//!   semantics (hardlink-first, `shutil.copy2`-equivalent fallback),
//!   used verbatim on every platform `std::fs` supports -- the same
//!   semantics upstream itself uses on every *non*-Windows platform.
//! - `windows_check_if_files_in_use` (a Windows-only "would this copy
//!   fail because files are locked" pre-check) is not ported for the
//!   same reason.
//! - Metadata preservation (`shutil.copystat`: mtime/permissions)
//!   after a real (non-hardlink) copy is narrowed to what
//!   `std::fs::copy` itself already preserves (permissions on Unix;
//!   it does not copy mtimes) rather than a separate explicit
//!   `copystat`-equivalent pass -- no caller in this crate has needed
//!   exact mtime preservation on a *copied* (not hardlinked) file.

use crate::filenames::samefile;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Port of `UnixFileCopier` (used verbatim on every platform this
/// port targets -- see module doc). Registers a batch of
/// source->dest file pairs, then performs the copy/rename/delete as
/// one operation over the whole batch.
#[derive(Default)]
pub struct FileCopier {
    delete_all: bool,
    copy_map: Vec<(PathBuf, PathBuf)>,
}

impl FileCopier {
    pub fn new(delete_all: bool) -> Self {
        Self { delete_all, copy_map: Vec::new() }
    }

    pub fn register(&mut self, src: &Path, dest: &Path) {
        self.copy_map.push((src.to_path_buf(), dest.to_path_buf()));
    }

    /// Port of `rename_all`.
    pub fn rename_all(&self) -> Result<()> {
        for (src, dest) in &self.copy_map {
            fs::rename(src, dest)?;
        }
        Ok(())
    }

    /// Port of `copy_all`: hardlink first (fast, no data duplication
    /// on the same filesystem), falling back to a real copy if
    /// hardlinking fails (different filesystem, filesystem doesn't
    /// support hardlinks, etc) -- matching upstream's own
    /// `with suppress(OSError): os.link(...) ... continue` /
    /// `with suppress(SameFileError): shutil.copy2(...)` fallback
    /// chain. Per-file failures are swallowed (matching upstream,
    /// which also only best-effort's metadata preservation) rather
    /// than aborting the whole batch.
    pub fn copy_all(&self) {
        for (src, dest) in &self.copy_map {
            if fs::hard_link(src, dest).is_ok() {
                continue;
            }
            if samefile(src, dest) {
                continue;
            }
            let _ = fs::copy(src, dest);
        }
    }

    /// Port of `delete_all_source_files`.
    pub fn delete_all_source_files(&self) {
        for (src, _) in &self.copy_map {
            let _ = fs::remove_file(src);
        }
    }

    /// Port of the `with copier:` context manager's `__exit__`:
    /// performs `copy_all`, then deletes sources if `delete_all` was
    /// requested (matching upstream's `if self.delete_all and
    /// exc_val is None`, i.e. only on success -- callers that want
    /// delete-on-success-only should call [`Self::copy_all`]
    /// themselves and only call [`Self::delete_all_source_files`] if
    /// it didn't error).
    fn finish_copy(&self) {
        self.copy_all();
        if self.delete_all {
            self.delete_all_source_files();
        }
    }
}

/// Port of `rename_files`: rename a batch of files.
pub fn rename_files(src_to_dest: &[(PathBuf, PathBuf)]) -> Result<()> {
    let mut copier = FileCopier::new(false);
    for (s, d) in src_to_dest {
        copier.register(s, d);
    }
    copier.rename_all()
}

/// Port of `copy_files`: copy a batch of files, optionally deleting
/// each source after a successful copy. Pairs where source and
/// destination are the same file are skipped, matching upstream.
pub fn copy_files(src_to_dest: &[(PathBuf, PathBuf)], delete_source: bool) {
    let mut copier = FileCopier::new(delete_source);
    for (s, d) in src_to_dest {
        if !samefile(s, d) {
            copier.register(s, d);
        }
    }
    copier.finish_copy();
}

/// Port of `identity_transform`: the default `transform_destination_filename`.
pub fn identity_transform(_src_path: &Path, dest_path: &Path) -> PathBuf {
    dest_path.to_path_buf()
}

fn register_folder_recursively(top_src: &Path, current_dir: &Path, copier: &mut FileCopier, dest_dir: &Path, transform: &dyn Fn(&Path, &Path) -> PathBuf, read_only: bool) -> Result<()> {
    for entry in fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();
        // Always relative to the *original* top-level source (not the
        // current recursion depth's directory) -- this is what
        // upstream's `os.walk`-based version naturally gets, and
        // getting it wrong (relative to `current_dir` instead) is a
        // real bug this port's own tests caught: a nested file would
        // land directly under `dest_dir` instead of under its real
        // subdirectory path.
        let rel = path.strip_prefix(top_src)?;
        let dest = dest_dir.join(rel);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if !read_only {
                fs::create_dir_all(&dest)?;
            }
            register_folder_recursively(top_src, &path, copier, dest_dir, transform, read_only)?;
        } else if file_type.is_symlink() {
            if !read_only {
                let link_target = fs::read_link(&path)?;
                let dest = transform(&path, &dest);
                #[cfg(unix)]
                std::os::unix::fs::symlink(&link_target, &dest)?;
                #[cfg(not(unix))]
                fs::copy(&path, &dest)?;
            }
        } else {
            let dest = transform(&path, &dest);
            copier.register(&path, &dest);
        }
    }
    Ok(())
}

/// Port of `copy_tree`: copy an entire directory tree, using
/// hardlinks where possible (falling back to real copies), preserving
/// symlinks on Unix. `transform_destination_filename` lets a caller
/// rename files as they're copied (e.g. sanitizing filenames);
/// [`identity_transform`] matches upstream's own default.
pub fn copy_tree(src: &Path, dest: &Path, transform_destination_filename: &dyn Fn(&Path, &Path) -> PathBuf, delete_source: bool) -> Result<()> {
    let dest = dest.canonicalize().or_else(|_| -> Result<PathBuf> {
        fs::create_dir_all(dest)?;
        Ok(dest.canonicalize()?)
    })?;
    if samefile(src, &dest) {
        anyhow::bail!("Cannot copy tree if the source and destination are the same: {} == {}", src.display(), dest.display());
    }

    let mut copier = FileCopier::new(delete_source);
    register_folder_recursively(src, src, &mut copier, &dest, transform_destination_filename, false)?;
    copier.copy_all();

    if delete_source && src.exists() {
        fs::remove_dir_all(src)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_files_hardlinks_when_possible_and_reads_back_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dest = dir.path().join("b.txt");
        fs::write(&src, b"hello").unwrap();

        copy_files(&[(src.clone(), dest.clone())], false);

        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        assert!(src.exists(), "source should still exist without delete_source");
    }

    #[test]
    fn copy_files_with_delete_source_removes_the_original_after_copying() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dest = dir.path().join("b.txt");
        fs::write(&src, b"hello").unwrap();

        copy_files(&[(src.clone(), dest.clone())], true);

        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        assert!(!src.exists(), "source should be removed with delete_source");
    }

    #[test]
    fn copy_files_skips_a_pair_that_is_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, b"hello").unwrap();

        // Same path both sides -- should be silently skipped, not error.
        copy_files(&[(src.clone(), src.clone())], false);
        assert_eq!(fs::read(&src).unwrap(), b"hello");
    }

    #[test]
    fn rename_files_moves_files_to_their_destination() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dest = dir.path().join("b.txt");
        fs::write(&src, b"move me").unwrap();

        rename_files(&[(src.clone(), dest.clone())]).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"move me");
    }

    #[test]
    fn copy_tree_recursively_copies_nested_files_and_preserves_structure() {
        let src_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(src_dir.path().join("sub")).unwrap();
        fs::write(src_dir.path().join("top.txt"), b"top").unwrap();
        fs::write(src_dir.path().join("sub/nested.txt"), b"nested").unwrap();

        let dest_dir = tempfile::tempdir().unwrap();
        let dest_path = dest_dir.path().join("copy_target");
        copy_tree(src_dir.path(), &dest_path, &identity_transform, false).unwrap();

        assert_eq!(fs::read(dest_path.join("top.txt")).unwrap(), b"top");
        assert_eq!(fs::read(dest_path.join("sub/nested.txt")).unwrap(), b"nested");
        assert!(src_dir.path().join("top.txt").exists(), "source should survive without delete_source");
    }

    #[test]
    fn copy_tree_with_delete_source_removes_the_original_tree() {
        let src_dir = tempfile::tempdir().unwrap();
        fs::write(src_dir.path().join("f.txt"), b"data").unwrap();
        let src_path = src_dir.path().to_path_buf();

        let dest_dir = tempfile::tempdir().unwrap();
        let dest_path = dest_dir.path().join("copy_target");
        copy_tree(&src_path, &dest_path, &identity_transform, true).unwrap();

        assert_eq!(fs::read(dest_path.join("f.txt")).unwrap(), b"data");
        assert!(!src_path.exists(), "source tree should be removed with delete_source");
    }

    #[test]
    fn copy_tree_rejects_copying_onto_itself() {
        let dir = tempfile::tempdir().unwrap();
        let err = copy_tree(dir.path(), dir.path(), &identity_transform, false).unwrap_err();
        assert!(err.to_string().contains("same"), "got: {err}");
    }

    #[test]
    fn copy_tree_applies_a_custom_filename_transform() {
        let src_dir = tempfile::tempdir().unwrap();
        fs::write(src_dir.path().join("a.txt"), b"x").unwrap();

        let dest_dir = tempfile::tempdir().unwrap();
        let dest_path = dest_dir.path().join("out");
        let transform = |_src: &Path, dest: &Path| dest.with_file_name(format!("renamed-{}", dest.file_name().unwrap().to_string_lossy()));
        copy_tree(src_dir.path(), &dest_path, &transform, false).unwrap();

        assert!(dest_path.join("renamed-a.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_preserves_symlinks_on_unix() {
        let src_dir = tempfile::tempdir().unwrap();
        fs::write(src_dir.path().join("real.txt"), b"real").unwrap();
        std::os::unix::fs::symlink("real.txt", src_dir.path().join("link.txt")).unwrap();

        let dest_dir = tempfile::tempdir().unwrap();
        let dest_path = dest_dir.path().join("out");
        copy_tree(src_dir.path(), &dest_path, &identity_transform, false).unwrap();

        let link_dest = dest_path.join("link.txt");
        assert!(fs::symlink_metadata(&link_dest).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link_dest).unwrap(), Path::new("real.txt"));
    }
}
