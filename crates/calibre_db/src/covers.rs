use crate::cache::Cache;
use anyhow::{Context, Result};
use std::fs;
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

/// Sets the cover image for a book.
///
/// # Arguments
/// * `cache` - The database cache.
/// * `book_id` - The ID of the book.
/// * `data` - The raw image data.
pub fn set_cover(cache: &Arc<Mutex<Cache>>, book_id: i32, data: &[u8]) -> Result<()> {
    let path = cover_path(cache, book_id)?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, data)?;

    // Invalidate thumbnail cache if it existed (TODO)
    Ok(())
}
