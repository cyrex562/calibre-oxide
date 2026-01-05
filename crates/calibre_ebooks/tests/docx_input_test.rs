use calibre_ebooks::input::docx_input::DOCXInput;
use calibre_ebooks::oeb::book::OEBBook;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_docx_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let docx_path = tmp_dir.path().join("test.docx");
    let file = File::create(&docx_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // 1. [Content_Types].xml
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#).unwrap();

    // 2. _rels/.rels
    zip.add_directory("_rels", options).unwrap();
    zip.start_file("_rels/.rels", options).unwrap();
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#).unwrap();

    // 3. word/document.xml
    zip.add_directory("word", options).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
  <w:p>
    <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
    <w:r><w:t>Chapter 1</w:t></w:r>
  </w:p>
  <w:p>
    <w:r><w:t>Hello World</w:t></w:r>
  </w:p>
</w:body>
</w:document>"#,
    )
    .unwrap();

    zip.finish().unwrap(); // Drop file

    // Test Conversion
    let output_dir = tmp_dir.path().join("output");
    std::fs::create_dir(&output_dir).unwrap();

    let plugin = DOCXInput::new();
    let book = plugin
        .convert(&docx_path, &output_dir)
        .expect("Conversion failed");

    // Check Manifest
    let html_item = book
        .manifest
        .iter()
        .find(|i| i.href == "index.html")
        .expect("index.html missing");
    let content = std::fs::read_to_string(&html_item.path).expect("Failed to read output html");

    assert!(content.contains("<h1>Chapter 1</h1>"));
    assert!(content.contains("<p>Hello World</p>"));
}
