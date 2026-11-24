//! Port of `old_src/src/calibre/ebooks/oeb/polish/import_book.py`.
//!
//! Both functions here are real: [`crate::conversion::plumber::convert_to_oebbook`]
//! (issue #38's `Plumber` refactor) supplies the format-dispatch input
//! side, [`crate::oeb::writer::OEBWriter`] serializes the resulting
//! in-memory [`crate::oeb::book::OEBBook`] to an on-disk OEB tree (the
//! Rust equivalent of Python's conditional `write_oebbook` call --
//! unconditional here since `convert_to_oebbook` always returns an
//! in-memory book, never a pre-written path), and
//! [`crate::epub::initialize_container`] provides the EPUB zip
//! scaffolding (mimetype + `META-INF/container.xml`) Python's
//! `calibre.ebooks.epub.initialize_container` provides.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use tempfile::tempdir;

use crate::conversion::plumber::convert_to_oebbook;
use crate::epub::initialize_container;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::writer::OEBWriter;

use super::container::Container;

/// Port of `auto_fill_manifest`: adds a manifest `<item>` for every file
/// present on disk that isn't already manifested (and isn't allowed to
/// go unmanifested), matching Python's sanity check that the freshly
/// generated href round-trips back to the same name.
pub fn auto_fill_manifest(container: &mut Container) -> Result<()> {
    let manifested: std::collections::HashSet<String> =
        container.manifest_id_map()?.into_values().collect();
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, _mt) in names {
        if manifested.contains(&name) || container.ok_to_be_unmanifested(&name) {
            continue;
        }
        let item = container.generate_item(&name, "", None, false)?;
        let opf_name = container.opf_name.clone();
        let href = container
            .get_xml(&opf_name)?
            .get_attr(item, "href")
            .unwrap_or("")
            .to_string();
        let gname = container.href_to_name(&href, Some(&opf_name));
        if gname.as_deref() != Some(name.as_str()) {
            bail!(
                "This should never happen (gname={:?}, name={:?}, href={:?})",
                gname,
                name,
                href
            );
        }
    }
    Ok(())
}

/// Port of `import_book_as_epub`.
pub fn import_book_as_epub(srcpath: &Path, destpath: &Path) -> Result<()> {
    let dest_str = destpath.to_string_lossy();
    if !dest_str.to_lowercase().ends_with(".epub") {
        bail!(
            "Can only import books into the EPUB format, not {}",
            destpath
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dest_str.into_owned())
        );
    }

    let tdir = tempdir().context("Failed to create a temporary directory")?;
    let extract_dir = tdir.path().join("source");
    let mut book = convert_to_oebbook(srcpath, &extract_dir)
        .with_context(|| format!("Failed to convert {}", srcpath.display()))?;

    let oeb_dir = tdir.path().join("oeb");
    fs::create_dir_all(&oeb_dir)?;
    OEBWriter::new()
        .write_book(&mut book, &oeb_dir)
        .context("Failed to write intermediate OEB tree")?;

    let opf_path = oeb_dir.join("content.opf");
    let mut container = Container::open(&oeb_dir, &opf_path)?;

    auto_fill_manifest(&mut container)?;

    // Auto-fix all HTML/CSS: parsing (and re-serializing on commit)
    // repairs recoverable markup errors, matching Python's
    // `c.parsed(name); c.dirty(name)` loop.
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if OEB_DOCS.contains(&mt.as_str()) || OEB_STYLES.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            container.dirty(&name);
        }
    }
    container.commit(false)?;

    let file = fs::File::create(destpath)
        .with_context(|| format!("Failed to create {}", destpath.display()))?;
    let opf_name = container.opf_name.clone();
    let mut zip = initialize_container(file, &opf_name, &[])?;
    let names: Vec<String> = container.name_path_map.keys().cloned().collect();
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for name in names {
        let data = container.raw_data(&name, false)?;
        zip.start_file(&name, options)?;
        zip.write_all(&data)?;
    }
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn import_book_as_epub_rejects_non_epub_destination() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.txt");
        fs::write(&src, b"hello").unwrap();
        let dest = dir.path().join("out.pdf");
        let err = import_book_as_epub(&src, &dest).unwrap_err();
        assert!(err.to_string().contains("EPUB"));
    }

    #[test]
    fn import_book_as_epub_converts_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.txt");
        fs::write(&src, b"Hello, world!\n\nSecond paragraph.").unwrap();
        let dest = dir.path().join("out.epub");
        import_book_as_epub(&src, &dest).unwrap();
        assert!(dest.exists());

        let file = fs::File::open(&dest).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut mimetype = String::new();
        archive
            .by_name("mimetype")
            .unwrap()
            .read_to_string(&mut mimetype)
            .unwrap();
        assert_eq!(mimetype, "application/epub+zip");
        assert!(archive.by_name("META-INF/container.xml").is_ok());
        assert!(archive.by_name("content.opf").is_ok());
    }
}
