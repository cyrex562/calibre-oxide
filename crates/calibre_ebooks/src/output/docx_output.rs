use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use zip::write::FileOptions;
use zip::ZipWriter;

pub struct DOCXOutput;

impl DOCXOutput {
    pub fn new() -> Self {
        DOCXOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let file = File::create(output_path).context("Failed to create DOCX file")?;
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // 1. [Content_Types].xml
        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#)?;

        // 2. _rels/.rels
        zip.start_file("_rels/.rels", options)?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#)?;

        // 3. word/document.xml
        // We will generate a basic document with the book title and simple text logic.
        // Full conversion is out of scope.
        let title = book
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.as_str())
            .unwrap_or("Untitled Book");

        zip.start_file("word/document.xml", options)?;
        write!(
            zip,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Title"/></w:pPr>
      <w:r><w:t>{}</w:t></w:r>
    </w:p>
    <w:p><w:r><w:t>Converted content pending full implementation.</w:t></w:r></w:p>
  </w:body>
</w:document>"#,
            xml_escape(title)
        )?;

        zip.finish()?;
        Ok(())
    }
}

fn xml_escape(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}
