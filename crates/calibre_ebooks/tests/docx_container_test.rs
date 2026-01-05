use calibre_ebooks::docx::container::DOCX;
use calibre_ebooks::docx::names::DOCXNamespaces;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_docx_container() {
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
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#).unwrap();

    // 2. _rels/.rels
    zip.add_directory("_rels", options).unwrap();
    zip.start_file("_rels/.rels", options).unwrap();
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#).unwrap();

    // 3. word/document.xml
    zip.add_directory("word", options).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(
        b"<w:document><w:body><w:p><w:r><w:t>Hello World</w:t></w:r></w:p></w:body></w:document>",
    )
    .unwrap();

    // 4. docProps/core.xml
    zip.add_directory("docProps", options).unwrap();
    zip.start_file("docProps/core.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>Test Document</dc:title>
<dc:creator>Test Author</dc:creator>
<cp:lastModifiedBy>Author</cp:lastModifiedBy>
<cp:revision>1</cp:revision>
</cp:coreProperties>"#).unwrap();

    zip.finish().unwrap(); // Returns File, dropped immediately
                           // Ensure file is closed/flushed

    // Test Reading
    let f = File::open(&docx_path).unwrap();
    let mut docx = DOCX::new(f).expect("Failed to open DOCX");

    assert_eq!(docx.document_name().unwrap(), "word/document.xml");

    // Check Content Types
    assert_eq!(
        docx.default_content_types.get("rels").unwrap(),
        "application/vnd.openxmlformats-package.relationships+xml"
    );
    assert_eq!(
        docx.content_types.get("word/document.xml").unwrap(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
    );

    // Check Metadata
    let meta = docx.get_metadata().expect("Failed to get metadata");
    let titles = meta.get("title");
    let authors = meta.get("creator");

    assert!(titles.iter().any(|i| i.value == "Test Document"));
    assert!(authors.iter().any(|i| i.value == "Test Author"));
}
