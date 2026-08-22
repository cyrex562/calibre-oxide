use crate::cache::Cache;
use anyhow::{Context, Result};
use calibre_ebooks::metadata::MetaInformation;
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

    // Real per-author names (not `author_sort`, which is a single
    // book-level formatted display string) -- same join
    // `copy_to_library.rs`'s `real_authors` uses. Falls back to
    // treating `author_sort` as one author name if the book has no
    // real `authors` link rows (e.g. inserted via raw SQL in a test),
    // same heuristic this function used before this fix.
    let authors: Vec<String> = {
        let conn = backend.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT authors.name FROM books_authors_link \
             JOIN authors ON authors.id = books_authors_link.author \
             WHERE books_authors_link.book = ? ORDER BY books_authors_link.id",
        )?;
        let mut names = Vec::new();
        for row in stmt.query_map([book_id], |row| row.get::<_, String>(0))? {
            names.push(row?);
        }
        names
    };

    // Construct MetaInformation
    let mut meta = MetaInformation::default();
    meta.title = title;
    meta.authors = if authors.is_empty() {
        vec![author_sort.clone()]
    } else {
        authors
    };
    meta.author_sort = if author_sort.is_empty() {
        None
    } else {
        Some(author_sort)
    };
    meta.uuid = uuid;

    // Generate XML
    let xml = meta.to_xml();

    // Write to file. Port of issue #93's crate-wide write-path
    // retrofit: real, journaled, crash-safe write through
    // `LibraryHandle` instead of a raw `fs::write` (`write_atomic`
    // creates the parent book directory itself).
    let book_dir = backend.library_path.join(path_rel);
    let opf_path = book_dir.join("metadata.opf");
    backend
        .write_handle()?
        .write_atomic(&opf_path, xml.as_bytes())?;

    // Port of docs/FAULT_TOLERANCE.md §8: "sidecar files: same rule"
    // as book-format files.
    guard
        .checksums()
        .record_file(book_id, "opf", "", &opf_path)?;

    Ok(())
}
