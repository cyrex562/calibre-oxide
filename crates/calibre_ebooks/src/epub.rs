use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;
use anyhow::{Context, Result};

pub fn read_epub_metadata(path: &Path) -> Result<MetaInformation> {
    let file = File::open(path).context("Failed to open file")?;
    let mut archive = ZipArchive::new(file).context("Failed to read zip")?;

    // 1. Read META-INF/container.xml to find the OPF path
    let container_xml = {
        let mut f = archive.by_name("META-INF/container.xml")
            .context("META-INF/container.xml not found")?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    let opf_path = extract_opf_path_from_container(&container_xml)
        .context("Could not find OPF path in container.xml")?;

    // 2. Read the OPF file
    let opf_content = {
        let mut f = archive.by_name(&opf_path)
            .context(format!("OPF file {} not found in archive", opf_path))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    // 3. Parse Metadata
    let meta = parse_opf(&opf_content)?;
    Ok(meta)
}

fn extract_opf_path_from_container(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    // Path: rootfile full-path attribute
    // <rootfiles><rootfile full-path="foo.opf" .../>
    
    root.descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("rootfile"))
        .and_then(|n| n.attribute("full-path").map(|s| s.to_string()))
}
