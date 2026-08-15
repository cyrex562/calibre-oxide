//! Rewriting a single entry inside an existing zip archive, safely.
//!
//! Port of `safe_replace` in `old_src/src/calibre/utils/zipfile.py`.
//!
//! There's no existing "shared zip helper" module in this crate to
//! extend (`grep -rn safe_replace crates/` finds nothing, and the zip
//! reading/writing code that exists is all per-format: EPUB/DOCX/ODT
//! container code, not a generic archive-entry-rewrite utility), so this
//! is a new, self-contained module. It lives in `calibre_ebooks` rather
//! than `calibre_utils` because its only caller
//! (`oeb::iterator::bookmarks`) is here and `calibre_utils` does not
//! otherwise depend on the `zip` crate -- adding that dependency there
//! for a single ebook-specific helper isn't worth the new cross-crate
//! surface.
//!
//! # Why not Python's approach
//!
//! Python's `safe_replace` reads the whole source archive into a
//! `SpooledTemporaryFile`, rewrites it there, then does
//! `zipstream.seek(0); zipstream.truncate(); shutil.copyfileobj(...)`
//! against the *original* file handle. That truncate-then-copy step is
//! exactly the failure mode `docs/FAULT_TOLERANCE.md` §2 rules out: a
//! crash or `lid-close` between the `truncate()` and the copy finishing
//! leaves a zero-length (or partial) file where the book used to be.
//!
//! This port instead writes the *entire* new archive to a temp file
//! next to the target (so the final `rename` is same-filesystem and
//! atomic), `fsync`s it, and only then renames it over the original --
//! the write-temp/fsync/rename sequence from §2. The original file is
//! never truncated or opened for writing until the replacement is
//! already complete and durable on disk.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

/// Replace (or, if `add_missing` is set, add) the entry named `name`
/// inside the zip archive at `zip_path`, leaving every other entry
/// byte-for-byte unchanged in content (though not necessarily in
/// compressed representation -- see below). Writes atomically: on any
/// error before the final rename, `zip_path` is untouched.
///
/// Port of `calibre.utils.zipfile.safe_replace(zipstream, name,
/// datastream, add_missing=...)`, specialized to a file path instead of
/// an open stream (this crate's only caller,
/// `bookmarks::save_bookmarks`, always has a path, and a path is what
/// the atomic rename discipline needs anyway).
///
/// Unlike Python's version, which copies every *other* entry's raw
/// compressed bytes unchanged (`z.read_raw`/`raw_bytes=True`) purely as
/// a performance optimization, this port decompresses and recompresses
/// every entry when rebuilding the archive. The decompressed content is
/// identical either way; only the compressed byte layout can differ.
/// `add_missing` with no matching entry appends the new content as an
/// additional file at the end, matching Python's `extra_replacements`
/// end-of-archive placement (and its documented caveat: parent
/// directories for a newly added path are not created as separate zip
/// entries).
pub fn safe_replace(zip_path: &Path, name: &str, data: &[u8], add_missing: bool) -> Result<()> {
    let source = File::open(zip_path)
        .with_context(|| format!("Failed to open {} for reading", zip_path.display()))?;
    let mut archive = ZipArchive::new(source)
        .with_context(|| format!("Failed to read zip archive {}", zip_path.display()))?;

    let parent = zip_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp_path = parent.join(format!(
        ".{}.tmp-{}",
        zip_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_string()),
        uuid::Uuid::new_v4().simple()
    ));

    let write_result = (|| -> Result<()> {
        let tmp_file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
        let mut writer = ZipWriter::new(tmp_file);
        let mut found = false;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .with_context(|| format!("Failed to read entry {i}"))?;
            let entry_name = entry.name().to_string();
            let options = FileOptions::default()
                .compression_method(entry.compression())
                .last_modified_time(entry.last_modified());
            let options = match entry.unix_mode() {
                Some(mode) => options.unix_permissions(mode),
                None => options,
            };

            if entry_name == name {
                writer
                    .start_file(entry_name, options)
                    .context("Failed to start replacement entry")?;
                writer
                    .write_all(data)
                    .context("Failed to write replacement entry")?;
                found = true;
            } else {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry
                    .read_to_end(&mut buf)
                    .with_context(|| format!("Failed to read entry {entry_name}"))?;
                writer
                    .start_file(entry_name, options)
                    .context("Failed to start copied entry")?;
                writer
                    .write_all(&buf)
                    .context("Failed to write copied entry")?;
            }
        }

        if !found {
            if add_missing {
                writer
                    .start_file(name, FileOptions::default())
                    .context("Failed to start new entry")?;
                writer
                    .write_all(data)
                    .context("Failed to write new entry")?;
            } else {
                anyhow::bail!("No entry named {name} found in {}", zip_path.display());
            }
        }

        let tmp_file = writer.finish().context("Failed to finalize zip archive")?;
        tmp_file.sync_all().context("Failed to fsync temp file")?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    fs::rename(&tmp_path, zip_path).with_context(|| {
        format!(
            "Failed to replace {} with rewritten archive",
            zip_path.display()
        )
    })?;

    if let Ok(dir) = File::open(parent) {
        // Best-effort directory fsync (unsupported on some
        // platforms/filesystems, e.g. Windows) -- the rename above is
        // already durable-on-commit for the file itself.
        let _ = dir.sync_all();
    }

    Ok(())
}

