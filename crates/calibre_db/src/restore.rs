use crate::cache::Cache;
use anyhow::{Context, Result};
use calibre_ebooks::opf::parse_opf;
use std::fs;
use std::sync::{Arc, Mutex};

/// Restores basic metadata (Title, Authors, UUID) from the OPF file in the book's directory.
pub fn restore_from_opf(cache: &Arc<Mutex<Cache>>, book_id: i32) -> Result<()> {
    // 1. Get path to OPF
    let opf_path = {
        let guard = cache.lock().unwrap();
        let path_rel = guard
            .backend
            .field_for(book_id, "path")?
            .context("Book path missing")?;
        guard
            .backend
            .library_path
            .join(path_rel)
            .join("metadata.opf")
    };

    if !opf_path.exists() {
        // No backup exists, nothing to restore
        return Ok(());
    }

    // 2. Read and Parse OPF
    let content = fs::read_to_string(&opf_path)?;
    let meta = parse_opf(&content)?;

    // 3. Update DB
    // We strictly use the write module or backend update.
    // Ideally we should use a transaction here.
    let guard = cache.lock().unwrap();
    let backend = &guard.backend;

    if !meta.title.is_empty() {
        backend.update(book_id, "title", &meta.title)?;
    }

    // For authors, our Update currently only supports updating `author_sort` string directly on books table
    // A full restore would need to update the authors table and links.
    // For this sprint we update what we can: author_sort if available.
    if !meta.authors.is_empty() {
        // Simplified: use first author as sort
        backend.update(book_id, "author_sort", &meta.authors[0])?;
    }

    if let Some(uuid) = meta.uuid {
        backend.update(book_id, "uuid", &uuid)?;
    }

    Ok(())
}
