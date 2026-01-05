use crate::oeb::book::OEBBook;
use crate::oeb::writer::OEBWriter;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use tempfile::tempdir;
use walkdir::WalkDir;
use zip::write::{FileOptions, ZipWriter};

pub struct HTMLZOutput;

impl HTMLZOutput {
    pub fn new() -> Self {
        HTMLZOutput
    }

    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        let temp_dir = tempdir().context("Failed to create temporary directory")?;
        let temp_path = temp_dir.path();

        // 1. Write Expanded Book
        let writer = OEBWriter::new();
        writer.write_book(book, temp_path)?;

        // 2. Zip it
        let file = File::create(output_path).context("Failed to create output HTMLZ file")?;
        let mut zip = ZipWriter::new(file);

        let options = FileOptions::default()
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

            #[cfg(windows)]
            let name = name.replace('\\', "/");

            zip.start_file(name, options)?;
            let mut f = File::open(path)?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        }

        zip.finish()?;

        Ok(())
    }
}
