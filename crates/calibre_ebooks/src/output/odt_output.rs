use crate::conversion::options::ConversionOptions;
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use std::fs;
use std::io::Write;
use std::path::Path;
use zip::write::FileOptions;
use zip::ZipWriter;

pub struct ODTOutput;

impl ODTOutput {
    pub fn new() -> Self {
        ODTOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path, _opts: &ConversionOptions) -> Result<()> {
        let file = fs::File::create(output_path)?;
        let mut zip = ZipWriter::new(file);

        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o755);

        // mimetype
        zip.start_file("mimetype", options)?;
        zip.write_all(b"application/vnd.oasis.opendocument.text")?;

        // META-INF/manifest.xml
        zip.add_directory("META-INF", options)?;
        zip.start_file("META-INF/manifest.xml", options)?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
 <manifest:file-entry manifest:full-path="/" manifest:version="1.2" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"#)?;

        // meta.xml -- where ODF keeps the document's metadata.
        //
        // It was not written at all, so a converted ODT had no title and
        // `metadata::odt::set_metadata` could not add one afterwards
        // either: that function requires this part and errors with
        // "No meta.xml in ODT" without it. Found by asking whether the
        // file we produce satisfies its format (#939).
        //
        // Declared in the manifest above as well, because ODF's manifest
        // is expected to list every part -- an undeclared one is the kind
        // of thing a strict reader rejects.
        zip.start_file("meta.xml", options)?;
        zip.write_all(build_meta_xml(book).as_bytes())?;

        // content.xml
        // Convert HTML to simple ODT XML paragraphs
        let mut content_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2">
 <office:body>
  <office:text>
"#,
        );

        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let html = String::from_utf8_lossy(&data);
                    let text = html2text(&html);

                    for line in text.lines() {
                        if !line.trim().is_empty() {
                            content_xml.push_str("   <text:p>");
                            content_xml.push_str(&html_escape::encode_text(line));
                            content_xml.push_str("</text:p>\n");
                        }
                    }
                }
            }
        }

        content_xml.push_str(
            r#"  </office:text>
 </office:body>
</office:document-content>"#,
        );

        zip.start_file("content.xml", options)?;
        zip.write_all(content_xml.as_bytes())?;

        zip.finish()?;
        Ok(())
    }
}

