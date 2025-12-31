use crate::oeb::{ManifestItem, OebBook, SpineItem};
use crate::traits::{ConversionOptions, InputPlugin};
use anyhow::{Context, Result};
use calibre_ebooks::opf::parse_opf;
use log;
use roxmltree;
use std::path::Path;
use tempfile;
use zip;

pub struct EpubInput;

impl InputPlugin for EpubInput {
    fn read(&self, path: &Path, _options: &ConversionOptions) -> Result<OebBook> {
        // 1. Unzip to a temporary directory
        let temp_dir = tempfile::Builder::new()
            .prefix("calibre_conversion_")
            .tempdir()
            .context("Failed to create temporary directory")?;

        let file = std::fs::File::open(path).context("Failed to open EPUB file")?;
        let mut archive = zip::ZipArchive::new(file).context("Failed to open ZIP archive")?;

        archive
            .extract(temp_dir.path())
            .context("Failed to extract EPUB")?;

        // 2. Find OPF path
        let container_path = temp_dir.path().join("META-INF/container.xml");
        let container_xml = std::fs::read_to_string(&container_path)
            .context("Failed to read META-INF/container.xml")?;

        let opf_rel_path = extract_opf_path_from_container(&container_xml)
            .context("Could not find OPF path in container.xml")?;
        let opf_path = temp_dir.path().join(&opf_rel_path);

        // 3. Read OPF content
        let opf_content = std::fs::read_to_string(&opf_path)
            .context(format!("Failed to read OPF file at {:?}", opf_path))?;

        // 4. Parse Metadata
        let meta = parse_opf(&opf_content).context("Failed to parse OPF content")?;

        let mut book = OebBook::new();
        book.metadata = meta;
        book.version = get_opf_version(&opf_content).unwrap_or_else(|| "2.0".to_string());

        log::info!("Extracted EPUB to {:?}", temp_dir.path());

        // 5. Parse Manifest and Spine
        let doc = roxmltree::Document::parse(&opf_content).context("Failed to parse OPF XML")?;
        let root = doc.root_element();
        let opf_dir = opf_path.parent().unwrap_or(temp_dir.path());

        // Manifest
        if let Some(manifest) = root
            .children()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("manifest"))
        {
            for node in manifest.children() {
                if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("item") {
                    let id = node.attribute("id").unwrap_or_default().to_string();
                    let href = node.attribute("href").unwrap_or_default().to_string();
                    let media_type = node.attribute("media-type").unwrap_or_default().to_string();

                    if !id.is_empty() && !href.is_empty() {
                        // href is relative to the OPF file location
                        let item_path = opf_dir.join(&href);

                        book.manifest.insert(
                            id.clone(),
                            ManifestItem {
                                id,
                                href,
                                media_type,
                                path: item_path,
                            },
                        );
                    }
                }
            }
        }

        // Spine
        if let Some(spine) = root
            .children()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("spine"))
        {
            for node in spine.children() {
                if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("itemref") {
                    let idref = node.attribute("idref").unwrap_or_default().to_string();
                    let linear = node.attribute("linear").map(|v| v != "no").unwrap_or(true);

                    if !idref.is_empty() {
                        book.spine.push(SpineItem { idref, linear });
                    }
                }
            }
        }

        // We leak the temp dir intentionally for now so files persist for the pipeline
        // In a real impl, we'd transfer ownership or copy files.
        // For now, let's just detach it.
        let _ = temp_dir.into_path();

        Ok(book)
    }
}

fn get_opf_version(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    root.attribute("version").map(|s| s.to_string())
}

fn extract_opf_path_from_container(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    root.descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("rootfile"))
        .and_then(|n| n.attribute("full-path").map(|s| s.to_string()))
}
