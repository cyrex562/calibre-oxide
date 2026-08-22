use crate::cache::Cache;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Resolves the absolute path to the cover image for a given book.
///
/// # Arguments
/// * `cache` - The database cache.
/// * `book_id` - The ID of the book.
///
/// # Returns
/// * `Result<PathBuf>` - The absolute path to the cover image.
pub fn cover_path(cache: &Arc<Mutex<Cache>>, book_id: i32) -> Result<PathBuf> {
    let guard = cache.lock().unwrap();
    let relative_path = guard
        .field_for(book_id, "path")?
        .context("Book path not found in DB")?; // "path" field in DB contains relative folder path

    let library_path = &guard.backend.library_path;
    let mut path = library_path.join(relative_path);
    path.push("cover.jpg");

    Ok(path)
}

/// Sets the cover image for a book and flips `has_cover` on. A no-op
/// if the book has no path yet (nothing has been added to it), same
/// as this crate's other file-management operations.
///
/// # Arguments
/// * `cache` - The database cache.
/// * `book_id` - The ID of the book.
/// * `data` - The raw image data.
pub fn set_cover(cache: &Arc<Mutex<Cache>>, book_id: i32, data: &[u8]) -> Result<()> {
    {
        let guard = cache.lock().unwrap();
        match guard.field_for(book_id, "path")? {
            Some(p) if !p.is_empty() => {}
            _ => return Ok(()),
        }
    }

    let path = cover_path(cache, book_id)?;

    // Port of issue #93's crate-wide write-path retrofit: real,
    // journaled, crash-safe write through `LibraryHandle` instead of
    // a raw `fs::write` (`write_atomic` creates the parent directory
    // itself, so no separate `create_dir_all` is needed here anymore).
    let handle = {
        let guard = cache.lock().unwrap();
        guard.backend.write_handle()?
    };
    handle.write_atomic(&path, data)?;

    {
        let guard = cache.lock().unwrap();
        // Port of docs/FAULT_TOLERANCE.md §8: "cover images... same
        // rule" as book-format files.
        guard.checksums().record_file(book_id, "cover", "", &path)?;
        let conn = guard.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET has_cover = 1 WHERE id = ?1", (book_id,))?;
    }

    // Invalidate thumbnail cache if it existed (TODO)
    Ok(())
}