/// The ODF `meta.xml` for this book.
///
/// Writes the same fields `metadata::odt::get_metadata` reads back, so a
/// converted ODT round-trips: `dc:title`, `dc:creator` (one joined string,
/// as ODT's is single-valued), `dc:language`, `dc:description`, and
/// `meta:keyword` per tag.
///
/// The publisher goes to `meta:user-defined name="opf.publisher"` -- ODF
/// has no `dc:publisher`, and that is the field the reader looks in.
fn build_meta_xml(book: &OEBBook) -> String {
    let mi = super::metadata_of(book);
    let esc = |s: &str| html_escape::encode_text(s).to_string();

    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.2">
 <office:meta>
"#,
    );
    // Guarded by the same placeholder rule the metadata writers use: a
    // book with no real title must not get "Unknown" written into it.
    if let Some(title) = crate::metadata::zip_edit::placeholders::real_title(&mi.title) {
        out.push_str(&format!("  <dc:title>{}</dc:title>\n", esc(title)));
    }
    if let Some(authors) = crate::metadata::zip_edit::placeholders::real_authors(&mi.authors) {
        out.push_str(&format!("  <dc:creator>{}</dc:creator>\n", esc(&authors.join(" & "))));
    }
    if let Some(languages) = crate::metadata::zip_edit::placeholders::real_languages(&mi.languages) {
        out.push_str(&format!("  <dc:language>{}</dc:language>\n", esc(&languages[0])));
    }
    if let Some(comments) = mi.comments.as_deref().filter(|c| !c.trim().is_empty()) {
        out.push_str(&format!("  <dc:description>{}</dc:description>\n", esc(comments)));
    }
    for tag in mi.tags.iter().filter(|t| !t.trim().is_empty()) {
        out.push_str(&format!("  <meta:keyword>{}</meta:keyword>\n", esc(tag)));
    }
    if let Some(publisher) = mi.publisher.as_deref().filter(|p| !p.trim().is_empty()) {
        out.push_str(&format!("  <meta:user-defined meta:name=\"opf.publisher\">{}</meta:user-defined>\n", esc(publisher)));
    }
    out.push_str(" </office:meta>\n</office:document-meta>");
    out
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
        book.metadata.add("publisher", "A Publisher");
        book.metadata.add("subject", "sf");
        book
    }

    fn convert(dir: &tempfile::TempDir, book: &OEBBook) -> std::path::PathBuf {
        let path = dir.path().join("book.odt");
        ODTOutput::new().convert(book, &path, &ConversionOptions::default()).unwrap();
        path
    }

    /// `meta.xml` was not written at all, so a converted ODT had no title
    /// -- and `metadata::odt::set_metadata` could not add one afterwards
    /// either, since it requires this part.
    #[test]
    fn metadata_round_trips_through_the_odt_reader() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let path = convert(&out, &a_book(src.path()));

        let read_back = crate::metadata::odt::get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "A Book");
        assert_eq!(read_back.authors, vec!["Ann Author".to_string()]);
        assert_eq!(read_back.languages, vec!["en".to_string()]);
        assert_eq!(read_back.publisher.as_deref(), Some("A Publisher"));
        assert!(read_back.tags.contains(&"sf".to_string()), "{:?}", read_back.tags);
    }

    /// And the embedding path now works on a file we produced, which is
    /// what `calibredb embed_metadata` needs.
    #[test]
    fn a_produced_odt_accepts_embedded_metadata_afterwards() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let path = convert(&out, &a_book(src.path()));

        let mut mi = crate::metadata::meta::MetaInformation::default();
        mi.title = "Corrected Title".to_string();
        crate::metadata::odt::set_metadata(&path, &mi).expect("a produced ODT should accept embedded metadata");

        assert_eq!(crate::metadata::odt::get_metadata(std::fs::File::open(&path).unwrap()).unwrap().title, "Corrected Title");
    }

    /// ODF requires `mimetype` first and uncompressed, exactly as EPUB
    /// does -- and the manifest is expected to list every part.
    #[test]
    fn the_package_is_well_formed() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let path = convert(&out, &a_book(src.path()));

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
        drop(first);

        for required in ["mimetype", "META-INF/manifest.xml", "content.xml", "meta.xml"] {
            assert!(names.iter().any(|n| n == required), "{required} missing: {names:?}");
        }

        use std::io::Read;
        let mut manifest = String::new();
        zip.by_name("META-INF/manifest.xml").unwrap().read_to_string(&mut manifest).unwrap();
        for declared in ["content.xml", "meta.xml"] {
            assert!(manifest.contains(declared), "{declared} is not declared in the manifest:\n{manifest}");
        }
    }

    /// A book with no real metadata must not have placeholders written
    /// into it -- the same rule the metadata writers follow.
    #[test]
    fn placeholder_metadata_is_not_written() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("c1.html"), "<html><body><p>Text.</p></body></html>").unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(src.path())));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.spine.add("c1", true);

        let path = convert(&out, &book);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        use std::io::Read;
        let mut meta = String::new();
        zip.by_name("meta.xml").unwrap().read_to_string(&mut meta).unwrap();

        assert!(!meta.contains("Unknown"), "a placeholder title was written:\n{meta}");
        assert!(!meta.contains("<dc:language>und"), "a placeholder language was written:\n{meta}");
    }

    #[test]
    fn the_content_is_still_converted() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let path = convert(&out, &a_book(src.path()));

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        use std::io::Read;
        let mut content = String::new();
        zip.by_name("content.xml").unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("quick brown fox"), "the book's text is missing:\n{content}");
    }
}
