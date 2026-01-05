use calibre_ebooks::input::epub_input::EPUBInput;
use calibre_ebooks::oeb::book::OEBBook;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_epub_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let epub_path = tmp_dir.path().join("test.epub");
    let file = File::create(&epub_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // 1. mimetype (must be uncompressed and first)
    zip.start_file("mimetype", options).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    // 2. META-INF/container.xml
    zip.add_directory("META-INF", options).unwrap();
    zip.start_file("META-INF/container.xml", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
   <rootfiles>
      <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#,
    )
    .unwrap();

    // 3. content.opf
    zip.start_file("content.opf", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="dcid" version="2.0">
   <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
      <dc:title>EPUB Test</dc:title>
      <dc:language>en</dc:language>
   </metadata>
   <manifest>
      <item id="html" href="index.html" media-type="application/xhtml+xml"/>
   </manifest>
   <spine>
      <itemref idref="html"/>
   </spine>
</package>"#,
    )
    .unwrap();

    // 4. index.html
    zip.start_file("index.html", options).unwrap();
    zip.write_all(b"<html><body><h1>Hello EPUB</h1></body></html>")
        .unwrap();

    zip.finish().unwrap();

    // Test Conversion
    let output_dir = tmp_dir.path().join("output");
    let plugin = EPUBInput::new();
    let book = plugin
        .convert(&epub_path, &output_dir)
        .expect("Conversion failed");

    // Verify Metadata
    let title = book
        .metadata
        .items
        .iter()
        .find(|i| i.term == "title")
        .unwrap();
    assert_eq!(title.value, "EPUB Test");

    // Verify Manifest
    assert!(book.manifest.items.contains_key("html"));
}
