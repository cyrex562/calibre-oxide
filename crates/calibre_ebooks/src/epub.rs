//! EPUB support.
//!
//! `read_epub_metadata` here is the EPUB arm of the metadata readers.
//! The submodules are the port of `old_src/src/calibre/ebooks/epub/`:
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` | this module |
//! | `cfi/` | [`cfi`] |
//! | `pages.py` | [`pages`] |
//! | `periodical.py` | [`periodical`] |
//!
//! `__init__.py`'s `rules()` is not ported: it walks cssutils
//! stylesheet objects, and this crate has no CSS object model to walk.

pub mod cfi;
pub mod pages;
pub mod periodical;

use std::io::{Seek, Write};

use zip::write::FileOptions;
use zip::ZipWriter;

use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

pub fn read_epub_metadata(path: &Path) -> Result<MetaInformation> {
    let file = File::open(path).context("Failed to open file")?;
    let mut archive = ZipArchive::new(file).context("Failed to read zip")?;

    // 1. Read META-INF/container.xml to find the OPF path
    let container_xml = {
        let mut f = archive
            .by_name("META-INF/container.xml")
            .context("META-INF/container.xml not found")?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    let opf_path = extract_opf_path_from_container(&container_xml)
        .context("Could not find OPF path in container.xml")?;

    // 2. Read the OPF file
    let opf_content = {
        let mut f = archive
            .by_name(&opf_path)
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

/// An extra root file to advertise in `META-INF/container.xml`.
///
/// Port of the `extra_entries` triples the Python `initialize_container`
/// takes: `(path, mimetype, data)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFile {
    /// Path inside the package.
    pub path: String,
    /// Media type declared for it.
    pub media_type: String,
    /// The bytes to store at `path`.
    pub data: Vec<u8>,
}

/// Build `META-INF/container.xml` pointing at `opf_path`.
///
/// Port of the Python `simple_container_xml`. `extra_entries` is
/// pre-rendered `<rootfile>` markup, as in the original.
pub fn simple_container_xml(opf_path: &str, extra_entries: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
   <rootfiles>
      <rootfile full-path="{opf_path}" media-type="application/oebps-package+xml"/>
      {extra_entries}
   </rootfiles>
</container>
    "#
    )
}

/// Start an EPUB package: the uncompressed `mimetype` entry first, as
/// the specification requires, then `META-INF/container.xml` and any
/// extra root files.
///
/// Port of the Python `initialize_container`. The Python returns the
/// open `ZipFile` for the caller to keep adding to; so does this.
pub fn initialize_container<W: Write + Seek>(
    sink: W,
    opf_name: &str,
    extra_entries: &[RootFile],
) -> Result<ZipWriter<W>> {
    let mut rootfiles = String::new();
    for entry in extra_entries {
        rootfiles.push_str(&format!(
            r#"<rootfile full-path="{}" media-type="{}"/>"#,
            entry.path, entry.media_type
        ));
    }
    let container = simple_container_xml(opf_name, &rootfiles);

    let mut zip = ZipWriter::new(sink);
    // The mimetype entry must be first and stored uncompressed, so a
    // reader can identify the file from its first bytes.
    let stored = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;

    let deflated = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory("META-INF", deflated.unix_permissions(0o755))?;
    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(container.as_bytes())?;

    for entry in extra_entries {
        zip.start_file(&entry.path, deflated)?;
        zip.write_all(&entry.data)?;
    }
    Ok(zip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn the_container_points_at_the_opf() {
        let xml = simple_container_xml("OEBPS/content.opf", "");
        let doc = roxmltree::Document::parse(&xml).expect("well formed");
        let rootfile = doc
            .descendants()
            .find(|n| n.tag_name().name() == "rootfile")
            .expect("a rootfile");
        assert_eq!(rootfile.attribute("full-path"), Some("OEBPS/content.opf"));
        assert_eq!(
            rootfile.attribute("media-type"),
            Some("application/oebps-package+xml")
        );
    }

    #[test]
    fn a_started_package_is_a_readable_epub() {
        let extra = vec![RootFile {
            path: "META-INF/encryption.xml".to_string(),
            media_type: "application/xml".to_string(),
            data: b"<encryption/>".to_vec(),
        }];
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = initialize_container(&mut buf, "metadata.opf", &extra).expect("starts");
            zip.finish().expect("finishes");
        }
        buf.set_position(0);

        let mut archive = zip::ZipArchive::new(buf).expect("a valid zip");
        // The mimetype must come first and be stored, not deflated.
        assert_eq!(archive.by_index(0).unwrap().name(), "mimetype");
        assert_eq!(
            archive.by_index(0).unwrap().compression(),
            zip::CompressionMethod::Stored
        );

        let mut mimetype = String::new();
        archive
            .by_name("mimetype")
            .unwrap()
            .read_to_string(&mut mimetype)
            .unwrap();
        assert_eq!(mimetype, "application/epub+zip");

        let mut container = String::new();
        archive
            .by_name("META-INF/container.xml")
            .unwrap()
            .read_to_string(&mut container)
            .unwrap();
        let doc = roxmltree::Document::parse(&container).expect("well formed");
        let paths: Vec<&str> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "rootfile")
            .filter_map(|n| n.attribute("full-path"))
            .collect();
        assert_eq!(paths, vec!["metadata.opf", "META-INF/encryption.xml"]);

        // And the extra entry's data really is in the package.
        let mut enc = String::new();
        archive
            .by_name("META-INF/encryption.xml")
            .unwrap()
            .read_to_string(&mut enc)
            .unwrap();
        assert_eq!(enc, "<encryption/>");
    }

    #[test]
    fn the_opf_path_is_read_back_by_the_metadata_reader() {
        // extract_opf_path_from_container is what reads this file, so
        // the writer and the reader in this module must agree.
        let xml = simple_container_xml("OEBPS/package.opf", "");
        assert_eq!(
            extract_opf_path_from_container(&xml).as_deref(),
            Some("OEBPS/package.opf")
        );
    }
}
