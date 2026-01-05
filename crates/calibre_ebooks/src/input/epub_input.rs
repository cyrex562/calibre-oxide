use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::reader::OEBReader;
use anyhow::{anyhow, Context, Result};
use roxmltree::Document;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub struct EPUBInput;

impl EPUBInput {
    pub fn new() -> Self {
        EPUBInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        // 1. Unzip to output_dir
        println!("Unzipping EPUB to {:?}", output_dir);
        let file = File::open(input_path).context("Failed to open EPUB file")?;
        let mut archive = ZipArchive::new(file).context("Failed to open ZIP archive")?;
        archive
            .extract(output_dir)
            .context("Failed to extract EPUB")?;

        // 2. Find OPF Path from META-INF/container.xml
        let container_xml_path = output_dir.join("META-INF").join("container.xml");
        let opf_path = if container_xml_path.exists() {
            Self::find_opf_path(&container_xml_path)?
        } else {
            // Fallback: search for *.opf
            // Simple recursive search? Or just checking root/OEBPS?
            // Let's stick to container.xml + simple fallback
            PathBuf::from("content.opf") // Default assumption
        };

        println!("OPF Path: {:?}", opf_path);

        // 3. Initialize OEBBook
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // 4. Read OPF
        let reader = OEBReader::new();
        let opf_path_str = opf_path
            .to_str()
            .ok_or_else(|| anyhow!("Invalid OPF path"))?;
        // OEBReader expects path relative to container root (output_dir)
        // If find_opf_path returns relative path, we are good.
        reader.read_opf(&mut book, opf_path_str)?;

        Ok(book)
    }

    fn find_opf_path(container_xml_path: &Path) -> Result<PathBuf> {
        let mut content = String::new();
        File::open(container_xml_path)?.read_to_string(&mut content)?;

        let doc = Document::parse(&content)?;
        for node in doc.descendants() {
            if node.tag_name().name() == "rootfile" {
                if let Some(media_type) = node.attribute("media-type") {
                    if media_type == "application/oebps-package+xml" {
                        if let Some(full_path) = node.attribute("full-path") {
                            return Ok(PathBuf::from(full_path));
                        }
                    }
                }
            }
        }

        // Fallback or error?
        // Let's try to return the first rootfile found if media-type match failed (sometimes case sensitivity or optional)
        for node in doc.descendants() {
            if node.tag_name().name() == "rootfile" {
                if let Some(full_path) = node.attribute("full-path") {
                    return Ok(PathBuf::from(full_path));
                }
            }
        }

        Err(anyhow!("Could not find OPF path in container.xml"))
    }
}
