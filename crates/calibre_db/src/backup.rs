use crate::cache::Cache;
use anyhow::{Context, Result};
use calibre_ebooks::opf::OpfMetadata;
use std::fs;
use std::sync::{Arc, Mutex};

/// Backs up the metadata for a book to an OPF file in its directory.
pub fn backup_metadata(cache: &Arc<Mutex<Cache>>, book_id: i32) -> Result<()> {
    let guard = cache.lock().unwrap();
    let backend = &guard.backend;

    // Fetch Basic Data
    let title = backend.field_for(book_id, "title")?.unwrap_or_default();
    let author_sort = backend
        .field_for(book_id, "author_sort")?
        .unwrap_or_default();
    let uuid = backend.field_for(book_id, "uuid")?;
    let path_rel = backend
        .field_for(book_id, "path")?
        .context("Book path missing")?;

    // Construct OpfMetadata
    let mut meta = OpfMetadata::default();
    meta.title = title;
    meta.authors = vec![author_sort]; // In a real app we'd parse authors correctly or fetch from authors table
    meta.uuid = uuid;

    // Generate XML
    let xml = meta.to_xml();

    // Write to file
    let book_dir = backend.library_path.join(path_rel);
    if !book_dir.exists() {
        fs::create_dir_all(&book_dir)?;
    }
    let opf_path = book_dir.join("metadata.opf");
    fs::write(opf_path, xml)?;

    Ok(())
}
