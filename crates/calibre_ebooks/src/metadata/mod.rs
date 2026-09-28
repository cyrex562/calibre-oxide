pub mod archive;
pub mod author_mapper;
pub mod authors;
pub mod azw4;
pub mod chm;
pub mod docx;
pub mod epub;
pub mod ereader;
pub mod extz;
pub mod fb2;
pub mod haodoo;
pub mod html;
pub mod imp;
pub mod kfx;
pub mod lit;
pub mod lrf;
pub mod lrx;
pub mod meta;
pub mod mobi;
pub mod odt;
pub mod pdb;
pub mod pdf;
pub mod plucker;
pub mod pml;
pub mod rar;
pub mod rb;
pub mod rtf;
pub mod search_internet;
pub mod snb;
pub mod sources;
pub mod tag_mapper;
pub mod toc;
pub mod topaz;
pub mod txt;
pub mod utils;
pub mod worker;
pub mod xmp;
pub mod zip;
pub mod zip_edit;

// Re-export commonly used items
pub use archive::{archive_type, get_comic_metadata, is_comic, parse_comic_comment};
pub use author_mapper::{cap_author_token, compile_rules, map_authors, Rule};
pub use authors::{author_to_author_sort, authors_to_string, string_to_authors};
pub use meta::{check_isbn, title_sort, MetaInformation};

use anyhow::{bail, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Whether [`set_metadata`] can write to a file of this extension.
///
/// An explicit list rather than "try it and see", so a caller can tell a
/// user "this format is not supported yet" before opening the file, and
/// distinguish that from a write that was attempted and failed. Kept
/// beside the dispatch below so the two cannot drift -- a test asserts
/// they agree.
pub fn can_set_metadata(extension: &str) -> bool {
    matches!(extension.to_lowercase().as_str(), "epub" | "odt" | "docx" | "pdf" | "fb2" | "rtf" | "htmlz" | "txtz" | "mobi" | "azw" | "prc")
}

/// Writes `mi` into the book file at `path`, in place (#834).
///
/// The write counterpart to [`get_metadata`], and upstream's
/// `metadata/meta.py`'s `set_metadata` in shape. Lives here rather than in
/// `calibre_db` because knowing which formats can be written is the format
/// crate's business -- a caller that is not a library (a CLI, the content
/// server) needs it too.
///
/// Errors for a format with no writer. [`can_set_metadata`] answers that
/// question without opening anything.
pub fn set_metadata<P: AsRef<Path>>(path: P, mi: &MetaInformation) -> Result<()> {
    let path = path.as_ref();
    let ext = path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).unwrap_or_default();

    match ext.as_str() {
        "epub" => epub::set_metadata(path, mi),
        "odt" => odt::set_metadata(path, mi),
        "docx" => docx::set_metadata(path, mi),
        "pdf" => pdf::set_metadata(path, mi),
        "fb2" => fb2::set_metadata(path, mi),
        "rtf" => rtf::set_metadata(path, mi),
        "htmlz" | "txtz" => extz::set_metadata(path, mi),
        // AZW3/KF8 is deliberately absent: its record layout differs and
        // this writer is the MOBI6 one.
        "mobi" | "azw" | "prc" => mobi::set_metadata(path, mi),
        // Named rather than lumped together: "no writer for MOBI yet" is
        // a different thing to tell a user than "that is not a book".
        "azw3" | "lit" | "rb" | "imp" | "lrf" | "lrx" | "azw4" | "chm" | "snb" | "pdb" | "updb" | "txt" | "html" | "htm" | "xhtml" | "zip" | "cbz" | "rar" | "cbr" => {
            bail!("no metadata writer for {ext} yet")
        }
        _ => bail!("Unsupported format: {}", ext),
    }
}

