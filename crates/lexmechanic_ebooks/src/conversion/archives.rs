use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::Path;
use zip::ZipArchive;

pub enum ArchiveType {
    Zip,
    Rar,
    SevenZip,
    Unknown,
}

pub struct ArchiveHandler;

impl ArchiveHandler {
    pub fn new() -> Self {
        ArchiveHandler
    }

    pub fn detect_type(path: &Path) -> ArchiveType {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "zip" | "epub" | "docx" | "odt" => ArchiveType::Zip,
            "rar" | "cbr" => ArchiveType::Rar,
            "7z" | "cb7" => ArchiveType::SevenZip,
            _ => ArchiveType::Unknown,
        }
    }

    pub fn extract(&self, archive_path: &Path, output_dir: &Path) -> Result<()> {
        let archive_type = Self::detect_type(archive_path);

        match archive_type {
            ArchiveType::Zip => self.extract_zip(archive_path, output_dir),
            ArchiveType::Rar => Err(anyhow::anyhow!(
                "RAR extraction not supported yet (requires unrar)"
            )),
            ArchiveType::SevenZip => Err(anyhow::anyhow!("7z extraction not supported yet")),
            ArchiveType::Unknown => Err(anyhow::anyhow!("Unknown archive format")),
        }
    }

    fn extract_zip(&self, archive_path: &Path, output_dir: &Path) -> Result<()> {
        let file = File::open(archive_path).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = match file.enclosed_name() {
                Some(path) => output_dir.join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
        Ok(())
    }
}
