use crate::cache::Cache;
use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::constants::LIBRARY_HANDLE_DIR_NAME;

/// Resolves the absolute path to the cover image for a given book.
///
/// # Arguments
/// * `cache` - The database cache.
/// * `book_id` - The ID of the book.
///
/// # Returns
/// * `Result<PathBuf>` - The absolute path to the cover image.
pub fn cover_path(cache: &Cache, book_id: i32) -> Result<PathBuf> {
    // An existing cover wherever it already is, in preference to where a
    // new one would go -- see `sidecar_cover_path` for why the location
    // changed, and `legacy_cover_path` for why the old one still works.
    if let Some(existing) = legacy_cover_path(cache, book_id)?.filter(|p| p.is_file()) {
        return Ok(existing);
    }
    sidecar_cover_path(cache, book_id)
}

/// Where a cover is written from now on: `.calibre-oxide/covers/<uuid>.jpg`.
///
/// It used to be `<book folder>/cover.jpg`, which cannot work once a
/// book's folder is wherever its files happen to sit (#889). In a flat
/// folder every book's "folder" is the library root, so every book would
/// share one `cover.jpg` — the second book added would overwrite the
/// first's cover, and every book would show the same picture.
///
/// Keyed by **uuid**, not by `books.id`. The id is a local autoincrement,
/// so two machines each adding a book both produce `1.jpg` and a sync
/// would have one cover overwrite an unrelated book's. The uuid is the
/// same on both.
pub fn sidecar_cover_path(cache: &Cache, book_id: i32) -> Result<PathBuf> {
    let uuid = cache.book_uuid(book_id)?.filter(|u| !u.is_empty()).with_context(|| format!("book {book_id} has no uuid to key its cover by"))?;
    Ok(cache.backend.library_path.join(LIBRARY_HANDLE_DIR_NAME).join("covers").join(format!("{uuid}.jpg")))
}

/// Where covers used to live, and still do for any library written
/// before this changed.
///
/// Read but never written. Returning `None` for a book with no folder
/// recorded keeps this from resolving to `<library>/cover.jpg`, which is
/// not any one book's cover.
pub fn legacy_cover_path(cache: &Cache, book_id: i32) -> Result<Option<PathBuf>> {
    match cache.field_for(book_id, "path")? {
        Some(rel) if !rel.is_empty() => Ok(Some(cache.backend.library_path.join(rel).join("cover.jpg"))),
        _ => Ok(None),
    }
}