pub fn get_metadata<P: AsRef<Path>>(path: P) -> Result<MetaInformation> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    let file = File::open(path)?;
    let stream = BufReader::new(file);

    match ext.as_str() {
        "epub" => epub::get_metadata(stream),
        "mobi" | "prc" | "azw" | "azw3" => mobi::get_metadata(stream),
        "fb2" => fb2::get_metadata(stream),
        "lit" => lit::get_metadata(stream),
        "pdf" => pdf::get_metadata(stream),
        "rb" => rb::get_metadata(stream),
        "imp" => imp::get_metadata(stream),
        "lrf" | "lrx" => lrx::get_metadata(stream),
        "azw4" => azw4::get_metadata(stream),
        "chm" => chm::get_metadata(stream),
        "docx" => docx::get_metadata(stream),
        "odt" => odt::get_metadata(stream),
        "snb" => snb::get_metadata(stream),
        "pdb" | "updb" => pdb::get_metadata(stream), // PDB dispatcher?
        "txt" => txt::get_metadata(stream),
        "rtf" => rtf::get_metadata(stream),
        "html" | "htm" | "xhtml" => html::get_metadata(stream),
        // EXTZ: a zip of content plus an OPF. Both were missing from this
        // dispatcher, so a `.htmlz` -- which this project both converts to
        // and from -- had no readable metadata at all.
        "htmlz" | "txtz" => extz::get_metadata(stream),
        "zip" | "cbz" => zip::get_metadata(stream),
        "rar" | "cbr" => rar::get_metadata(stream),
        // "xmp" => xmp::get_metadata(stream), // XMP usually sidecar?
        _ => bail!("Unsupported format: {}", ext),
    }
}

#[cfg(test)]
mod set_metadata_dispatch_tests {
    use super::*;

    /// `can_set_metadata` and `set_metadata` must agree. A disagreement is
    /// silent in both directions: a caller either refuses a format that
    /// works, or attempts one that does not and reports a confusing error.
    #[test]
    fn the_supported_list_matches_what_the_dispatch_actually_writes() {
        let dir = tempfile::tempdir().unwrap();
        // Every extension `get_metadata` recognises, so a format added to
        // the reader and forgotten by the writer still lands in one branch
        // or the other rather than being untested.
        let extensions = [
            "epub", "mobi", "prc", "azw", "azw3", "fb2", "lit", "pdf", "rb", "imp", "lrf", "lrx", "azw4", "chm", "docx", "odt", "snb", "pdb", "updb", "txt",
            "rtf", "html", "htm", "xhtml", "zip", "cbz", "rar", "cbr", "htmlz", "txtz",
        ];

        for ext in extensions {
            let path = dir.path().join(format!("book.{ext}"));
            // Deliberately not a valid file of that type: this asserts
            // which *branch* the dispatch takes, and an unsupported format
            // must be refused before anything is opened.
            std::fs::write(&path, b"not really a book").unwrap();

            let claimed = can_set_metadata(ext);
            let result = set_metadata(&path, &MetaInformation::default());

            if claimed {
                // A supported format may still fail on rubbish content --
                // what it must not do is say "no writer".
                if let Err(e) = result {
                    let message = format!("{e:#}");
                    assert!(!message.contains("no metadata writer"), "{ext} is claimed as supported but the dispatch has no writer for it: {message}");
                    assert!(!message.contains("Unsupported format"), "{ext} is claimed as supported but the dispatch rejects it: {message}");
                }
            } else {
                let message = format!("{:#}", result.expect_err("an unsupported format must not silently succeed"));
                assert!(
                    message.contains("no metadata writer") || message.contains("Unsupported format"),
                    "{ext} is not claimed as supported, so it should be refused as unwritable, got: {message}"
                );
            }
        }
    }

    /// The formats with writers, named explicitly so removing one is a
    /// deliberate act rather than a quiet regression.
    #[test]
    fn the_six_written_formats_are_claimed() {
        for ext in ["epub", "odt", "docx", "pdf", "fb2", "rtf", "htmlz", "txtz", "mobi", "azw", "prc"] {
            assert!(can_set_metadata(ext), "{ext} should be writable");
            assert!(can_set_metadata(&ext.to_uppercase()), "{ext} should be recognised case-insensitively");
        }
    }

    #[test]
    fn an_unknown_extension_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xyz");
        std::fs::write(&path, b"whatever").unwrap();

        assert!(!can_set_metadata("xyz"));
        assert!(set_metadata(&path, &MetaInformation::default()).is_err());
    }
}
