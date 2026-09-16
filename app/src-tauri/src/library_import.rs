//! Real "import a whole-library archive" (issue #761): the desktop-app
//! half of the export/import pair -- `crates/calibre_srv/src/library_export.rs`
//! streams the real zip archive of a library's own on-disk layout;
//! this module extracts one back to a real local path, then `lib.rs`'s
//! own `open_library` ("open a library at a path", issue #725) opens
//! it, exactly the same flow a freshly-picked or reopened library
//! already goes through.
//!
//! # Real zip-slip hardening, two layers
//!
//! `enclosed_name()` already rejects an entry containing `..` or an
//! absolute path (real, in the `zip` crate itself). This adds a
//! second, symlink-resolved check beyond that -- matches this
//! project's established path-traversal-hardening pattern (see
//! `crates/calibre_srv/src/save_to_disk.rs`'s own doc for the same
//! two-layer shape and why a lexical check alone isn't enough): a
//! symlink pre-planted inside the destination (by a previous partial
//! extraction, for instance) could otherwise redirect a lexically-safe
//! path outside it.

use std::path::{Path, PathBuf};

/// Extracts every entry of the real zip at `archive_path` into
/// `dest_root` (created if it doesn't exist yet). Returns an error
/// (not a partial-success value) on the first problem -- an import
/// that partially wrote a corrupt library is worse than one that
/// wrote nothing.
pub fn extract_zip(archive_path: &Path, dest_root: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| format!("opening the archive: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("reading the archive: {e}"))?;

    std::fs::create_dir_all(dest_root).map_err(|e| e.to_string())?;
    let canonical_dest = std::fs::canonicalize(dest_root).map_err(|e| e.to_string())?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel_path) = entry.enclosed_name().map(Path::to_path_buf) else {
            return Err(format!("refusing to extract an unsafe archive entry: {}", entry.name()));
        };
        let is_dir = entry.name().ends_with('/');
        let out_path: PathBuf = dest_root.join(&rel_path);

        let parent = if is_dir { out_path.as_path() } else { out_path.parent().unwrap_or(dest_root) };
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

        let canonical_parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
        if !canonical_parent.starts_with(&canonical_dest) {
            return Err(format!("refusing to extract outside the destination: {}", out_path.display()));
        }

        if is_dir {
            continue;
        }
        let mut out_file = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, options).unwrap();
                std::io::Write::write_all(&mut zip, content).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extracts_a_real_archive_with_nested_directories() {
        let src_dir = tempfile::tempdir().unwrap();
        let archive_path = src_dir.path().join("lib.zip");
        std::fs::write(&archive_path, make_zip(&[("metadata.db", b"real db bytes"), ("Author/Title (1)/Title.txt", b"real book bytes")])).unwrap();

        let dest = src_dir.path().join("extracted");
        extract_zip(&archive_path, &dest).unwrap();

        assert_eq!(std::fs::read(dest.join("metadata.db")).unwrap(), b"real db bytes");
        assert_eq!(std::fs::read(dest.join("Author/Title (1)/Title.txt")).unwrap(), b"real book bytes");
    }

    #[test]
    fn a_zip_slip_entry_is_refused() {
        // A raw "../../etc/passwd"-style entry name is exactly what
        // `enclosed_name()` is real, in-crate protection against --
        // confirm this port actually surfaces that as a real error
        // rather than silently skipping or panicking.
        let src_dir = tempfile::tempdir().unwrap();
        let archive_path = src_dir.path().join("evil.zip");
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::default();
            zip.start_file("../../escaped.txt", options).unwrap();
            std::io::Write::write_all(&mut zip, b"should never land here").unwrap();
            zip.finish().unwrap();
        }
        std::fs::write(&archive_path, buf).unwrap();

        let dest = src_dir.path().join("extracted");
        let result = extract_zip(&archive_path, &dest);
        assert!(result.is_err(), "a zip-slip entry must be refused");
    }
}
