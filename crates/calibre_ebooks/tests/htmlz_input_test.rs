use calibre_ebooks::input::htmlz_input::HTMLZInput;
use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_htmlz_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let htmlz_path = tmp_dir.path().join("test.htmlz");
    let file = File::create(&htmlz_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Add index.html
    zip.start_file("index.html", options).unwrap();
    zip.write_all(
        b"<html><head><title>HTMLZ Test</title></head><body><p>Content</p></body></html>",
    )
    .unwrap();

    // Add Metadata OPF (Optional, testing without it logic, or with it?)
    // Let's test WITH OPF first as that is standard HTMLZ.
    zip.start_file("metadata.opf", options).unwrap();
    zip.write_all(
        r#"<?xml version='1.0' encoding='utf-8'?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>HTMLZ Test Title</dc:title>
  </metadata>
  <manifest>
    <item id="index" href="index.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="index"/>
  </spine>
</package>"#
            .as_bytes(),
    )
    .unwrap();

    zip.finish().unwrap();

    // Test Conversion
    let output_dir = tmp_dir.path().join("output");
    let plugin = HTMLZInput::new();
    let book = plugin
        .convert(&htmlz_path, &output_dir)
        .expect("Conversion failed");

    // Verify
    let title = book
        .metadata
        .items
        .iter()
        .find(|i| i.term == "title")
        .unwrap();
    assert_eq!(title.value, "HTMLZ Test Title");
    assert!(book.manifest.items.values().any(|i| i.href == "index.html"));
}
