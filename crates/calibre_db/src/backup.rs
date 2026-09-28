use crate::cache::Cache;
use crate::constants::LIBRARY_HANDLE_DIR_NAME;
use anyhow::{Context, Result};
use calibre_ebooks::metadata::MetaInformation;
use std::sync::{Arc, Mutex};

/// Where an OPF backup is written from now on:
/// `.calibre-oxide/opf/<uuid>.opf`.
///
/// It used to be `<book folder>/metadata.opf`, which cannot work once a
/// book's folder is wherever its files happen to sit (#889). In a flat
/// folder every book's "folder" is the library root, so every book would
/// share one `metadata.opf` and each backup would overwrite the last --
/// exactly the collision `covers::sidecar_cover_path` already moved out
/// of the book folder for the same reason.
///
/// Keyed by **uuid** for the same reason covers are: `books.id` is a
/// local autoincrement, so two machines each adding a book both produce
/// `1.opf` and a sync would have one book's metadata overwrite an
/// unrelated book's.
pub fn sidecar_opf_path(cache: &Cache, book_id: i32) -> Result<std::path::PathBuf> {
    let uuid = cache
        .book_uuid(book_id)?
        .filter(|u| !u.is_empty())
        .with_context(|| format!("book {book_id} has no uuid to key its metadata backup by"))?;
    Ok(cache
        .backend
        .library_path
        .join(LIBRARY_HANDLE_DIR_NAME)
        .join("opf")
        .join(format!("{uuid}.opf")))
}

/// Where OPF backups used to live, and still do for any library written
/// before this changed.
///
/// Read but never written. Returning `None` for a book with no folder
/// recorded keeps this from resolving to `<library>/metadata.opf`, which
/// is not any one book's metadata.
pub fn legacy_opf_path(cache: &Cache, book_id: i32) -> Result<Option<std::path::PathBuf>> {
    match cache.field_for(book_id, "path")? {
        Some(rel) if !rel.is_empty() => Ok(Some(cache.backend.library_path.join(rel).join("metadata.opf"))),
        _ => Ok(None),
    }
}

/// Backs up the metadata for a book to its OPF sidecar.
pub fn backup_metadata(cache: &Arc<Mutex<Cache>>, book_id: i32) -> Result<()> {
    let guard = cache.lock().unwrap();
    let backend = &guard.backend;

    // Fetch Basic Data
    let title = backend.field_for(book_id, "title")?.unwrap_or_default();
    let author_sort = backend
        .field_for(book_id, "author_sort")?
        .unwrap_or_default();
    let uuid = backend.field_for(book_id, "uuid")?;

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
    let opf_path = sidecar_opf_path(&guard, book_id)?;
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
