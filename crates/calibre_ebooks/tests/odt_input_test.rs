use calibre_ebooks::input::odt_input::ODTInput;
use std::fs;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;
use zip::ZipWriter;

#[test]
fn test_odt_input_conversion() {
    let temp_dir = tempdir().unwrap();
    let input_path = temp_dir.path().join("test.odt");
    let output_dir = temp_dir.path().join("output");

    // Create a mock ODT file
    let file = fs::File::create(&input_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o755);

    zip.start_file("mimetype", options).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.text")
        .unwrap();

    zip.start_file("content.xml", options).unwrap();
    let content_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2">
 <office:body>
  <office:text>
   <text:p>Hello, ODT!</text:p>
  </office:text>
 </office:body>
</office:document-content>"#;
    zip.write_all(content_xml.as_bytes()).unwrap();
    zip.finish().unwrap();

    // Run conversion
    let input = ODTInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    // Verify
    assert!(output_dir.join("index.html").exists());
    let index_html = fs::read_to_string(output_dir.join("index.html")).unwrap();
    assert!(index_html.contains("Hello, ODT!"));

    // Check Manifest
    assert!(book.manifest.hrefs.contains_key("index.html"));
}
