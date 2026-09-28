//! Embedding a book's metadata into its own files (#834).
//!
//! `calibredb embed_metadata` used to write only an OPF sidecar, with a
//! comment saying real embedding "would require `calibre_ebooks` support
//! for writing those formats with metadata". That support now exists for
//! the zip-based formats, so this is the dispatch.
//!
//! Formats with no writer yet are **reported as skipped**, not silently
//! passed over. A command that says it embedded metadata into a MOBI it
//! did not touch is worse than one that admits the gap.

use anyhow::{Context, Result};
use calibre_ebooks::metadata::MetaInformation;

use crate::cache::Cache;

/// What happened to one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatOutcome {
    /// Metadata was written into the file.
    Embedded,
    /// No writer exists for this format yet.
    NoWriter,
    /// The file the database points at is not there.
    FileMissing,
    Failed(String),
}

/// Formats this can write metadata into.
///
/// Delegates to `calibre_ebooks`, which owns the dispatch: duplicating the
/// list here is how the two would drift, and a `has_writer` that disagreed
/// with the writer would report `NoWriter` for a format that works, or
/// try one that does not.
pub fn has_writer(format: &str) -> bool {
    calibre_ebooks::metadata::can_set_metadata(format)
}

/// Assembles the metadata to embed from the library's own record.
///
/// Deliberately fuller than [`crate::backup::backup_metadata`]'s version,
/// which only needs title/authors/uuid for its OPF: the format writers can
/// carry publisher, tags, languages and comments too, and leaving them out
/// would mean an "embed metadata" that embedded a third of it.
pub fn metadata_for_book(cache: &Cache, book_id: i32) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();

    if let Some(title) = cache.field_for(book_id, "title")? {
        if !title.is_empty() {
            mi.title = title;
        }
    }
    // `field_for` returns authors pre-joined with " & ", which is how the
    // rest of this crate renders them.
    if let Some(authors) = cache.field_for(book_id, "authors")? {
        let names: Vec<String> = authors.split(" & ").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if !names.is_empty() {
            mi.authors = names;
        }
    }
    mi.publisher = cache.field_for(book_id, "publisher")?.filter(|p| !p.trim().is_empty());
    mi.comments = cache.field_for(book_id, "comments")?.filter(|c| !c.trim().is_empty());
    mi.series = cache.field_for(book_id, "series")?.filter(|s| !s.trim().is_empty());

    if let Some(tags) = cache.field_for(book_id, "tags")? {
        mi.tags = tags.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
    if let Some(languages) = cache.field_for(book_id, "languages")? {
        let langs: Vec<String> = languages.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if !langs.is_empty() {
            mi.languages = langs;
        }
    }
    if let Some(index) = cache.field_for(book_id, "series_index")? {
        if let Ok(parsed) = index.parse::<f64>() {
            mi.series_index = parsed;
        }
    }
    if let Some(identifiers) = cache.field_for(book_id, "identifiers")? {
        for pair in identifiers.split(',') {
            if let Some((kind, value)) = pair.split_once(':') {
                let (kind, value) = (kind.trim(), value.trim());
                if !kind.is_empty() && !value.is_empty() {
                    mi.identifiers.insert(kind.to_lowercase(), value.to_string());
                }
            }
        }
    }
    Ok(mi)
}

