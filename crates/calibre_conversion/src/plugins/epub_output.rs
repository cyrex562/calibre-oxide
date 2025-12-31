use crate::oeb::{ManifestItem, OebBook};
use crate::traits::{ConversionOptions, OutputPlugin};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::FileOptions;

pub struct EpubOutput;

impl OutputPlugin for EpubOutput {
    fn write(&self, book: &OebBook, path: &Path, _options: &ConversionOptions) -> Result<()> {
        let file = File::create(path).context("Failed to create output EPUB file")?;
        let mut zip = zip::ZipWriter::new(file);

        // 1. Write mimetype (must be first, uncompressed)
        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        zip.start_file("mimetype", options)?;
        zip.write_all(b"application/epub+zip")?;

        // 2. Write META-INF/container.xml
        zip.start_file("META-INF/container.xml", FileOptions::default())?;
        zip.write_all(CONTAINER_XML.as_bytes())?;

        // 3. Write content.opf
        // We need to generate the OPF XML from the OebBook metadata/manifest/spine
        let opf_content = generate_opf(book);
        zip.start_file("content.opf", FileOptions::default())?;
        zip.write_all(opf_content.as_bytes())?;

        // 4. Write Resources from Manifest
        for item in book.manifest.values() {
            // Determine path in zip (href)
            let zip_path = &item.href;

            // Read source file
            let mut source_file = File::open(&item.path)
                .context(format!("Failed to open resource file {:?}", item.path))?;
            let mut buffer = Vec::new();
            source_file.read_to_end(&mut buffer)?;

            // Write to zip
            zip.start_file(zip_path, FileOptions::default())?;
            zip.write_all(&buffer)?;
        }

        // TODO: Generate NCX/TOC if missing

        zip.finish()?;
        Ok(())
    }
}

const CONTAINER_XML: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
   <rootfiles>
      <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>
"#;

fn generate_opf(book: &OebBook) -> String {
    // Simple OPF generator
    // Valid minimal OPF requires: metadata, manifest, spine

    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="uuid_id" version="2.0">
"#,
    );

    // Metadata
    xml.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");
    xml.push_str(&format!(
        "    <dc:title>{}</dc:title>\n",
        escape_xml(&book.metadata.title)
    ));
    for author in &book.metadata.authors {
        xml.push_str(&format!(
            "    <dc:creator opf:role=\"aut\">{}</dc:creator>\n",
            escape_xml(author)
        ));
    }
    // Add UUID if present, else default?
    xml.push_str("    <dc:language>en</dc:language>\n"); // Default for now
    xml.push_str("  </metadata>\n");

    // Manifest
    xml.push_str("  <manifest>\n");
    for item in book.manifest.values() {
        xml.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"{}\"/>\n",
            escape_xml(&item.id),
            escape_xml(&item.href),
            escape_xml(&item.media_type)
        ));
    }
    // Ensure NCX is in manifest?
    xml.push_str("  </manifest>\n");

    // Spine
    xml.push_str("  <spine toc=\"ncx\">\n");
    for item_ref in &book.spine {
        let linear = if item_ref.linear { "yes" } else { "no" };
        xml.push_str(&format!(
            "    <itemref idref=\"{}\" linear=\"{}\"/>\n",
            escape_xml(&item_ref.idref),
            linear
        ));
    }
    xml.push_str("  </spine>\n");

    xml.push_str("</package>");
    xml
}

fn escape_xml(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}