/// Atomically (re)builds a brand-new zip archive at `dest_path`: `build`
/// is handed a [`ZipWriter`] over a temp file created next to
/// `dest_path`, and once it returns successfully the temp file is
/// `fsync`ed and renamed into place. On any error (from `build` or from
/// finalizing/renaming), `dest_path` is left untouched and the temp file
/// is removed.
///
/// This is the same write-temp/fsync/rename discipline as
/// [`safe_replace`] (`docs/FAULT_TOLERANCE.md` §2), factored out for
/// callers that need to write a *whole new* archive from scratch rather
/// than replace one entry inside an existing one -- notably
/// `oeb::polish::container::EpubContainer::commit_epub`, which rewrites
/// the entire zip from its exploded-directory working copy on every
/// commit (port of `calibre.ebooks.tweak.zip_rebuilder`). `safe_replace`
/// itself isn't reusable for that: it opens and iterates an *existing*
/// archive's entries, which doesn't fit "rebuild from a directory tree"
/// at all.
pub fn build_zip_atomic(
    dest_path: &Path,
    build: impl FnOnce(&mut ZipWriter<File>) -> Result<()>,
) -> Result<()> {
    let parent = dest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp_path = parent.join(format!(
        ".{}.tmp-{}",
        dest_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_string()),
        uuid::Uuid::new_v4().simple()
    ));

    let write_result = (|| -> Result<()> {
        let tmp_file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
        let mut writer = ZipWriter::new(tmp_file);
        build(&mut writer)?;
        let tmp_file = writer.finish().context("Failed to finalize zip archive")?;
        tmp_file.sync_all().context("Failed to fsync temp file")?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    fs::rename(&tmp_path, dest_path).with_context(|| {
        format!(
            "Failed to move newly built archive into place at {}",
            dest_path.display()
        )
    })?;

    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::FileOptions;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        for (name, data) in entries {
            zip.start_file(*name, FileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    fn read_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
        let file = File::open(path).ok()?;
        let mut archive = ZipArchive::new(file).ok()?;
        let mut entry = archive.by_name(name).ok()?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    #[test]
    fn replaces_existing_entry_preserving_others() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        make_zip(
            &zip_path,
            &[("mimetype", b"application/epub+zip"), ("a.txt", b"old")],
        );

        safe_replace(&zip_path, "a.txt", b"new", false).unwrap();

        assert_eq!(read_entry(&zip_path, "a.txt").unwrap(), b"new");
        assert_eq!(
            read_entry(&zip_path, "mimetype").unwrap(),
            b"application/epub+zip"
        );
    }

    #[test]
    fn missing_entry_without_add_missing_errors() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        make_zip(&zip_path, &[("a.txt", b"old")]);

        let result = safe_replace(&zip_path, "missing.txt", b"data", false);
        assert!(result.is_err());
        // Original archive must be untouched on failure.
        assert_eq!(read_entry(&zip_path, "a.txt").unwrap(), b"old");
    }

    #[test]
    fn missing_entry_with_add_missing_appends() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        make_zip(&zip_path, &[("a.txt", b"old")]);

        safe_replace(
            &zip_path,
            "META-INF/calibre_bookmarks.txt",
            b"bm-data",
            true,
        )
        .unwrap();

        assert_eq!(read_entry(&zip_path, "a.txt").unwrap(), b"old");
        assert_eq!(
            read_entry(&zip_path, "META-INF/calibre_bookmarks.txt").unwrap(),
            b"bm-data"
        );
    }

    #[test]
    fn no_temp_file_left_behind_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        make_zip(&zip_path, &[("a.txt", b"old")]);
        safe_replace(&zip_path, "a.txt", b"new", false).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn build_zip_atomic_writes_new_archive() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        build_zip_atomic(&zip_path, |writer| {
            writer.start_file("mimetype", FileOptions::default())?;
            writer.write_all(b"application/epub+zip")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            read_entry(&zip_path, "mimetype").unwrap(),
            b"application/epub+zip"
        );
    }

    #[test]
    fn build_zip_atomic_leaves_existing_archive_untouched_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("book.epub");
        make_zip(&zip_path, &[("a.txt", b"old")]);
        let result = build_zip_atomic(&zip_path, |_writer| anyhow::bail!("boom"));
        assert!(result.is_err());
        assert_eq!(read_entry(&zip_path, "a.txt").unwrap(), b"old");
    }
}
