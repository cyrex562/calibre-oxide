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

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
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
</manifest:manifest>"#)?;

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
