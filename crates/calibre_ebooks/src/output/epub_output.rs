use crate::oeb::book::OEBBook;
use crate::oeb::writer::OEBWriter;
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::tempdir;
use walkdir::WalkDir;
use zip::write::{FileOptions, ZipWriter};

pub struct EPUBOutput;

impl EPUBOutput {
    pub fn new() -> Self {
        EPUBOutput
    }

    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        let temp_dir = tempdir().context("Failed to create temporary directory")?;
        let temp_path = temp_dir.path();

        // 1. Write OEB content to temp dir
        let writer = OEBWriter::new();
        writer
            .write_book(book, temp_path)
            .context("Failed to write OEB content")?;

        // 2. Create META-INF/container.xml if not present
        let meta_inf = temp_path.join("META-INF");
        if !meta_inf.exists() {
            fs::create_dir(&meta_inf)?;
        }
        let container_xml = meta_inf.join("container.xml");
        if !container_xml.exists() {
            let xml = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
   <rootfiles>
      <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#;
            fs::write(&container_xml, xml)?;
        }

        // 3. Create ZIP at output_path
        let file = fs::File::create(output_path).context("Failed to create output file")?;
        let mut zip = ZipWriter::new(file);

        // 4. Write mimetype (STORED, must be first)
        let options_stored = FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);

        zip.start_file("mimetype", options_stored)?;
        zip.write_all(b"application/epub+zip")?;

        // 5. Write content (DEFLATED)
        let options_deflated = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        for entry in WalkDir::new(temp_path) {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }

            let name = path
                .strip_prefix(temp_path)?
                .to_str()
                .context("Non-UTF8 path")?;

            // Skip mimetype as we already wrote it
            if name == "mimetype" {
                continue;
            }

            // Improve paths for ZIP (forward slashes)
            #[cfg(windows)]
            let name = name.replace('\\', "/");

            zip.start_file(name, options_deflated)?;
            let mut f = fs::File::open(path)?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        }

        zip.finish()?;
        Ok(())
    }
}
