//! DOCX output.
//!
//! This used to be a stub. It wrote `[Content_Types].xml`, `_rels/.rels`
//! and a `word/document.xml` containing the book's title followed by the
//! literal sentence "Converted content pending full implementation." --
//! and it is the plugin the output registry dispatches `.docx` to, so
//! converting any book to DOCX reported success and produced a file with
//! none of the book in it.
//!
//! The real writer ([`crate::docx::writer::from_html::convert`], ~35 PRs
//! of #23/#132) was finished and had no production caller: its own module
//! doc says "now FULLY ported". This wires it up.
//!
//! Found by asking whether a produced file satisfies its format (#939) --
//! the same question that turned up four EPUB defects, and the same shape
//! as #926's PDB debug dump sitting in front of a real converter.

use crate::conversion::options::ConversionOptions;
use crate::docx::writer::container::PageOptions;
use crate::docx::writer::from_html;
use crate::metadata::meta::MetaInformation;
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;

pub struct DOCXOutput;

impl DOCXOutput {
    pub fn new() -> Self {
        DOCXOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path, opts: &ConversionOptions) -> Result<()> {
        let mi = metadata_of(book);

        // Disclosed narrowing: `ConversionOptions` has no page-size,
        // margin or `preserve_cover_aspect_ratio` fields, so the writer
        // gets calibre's own recommended defaults (letter, one-inch
        // margins). Threading real page options through is a separate
        // change to `ConversionOptions`, and inventing fields here would
        // put them out of step with every other output plugin.
        let _ = opts;
        let page_options = PageOptions::default();

        // `add_cover` is false: a cover needs an image in the manifest and
        // the book may have none. The real writer resolves one when asked,
        // and demanding it here would fail conversions that worked before.
        let writer = from_html::convert(book, &page_options, &mi, true, false);

        let file = File::create(output_path).with_context(|| format!("creating {}", output_path.display()))?;
        writer.write(file, &mi).map_err(|e| anyhow::anyhow!("writing {}: {e}", output_path.display()))?;
        Ok(())
    }
}

/// The book's metadata as a [`MetaInformation`].
///
/// The inverse of `oeb::transforms::metadata::meta_info_to_oeb_metadata`,
/// narrowed to the six fields the DOCX property parts actually read
/// (`core_properties` uses title/authors/languages/tags/comments,
/// `app_properties` uses publisher). Adding more would be inventing
/// requirements this output does not have.
///
/// Terms are looked up both bare and `dc:`-prefixed, because both forms
/// occur: producers in this crate add bare ones, while a book read from an
/// OPF carries them namespaced.
fn metadata_of(book: &OEBBook) -> MetaInformation {
    let first = |terms: [&str; 2]| -> Option<String> { terms.iter().find_map(|t| book.metadata.get(t).first().map(|i| i.value.clone())) };
    let all = |terms: [&str; 2]| -> Vec<String> {
        for term in terms {
            let values: Vec<String> = book.metadata.get(term).iter().map(|i| i.value.clone()).collect();
            if !values.is_empty() {
                return values;
            }
        }
        Vec::new()
    };

    let mut mi = MetaInformation::default();
    if let Some(title) = first(["title", "dc:title"]).filter(|t| !t.trim().is_empty()) {
        mi.title = title;
    }
    let authors = all(["creator", "dc:creator"]);
    if !authors.is_empty() {
        mi.authors = authors;
    }
    let languages = all(["language", "dc:language"]);
    if !languages.is_empty() {
        mi.languages = languages;
    }
    mi.tags = all(["subject", "dc:subject"]);
    mi.publisher = first(["publisher", "dc:publisher"]).filter(|p| !p.trim().is_empty());
    mi.comments = first(["description", "dc:description"]).filter(|c| !c.trim().is_empty());
    mi
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn a_book(dir: &Path) -> OEBBook {
        std::fs::write(dir.join("c1.html"), "<html><body><h1>Chapter One</h1><p>The quick brown fox.</p></body></html>").unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(dir)));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.spine.add("c1", true);
        book.metadata.add("title", "A Book");
        book.metadata.add("creator", "Ann Author");
        book.metadata.add("language", "en");
        book
    }

    fn entries(path: &Path) -> Vec<String> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect()
    }

    fn part(path: &Path, name: &str) -> String {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    /// The headline: the book's own text reaches the document.
    ///
    /// This plugin used to emit the title plus the literal sentence
    /// "Converted content pending full implementation." while reporting
    /// success, so every DOCX conversion silently produced a file with
    /// none of the book in it.
    #[test]
    fn the_books_content_reaches_the_document() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let book = a_book(src.path());
        let path = out.path().join("book.docx");

        DOCXOutput::new().convert(&book, &path, &ConversionOptions::default()).unwrap();

        let document = part(&path, "word/document.xml");
        assert!(document.contains("The quick brown fox"), "the book's text is missing:\n{document}");
        assert!(document.contains("Chapter One"), "the heading is missing");
        assert!(!document.contains("pending full implementation"), "the stub text is still being written");
    }

    /// A real DOCX is an OPC package of eleven parts, not three. The
    /// metadata parts matter twice over: Word shows the title from them,
    /// and `metadata::docx::set_metadata` refuses a file without
    /// `docProps/core.xml`, so a book produced without one could not have
    /// metadata embedded into it afterwards either.
    #[test]
    fn the_package_has_the_parts_a_real_docx_has() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let book = a_book(src.path());
        let path = out.path().join("book.docx");

        DOCXOutput::new().convert(&book, &path, &ConversionOptions::default()).unwrap();

        let names = entries(&path);
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/core.xml",
            "docProps/app.xml",
            "word/document.xml",
            "word/styles.xml",
            "word/numbering.xml",
            "word/fontTable.xml",
            "word/_rels/document.xml.rels",
        ] {
            assert!(names.iter().any(|n| n == required), "{required} is missing from the package: {names:?}");
        }
    }

    /// The metadata round-trips through this crate's own DOCX reader --
    /// which it could not before, there being no `docProps/core.xml`.
    #[test]
    fn metadata_round_trips_through_the_docx_reader() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let book = a_book(src.path());
        let path = out.path().join("book.docx");

        DOCXOutput::new().convert(&book, &path, &ConversionOptions::default()).unwrap();

        let read_back = crate::metadata::docx::get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "A Book");
    }

    /// Terms are looked up bare *and* `dc:`-prefixed, because both occur:
    /// producers in this crate add bare ones, a book read from an OPF
    /// carries them namespaced.
    #[test]
    fn namespaced_metadata_terms_are_found_too() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("c1.html"), "<html><body><p>Text.</p></body></html>").unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(src.path())));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.spine.add("c1", true);
        book.metadata.add("dc:title", "Namespaced Title");
        book.metadata.add("dc:publisher", "A Publisher");

        let mi = metadata_of(&book);
        assert_eq!(mi.title, "Namespaced Title");
        assert_eq!(mi.publisher.as_deref(), Some("A Publisher"));
    }

    /// A book with no metadata at all must still convert, with the
    /// placeholders left alone rather than written out as real values.
    #[test]
    fn a_book_without_metadata_still_converts() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("c1.html"), "<html><body><p>Text.</p></body></html>").unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(src.path())));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.spine.add("c1", true);
        let path = out.path().join("book.docx");

        DOCXOutput::new().convert(&book, &path, &ConversionOptions::default()).unwrap();
        assert!(part(&path, "word/document.xml").contains("Text."));
    }
}
