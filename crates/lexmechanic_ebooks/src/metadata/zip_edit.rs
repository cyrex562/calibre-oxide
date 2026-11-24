//! Replacing one entry inside a zip-based book, in place (#834).
//!
//! EPUB, ODT and DOCX all keep their metadata in an XML part inside a zip,
//! so writing metadata to any of them is the same operation: read the
//! archive, swap one entry, put the result back. Shared here rather than
//! copied per format, because the parts that are easy to get wrong are the
//! parts every caller needs.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use zip::ZipArchive;

/// Rewrites `archive_path` inside the zip at `path`, leaving every other
/// entry byte-identical.
///
/// Writes a whole new archive beside the original and renames over it, so
/// an interrupted write cannot truncate somebody's book.
pub fn replace_entry(path: &Path, archive_path: &str, content: &[u8]) -> Result<()> {
    let staging = tempfile::Builder::new().prefix("set-metadata").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    {
        // The source archive is opened *inside* this block so its file
        // handle is closed before the rename below. Windows refuses to
        // replace a file anything still has open ("Access is denied",
        // os error 5) where Unix allows it -- so on Linux this scoping
        // looks like style and on Windows it is the difference between
        // working and not. Do not collapse it.
        let mut archive = ZipArchive::new(std::fs::File::open(path)?)?;
        let mut out = zip::ZipWriter::new(std::fs::File::create(staging.path())?);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            // Each entry keeps its own compression method. That matters
            // for EPUB, whose `mimetype` must stay first and stored, and
            // costs nothing for the formats where it does not.
            let options = zip::write::FileOptions::default().compression_method(entry.compression());
            out.start_file(&name, options)?;
            if name == archive_path {
                out.write_all(content)?;
            } else {
                std::io::copy(&mut entry, &mut out)?;
            }
        }
        out.finish()?;
    }

    // `persist` renames, which is atomic within a filesystem -- and the
    // temp file is deliberately created beside the book so it is the same
    // one.
    staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
    Ok(())
}

/// Reads one entry's text out of the zip at `path`.
pub fn read_entry_text(path: &Path, archive_path: &str) -> Result<String> {
    use anyhow::Context;
    use std::io::Read;

    let mut archive = ZipArchive::new(std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?)?;
    let mut entry = archive.by_name(archive_path).with_context(|| format!("{archive_path} not found in {}", path.display()))?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(text)
}

/// Whether a `MetaInformation` field carries a real value, or one of
/// calibre's placeholders for "not known".
///
/// `MetaInformation::default()` is **not** empty: it sets
/// `title = "Unknown"`, `authors = ["Unknown"]` and `languages = ["und"]`,
/// the same placeholders the readers produce when a file says nothing. So
/// a metadata writer that treats "non-empty" as "the caller set this"
/// will happily write `Unknown` over a real title -- which is data loss
/// dressed up as an edit.
///
/// Every `set_metadata` routes its title/authors/languages through these.
pub mod placeholders {
    /// The placeholder title and author both readers and `Default` use.
    pub const UNKNOWN: &str = "Unknown";
    /// The placeholder language: BCP-47 "undetermined".
    pub const UNDETERMINED: &str = "und";

    pub fn real_title(title: &str) -> Option<&str> {
        let title = title.trim();
        if title.is_empty() || title == UNKNOWN {
            None
        } else {
            Some(title)
        }
    }

    /// `None` when the list is empty or is exactly the single `Unknown`
    /// placeholder. A list that merely *contains* "Unknown" alongside a
    /// real name is kept: that is a real co-author credit, oddly named.
    pub fn real_authors(authors: &[String]) -> Option<&[String]> {
        match authors {
            [] => None,
            [only] if only.trim() == UNKNOWN => None,
            _ => Some(authors),
        }
    }

    pub fn real_languages(languages: &[String]) -> Option<&[String]> {
        match languages {
            [] => None,
            [only] if only.trim() == UNDETERMINED => None,
            _ => Some(languages),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::placeholders::*;

    #[test]
    fn the_default_metainformation_looks_unset_to_a_writer() {
        // The whole point: a `Default` must not overwrite a real book.
        let mi = crate::metadata::MetaInformation::default();
        assert_eq!(real_title(&mi.title), None, "the default title is a placeholder");
        assert_eq!(real_authors(&mi.authors), None, "the default author is a placeholder");
        assert_eq!(real_languages(&mi.languages), None, "the default language is a placeholder");
    }

    #[test]
    fn real_values_pass_through() {
        assert_eq!(real_title("Dune"), Some("Dune"));
        assert_eq!(real_authors(&["Frank Herbert".to_string()]), Some(&["Frank Herbert".to_string()][..]));
        assert_eq!(real_languages(&["en".to_string()]), Some(&["en".to_string()][..]));
    }

    #[test]
    fn whitespace_and_empty_are_not_values() {
        assert_eq!(real_title("   "), None);
        assert_eq!(real_authors(&[]), None);
        assert_eq!(real_languages(&[]), None);
    }

    /// A book really credited to "Unknown" alongside a named author keeps
    /// both -- only the lone placeholder is ignored.
    #[test]
    fn unknown_beside_a_real_author_is_kept() {
        let authors = vec!["Ann Author".to_string(), "Unknown".to_string()];
        assert_eq!(real_authors(&authors), Some(&authors[..]));
    }
}
