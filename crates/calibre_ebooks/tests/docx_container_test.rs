//! End-to-end read of a DOCX package through the public API: content
//! types, relationships, the main document part, and book metadata.

use calibre_ebooks::docx::container::Docx;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

const CONTENT_TYPES: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#;

const RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#;

const DOCUMENT: &[u8] =
    b"<w:document><w:body><w:p><w:r><w:t>Hello World</w:t></w:r></w:p></w:body></w:document>";

const DOC_RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

const CORE_PROPS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>Test Document</dc:title>
<dc:creator>Test Author</dc:creator>
<dc:language>en</dc:language>
<cp:lastModifiedBy>Author</cp:lastModifiedBy>
<cp:revision>1</cp:revision>
</cp:coreProperties>"#;

fn write_sample(path: &std::path::Path) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", RELS),
        ("word/document.xml", DOCUMENT),
        ("word/_rels/document.xml.rels", DOC_RELS),
        ("docProps/core.xml", CORE_PROPS),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn reads_parts_content_types_and_metadata() {
    let tmp_dir = tempdir().unwrap();
    let docx_path = tmp_dir.path().join("test.docx");
    write_sample(&docx_path);

    let mut docx = Docx::open(&docx_path).expect("opens");

    assert_eq!(docx.document_name().unwrap(), "word/document.xml");
    assert!(docx.is_transitional());

    assert_eq!(
        docx.default_content_types.get("rels").map(String::as_str),
        Some("application/vnd.openxmlformats-package.relationships+xml")
    );
    assert_eq!(
        docx.content_types
            .get("word/document.xml")
            .map(String::as_str),
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
    );

    let body = docx.read_str("word/document.xml").unwrap();
    assert!(body.contains("Hello World"));

    let rels = docx.document_relationships().unwrap();
    assert_eq!(rels.target("rId7"), Some("word/media/image1.png"));

    let mi = docx.metadata();
    assert_eq!(mi.title, "Test Document");
    assert_eq!(mi.authors, vec!["Test Author"]);
    // "en" canonicalizes to "eng" -- see calibre_utils::localization
    // (issue #140).
    assert_eq!(mi.languages, vec!["eng"]);
}

#[test]
fn a_package_missing_its_structural_parts_is_rejected() {
    let tmp_dir = tempdir().unwrap();
    let path = tmp_dir.path().join("broken.docx");
    let file = File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(DOCUMENT).unwrap();
    zip.finish().unwrap();

    assert!(Docx::open(&path).is_err(), "no [Content_Types].xml");
}