/// Writes the library's metadata into every one of a book's files that has
/// a writer.
///
/// Returns `(format, outcome)` per format, in the order the database lists
/// them.
pub fn embed_metadata(cache: &Cache, book_id: i32) -> Result<Vec<(String, FormatOutcome)>> {
    let mi = metadata_for_book(cache, book_id)?;
    let book_dir = cache.book_dir(book_id)?.with_context(|| format!("no book with id {book_id}"))?;

    let mut outcomes = Vec::new();
    for (format, name) in cache.format_file_names(book_id)? {
        if !has_writer(&format) {
            outcomes.push((format, FormatOutcome::NoWriter));
            continue;
        }
        let path = book_dir.join(format!("{name}.{}", format.to_lowercase()));
        if !path.is_file() {
            outcomes.push((format, FormatOutcome::FileMissing));
            continue;
        }

        match calibre_ebooks::metadata::set_metadata(&path, &mi) {
            Ok(()) => {
                // The file's content changed, so the recorded hash is now
                // wrong. Leaving it stale would make the next scan report
                // the book as edited outside the app (#894).
                if let Err(e) = cache.checksums().record_file(book_id, "format", &format.to_uppercase(), &path) {
                    log::warn!("embedded metadata into {} but could not re-record its checksum: {e}", path.display());
                }
                outcomes.push((format, FormatOutcome::Embedded));
            }
            Err(e) => outcomes.push((format, FormatOutcome::Failed(format!("{e:#}")))),
        }
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A real EPUB, minimal but valid enough to read metadata back out of.
    fn write_epub(path: &std::path::Path) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        zip.start_file("mimetype", zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        let deflated = zip::write::FileOptions::default();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("content.opf", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Stale Title</dc:title><dc:identifier id="uid">urn:uuid:test</dc:identifier></metadata><manifest><item id="c1" href="c1.html" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
        zip.start_file("c1.html", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Text</p></body></html>"#).unwrap();
        zip.finish().unwrap();
    }

    fn library_with_an_epub() -> (tempfile::TempDir, Cache, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book = dir.path().join("book.epub");
        write_epub(&book);

        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Real Title".to_string();
        meta.authors = vec!["Ann Author".to_string()];
        let book_id = cache.register_book_in_place("book.epub", &meta).unwrap();
        (dir, cache, book_id)
    }

    /// The headline: what the library knows reaches the file on disk.
    #[test]
    fn embedding_writes_the_librarys_metadata_into_the_epub() {
        let (dir, cache, book_id) = library_with_an_epub();
        cache.set_field(book_id, "title", "Corrected Title").unwrap();

        let outcomes = embed_metadata(&cache, book_id).unwrap();
        assert_eq!(outcomes, vec![("EPUB".to_string(), FormatOutcome::Embedded)], "{outcomes:?}");

        let on_disk = calibre_ebooks::metadata::get_metadata(&dir.path().join("book.epub")).unwrap();
        assert_eq!(on_disk.title, "Corrected Title", "the file should carry the library's title, not its own stale one");
    }

    /// A format with no writer is named, not silently skipped.
    #[test]
    fn a_format_without_a_writer_is_reported() {
        let (dir, cache, book_id) = library_with_an_epub();
        // LIT, not MOBI: MOBI gained a writer (#834), and a test that used
        // it to stand for "unsupported" would silently stop testing the
        // NoWriter path rather than failing.
        let lit = dir.path().join("book.lit");
        std::fs::write(&lit, b"not really a lit").unwrap();
        cache.add_format(book_id, &lit, "LIT", true).unwrap();

        let outcomes = embed_metadata(&cache, book_id).unwrap();
        let lit_outcome = outcomes.iter().find(|(f, _)| f == "LIT").map(|(_, o)| o.clone());
        assert_eq!(lit_outcome, Some(FormatOutcome::NoWriter), "{outcomes:?}");
    }

    /// Embedding changes the file, so its recorded checksum has to be
    /// updated -- otherwise the next scan reports the book as edited
    /// outside the app (#894).
    #[test]
    fn the_recorded_checksum_is_updated_so_the_next_scan_sees_no_drift() {
        let (dir, cache, book_id) = library_with_an_epub();
        let before = cache.checksums().recorded_identity(book_id, "format", "EPUB").unwrap().0;

        cache.set_field(book_id, "title", "Corrected Title").unwrap();
        embed_metadata(&cache, book_id).unwrap();

        let after = cache.checksums().recorded_identity(book_id, "format", "EPUB").unwrap().0;
        assert!(after.is_some(), "a checksum should still be recorded");
        assert_ne!(after, before, "the checksum should have been re-recorded after the file changed");

        // And the real proof: a drift check sees nothing edited.
        let scanned = crate::scan::walk(dir.path(), &crate::scan::ScanOptions::default(), std::time::SystemTime::now() + std::time::Duration::from_secs(3600)).unwrap();
        let drift = crate::drift::detect(&cache, &scanned).unwrap();
        assert!(drift.edited.is_empty(), "the book should not look edited: {:?}", drift.edited);
    }

    #[test]
    fn metadata_is_assembled_from_the_librarys_own_fields() {
        let (_dir, cache, book_id) = library_with_an_epub();
        cache.set_field(book_id, "publisher", "Real Publisher").unwrap();
        cache.set_field(book_id, "tags", "Science Fiction, Classics").unwrap();
        cache.set_field(book_id, "languages", "en").unwrap();

        let mi = metadata_for_book(&cache, book_id).unwrap();
        assert_eq!(mi.publisher.as_deref(), Some("Real Publisher"));
        assert_eq!(mi.tags, vec!["Science Fiction".to_string(), "Classics".to_string()]);
        assert_eq!(mi.languages, vec!["en".to_string()]);
    }

    #[test]
    fn has_writer_is_case_insensitive_and_honest_about_gaps() {
        assert!(has_writer("epub") && has_writer("EPUB"));
        assert!(has_writer("odt") && has_writer("docx"));
        assert!(has_writer("pdf") && has_writer("PDF"));
        assert!(has_writer("fb2") && has_writer("FB2"));
        assert!(has_writer("rtf") && has_writer("RTF"));
        assert!(has_writer("mobi") && has_writer("MOBI"));
        assert!(has_writer("htmlz") && has_writer("txtz"));
        // AZW3 stays absent on purpose: its record layout differs from
        // MOBI6's and the writer is the MOBI6 one.
        for absent in ["AZW3", "LIT", "SNB", "PDB"] {
            assert!(!has_writer(absent), "{absent} has no writer yet and must not claim one");
        }
    }
}