/// Sets the cover image for a book and flips `has_cover` on. A no-op
/// if the book has no path yet (nothing has been added to it), same
/// as this crate's other file-management operations.
///
/// # Arguments
/// * `cache` - The database cache.
/// * `book_id` - The ID of the book.
/// * `data` - The raw image data.
pub fn set_cover(cache: &Cache, book_id: i32, data: &[u8]) -> Result<()> {
    // No longer requires the book to have a folder: covers live in the
    // library's own state directory now, so a book whose files sit in
    // the library root can still have one.
    //
    // Written to the sidecar even when a legacy cover exists beside the
    // book. Having one writer and one location is worth more than
    // keeping a library readable by real calibre, which the folder model
    // in #889 already gave up on.
    let path = sidecar_cover_path(cache, book_id)?;

    // Port of issue #93's crate-wide write-path retrofit: real,
    // journaled, crash-safe write through `LibraryHandle` instead of
    // a raw `fs::write` (`write_atomic` creates the parent directory
    // itself, so no separate `create_dir_all` is needed here anymore).
    let handle = cache.backend.write_handle()?;
    handle.write_atomic(&path, data)?;

    // Retire any cover sitting beside the book. `cover_path` prefers
    // an existing legacy file, so leaving it would mean the stale one
    // keeps winning every read and replacing a cover appears to do
    // nothing at all. Removing it here is what makes the fallback a
    // one-way migration rather than a permanent fork.
    if let Some(legacy) = legacy_cover_path(cache, book_id)?.filter(|p| p.is_file()) {
        if let Err(e) = handle.remove_atomic(&legacy) {
            log::warn!("wrote the new cover for book {book_id} but could not remove {}: {e}", legacy.display());
        }
    }

    // Port of docs/FAULT_TOLERANCE.md §8: "cover images... same
    // rule" as book-format files.
    cache.checksums().record_file(book_id, "cover", "", &path)?;
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET has_cover = 1 WHERE id = ?1", (book_id,))?;
    }

    // `ChangeOp::CoverSet` existed from the start and had no producer at
    // all, so a rebuild from the log lost every cover flag. Outside the
    // connection lock, since appending fsyncs.
    cache.record_cover_set(book_id, blake3::hash(data).to_hex().to_string());

    // Invalidate thumbnail cache if it existed (TODO)
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;

    /// Gives a book a folder of its own and moves its files into it.
    ///
    /// `add_book` no longer creates one (#893), and a book whose files sit
    /// in the library root has no "beside the book" to speak of --
    /// `legacy_cover_path` deliberately returns `None` there, because
    /// `<library>/cover.jpg` is nobody's cover in particular. A test whose
    /// subject *is* that old location has to arrange it.
    fn give_it_its_own_folder(dir: &tempfile::TempDir, cache: &Cache, book_id: i32) -> PathBuf {
        let folder = "An Author/A Book";
        let book_dir = dir.path().join(folder);
        std::fs::create_dir_all(&book_dir).unwrap();
        for (format, name) in cache.format_file_names(book_id).unwrap() {
            let file = format!("{name}.{}", format.to_lowercase());
            std::fs::rename(dir.path().join(&file), book_dir.join(&file)).unwrap();
        }
        cache.set_book_path(book_id, folder).unwrap();
        book_dir
    }

    fn library_with_book() -> (tempfile::TempDir, Cache, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("src.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "A Book".to_string();
        meta.authors = vec!["An Author".to_string()];
        let id = cache.add_book(&source, &meta).unwrap();
        (dir, cache, id)
    }

    #[test]
    fn a_cover_is_written_to_the_state_directory_not_beside_the_book() {
        let (dir, cache, id) = library_with_book();
        set_cover(&cache, id, b"cover bytes").unwrap();

        let path = cover_path(&cache, id).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"cover bytes");
        assert!(path.starts_with(dir.path().join(LIBRARY_HANDLE_DIR_NAME)), "{}", path.display());
        assert!(!dir.path().join("cover.jpg").exists(), "a new cover should not be written beside the book");
        assert!(cache.has_cover(id).unwrap());
    }

    /// The reason for the move: in a flat folder every book's folder is
    /// the library root, so one shared `cover.jpg` would mean the second
    /// book added overwrites the first's cover and every book shows the
    /// same picture.
    #[test]
    fn two_books_sharing_a_folder_get_different_covers() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();

        let mut ids = Vec::new();
        for name in ["one.pdf", "two.pdf"] {
            let source = dir.path().join(name);
            std::fs::write(&source, name).unwrap();
            let mut meta = MetaInformation::default();
            meta.title = "Same Title".to_string();
            meta.authors = vec!["Same Author".to_string()];
            ids.push(cache.add_book(&source, &meta).unwrap());
        }
        assert_eq!(cache.field_for(ids[0], "path").unwrap(), cache.field_for(ids[1], "path").unwrap(), "these books do share a folder");

        set_cover(&cache, ids[0], b"FIRST COVER").unwrap();
        set_cover(&cache, ids[1], b"SECOND COVER").unwrap();

        assert_ne!(cover_path(&cache, ids[0]).unwrap(), cover_path(&cache, ids[1]).unwrap());
        assert_eq!(std::fs::read(cover_path(&cache, ids[0]).unwrap()).unwrap(), b"FIRST COVER");
        assert_eq!(std::fs::read(cover_path(&cache, ids[1]).unwrap()).unwrap(), b"SECOND COVER");
    }

    /// A library written before the move keeps its covers.
    #[test]
    fn a_cover_already_beside_the_book_is_still_found() {
        let (dir, cache, id) = library_with_book();
        let legacy = give_it_its_own_folder(&dir, &cache, id).join("cover.jpg");
        std::fs::write(&legacy, b"OLD COVER").unwrap();

        assert_eq!(cover_path(&cache, id).unwrap(), legacy);
        assert_eq!(std::fs::read(cover_path(&cache, id).unwrap()).unwrap(), b"OLD COVER");
    }

    /// Replacing a legacy cover writes the new one to the sidecar, and
    /// the sidecar is what is read afterwards -- otherwise the stale file
    /// beside the book would keep winning and the replacement would look
    /// like it had no effect.
    #[test]
    fn replacing_a_legacy_cover_takes_effect() {
        let (dir, cache, id) = library_with_book();
        let legacy = give_it_its_own_folder(&dir, &cache, id).join("cover.jpg");
        std::fs::write(&legacy, b"OLD COVER").unwrap();

        set_cover(&cache, id, b"NEW COVER").unwrap();
        assert_eq!(std::fs::read(cover_path(&cache, id).unwrap()).unwrap(), b"NEW COVER");
    }

    /// Keyed by uuid rather than `books.id`: the id is a local
    /// autoincrement, so two machines each adding a book would both write
    /// `1.jpg` and a sync would give one book the other's cover.
    #[test]
    fn the_sidecar_is_keyed_by_uuid() {
        let (_dir, cache, id) = library_with_book();
        let uuid = cache.book_uuid(id).unwrap().unwrap();
        let path = sidecar_cover_path(&cache, id).unwrap();
        assert_eq!(path.file_name().unwrap().to_string_lossy(), format!("{uuid}.jpg"));
        assert!(!path.to_string_lossy().contains(&format!("/{id}.jpg")));
    }

    #[test]
    fn a_book_with_no_folder_has_no_legacy_cover_path() {
        let (_dir, cache, id) = library_with_book();
        cache.set_book_path(id, "").unwrap();
        // Not `<library>/cover.jpg`, which is nobody's cover.
        assert_eq!(legacy_cover_path(&cache, id).unwrap(), None);
    }

    /// A book whose files sit in the library root can still have a cover
    /// -- `set_cover` used to give up when the book had no folder.
    #[test]
    fn a_book_in_the_library_root_can_still_have_a_cover() {
        let (_dir, cache, id) = library_with_book();
        cache.set_book_path(id, "").unwrap();

        set_cover(&cache, id, b"cover bytes").unwrap();
        assert!(cache.has_cover(id).unwrap());
        assert_eq!(std::fs::read(cover_path(&cache, id).unwrap()).unwrap(), b"cover bytes");
    }
}
