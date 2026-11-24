//! Port of `old_src/src/calibre/ebooks/oeb/polish/create.py`.
//!
//! Per issue #166: the `epub`/`txt`/`md`/`docx` output paths of
//! [`create_book`] are ported for real. `fmt == "azw3"` is given a real
//! signature with a `todo!()` body -- it needs `opf_to_azw3`
//! (`Container.opf_to_azw3` in Python), which `container.rs` (issue
//! #161) already left as a documented gap on `Azw3Container` (blocked on
//! wiring `Plumber` + `mobi::writer2`/`writer8` together, the same shape
//! of gap issue #157 tracks for joint MOBI6+KF8 output). Not attempted
//! here either.
//!
//! # Design notes
//!
//! **`docx` is real, not stubbed**, even though it superficially looks
//! like it needs the same `Plumber` wiring the `azw3` path does. Python's
//! call is `DOCX(p.opts, log).write(path, mi, create_empty_document=True)`
//! -- and this crate's [`DocxWriter::new`] already starts from an empty
//! `<w:body>` skeleton with no OEB content required (see its own docs),
//! so `DocxWriter::new(opts).write(sink, mi)` *is* the
//! `create_empty_document=True` case already, without any `Plumber`
//! involvement.
//!
//! **`lang_as_iso639_1` is skipped.** Python canonicalizes the OPF's
//! `<dc:language>` text to an ISO 639-1 two-letter code via a language
//! database this crate has no port of. `MetaInformation::languages`
//! already stores calibre's own canonical (already-short) language
//! codes in practice, so this port uses the OPF's `<dc:language>` text
//! as-is -- the same simplification `docx/writer/container.rs`'s
//! `core_properties` already documents and accepts for the same reason.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::docx::writer::container::{DocxWriter, PageOptions};
use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;
use crate::oeb::constants::OPF2_NS;
use crate::zipfile_safe_replace::build_zip_atomic;

use super::container::opf_namespaces;
use super::parsing;
use super::pretty::{pretty_html_tree, pretty_xml_tree};
use super::toc::{create_ncx, Toc};
use super::utils::guess_type;
use crate::xmltree::Xml;

/// Port of `valid_empty_formats`.
const VALID_EMPTY_FORMATS: &[&str] = &["epub", "txt", "docx", "azw3", "md"];

/// Port of `create_toc`.
pub fn create_toc(mi: &MetaInformation, opf: &Xml, html_name: &str, lang: &str) -> Result<Xml> {
    let ns = opf_namespaces();
    let uuid = opf
        .opf_xpath(r#"//*[@id="uuid_id"]"#, &ns)
        .first()
        .and_then(|&id| opf.element_text(id))
        .unwrap_or("")
        .to_string();
    let mut toc = Toc::new();
    toc.add(
        toc.root,
        Some("Start".to_string()),
        Some(html_name.to_string()),
        None,
    );
    create_ncx(&toc, |x: &str| x.to_string(), &mi.title, lang, &uuid)
}

/// Port of `create_book`: creates an empty book in the specified format
/// at the specified location.
pub fn create_book(
    mi: &MetaInformation,
    path: &Path,
    fmt: &str,
    opf_name: &str,
    html_name: &str,
    toc_name: &str,
) -> Result<()> {
    if !VALID_EMPTY_FORMATS.contains(&fmt) {
        bail!("Cannot create empty book in the {fmt} format");
    }

    // Port of `mi.is_null('title')`: true when the title is empty or
    // equal to calibre's "unknown title" sentinel (`MetaInformation`'s
    // own default), matching `calibre.ebooks.metadata.book.base
    // .Metadata.is_null`'s documented rule for `title` exactly.
    let title_is_null = mi.title.is_empty() || mi.title == "Unknown";

    if fmt == "txt" {
        let data = if title_is_null {
            Vec::new()
        } else {
            mi.title.clone().into_bytes()
        };
        fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
        return Ok(());
    }
    if fmt == "md" {
        let data = if title_is_null {
            String::new()
        } else {
            format!("# {}\n", mi.title)
        };
        fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
        return Ok(());
    }
    if fmt == "docx" {
        // Calibre's recommended one-inch margins, matching Python's
        // explicit `margin_left/right/top/bottom = 72` overrides (72pt
        // == 1in) -- `PageOptions::default()` already uses these.
        let writer = DocxWriter::new(PageOptions::default());
        let file = fs::File::create(path)
            .with_context(|| format!("Failed to create {}", path.display()))?;
        writer.write(file, mi)?;
        return Ok(());
    }

    let mut lang = "und".to_string();
    let mut opf = Xml::parse(&mi.to_xml())?;
    let ns = opf_namespaces();
    for l in opf.opf_xpath("//*", &ns) {
        if opf.local_name(l) == Some("language") {
            if let Some(text) = opf.element_text(l) {
                if !text.is_empty() {
                    lang = text.to_string();
                    break;
                }
            }
        }
    }

    let package = opf
        .root_element()
        .ok_or_else(|| anyhow::anyhow!("freshly generated OPF has no root element"))?;
    let manifest = opf.new_element("manifest", Some(OPF2_NS));
    opf.insert_element(package, manifest, Some(1));
    let html_item = opf.new_element("item", Some(OPF2_NS));
    opf.set_attr(html_item, "href", html_name);
    opf.set_attr(html_item, "id", "start");
    opf.set_attr(html_item, "media-type", guess_type("a.xhtml"));
    opf.insert_element(manifest, html_item, None);
    let ncx_item = opf.new_element("item", Some(OPF2_NS));
    opf.set_attr(ncx_item, "href", toc_name);
    opf.set_attr(ncx_item, "id", "ncx");
    opf.set_attr(ncx_item, "media-type", guess_type(toc_name));
    opf.insert_element(manifest, ncx_item, None);
    let spine = opf.new_element("spine", Some(OPF2_NS));
    opf.set_attr(spine, "toc", "ncx");
    opf.insert_element(package, spine, Some(2));
    let itemref = opf.new_element("itemref", Some(OPF2_NS));
    opf.set_attr(itemref, "idref", "start");
    opf.insert_element(spine, itemref, None);

    let container_xml = format!(
        "<?xml version=\"1.0\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n   <rootfiles>\n      <rootfile full-path=\"{}\" media-type=\"application/oebps-package+xml\"/>\n   </rootfiles>\n</container>\n    ",
        crate::xml_util::prepare_string_for_xml(opf_name, true),
    );

    let html_template = include_str!("../../../resources/templates/new_book.html");
    let html_source = html_template
        .replace(
            "_LANGUAGE_",
            &crate::xml_util::prepare_string_for_xml(&lang, true),
        )
        .replace(
            "_TITLE_",
            &crate::xml_util::prepare_string_for_xml(&mi.title, false),
        )
        .replace(
            "_AUTHORS_",
            &crate::xml_util::prepare_string_for_xml(&authors_to_string(&mi.authors), false),
        );
    let mut html_dom = parsing::parse(&html_source, true, false);
    pretty_html_tree(&mut html_dom)?;
    let html_bytes = html_dom.serialize(html_dom.root).into_bytes();

    let ncx = create_toc(mi, &opf, html_name, &lang)?;
    let ncx_bytes = ncx.serialize();

    if let Some(root) = opf.root_element() {
        pretty_xml_tree(&mut opf, root, 0, "  ");
    }
    let opf_bytes = opf.serialize();

    if fmt == "azw3" {
        todo!(
            "placeholder: AZW3 output needs Container::opf_to_azw3 -- see \
             container.rs's Azw3Container::commit docs for the tracked gap \
             (issue #161/#157)"
        );
    }

    build_zip_atomic(path, |writer| {
        let stored =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("mimetype", stored)?;
        writer.write_all(b"application/epub+zip")?;
        writer.add_directory("META-INF", stored.unix_permissions(0o755))?;
        writer.start_file("META-INF/container.xml", stored)?;
        writer.write_all(container_xml.as_bytes())?;
        writer.start_file(opf_name, stored)?;
        writer.write_all(&opf_bytes)?;
        writer.start_file(html_name, stored)?;
        writer.write_all(&html_bytes)?;
        writer.start_file(toc_name, stored)?;
        writer.write_all(&ncx_bytes)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mi() -> MetaInformation {
        MetaInformation::new("Test Book", vec!["Kovid Goyal".to_string()])
    }

    #[test]
    fn create_book_txt_writes_title_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.txt");
        create_book(
            &sample_mi(),
            &path,
            "txt",
            "metadata.opf",
            "start.xhtml",
            "toc.ncx",
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Test Book");
    }

    #[test]
    fn create_book_txt_skips_null_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.txt");
        create_book(
            &MetaInformation::default(),
            &path,
            "txt",
            "metadata.opf",
            "start.xhtml",
            "toc.ncx",
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn create_book_md_writes_heading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.md");
        create_book(
            &sample_mi(),
            &path,
            "md",
            "metadata.opf",
            "start.xhtml",
            "toc.ncx",
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Test Book\n");
    }

    #[test]
    fn create_book_docx_produces_valid_package() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.docx");
        create_book(
            &sample_mi(),
            &path,
            "docx",
            "metadata.opf",
            "start.xhtml",
            "toc.ncx",
        )
        .unwrap();
        let file = fs::File::open(&path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        assert!(zip.by_name("word/document.xml").is_ok());
        assert!(zip.by_name("docProps/core.xml").is_ok());
    }

    #[test]
    fn create_book_epub_reopens_as_a_container() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        create_book(
            &sample_mi(),
            &path,
            "epub",
            "metadata.opf",
            "start.xhtml",
            "toc.ncx",
        )
        .unwrap();

        let tdir = tempfile::tempdir().unwrap();
        let mut c =
            crate::oeb::polish::container::EpubContainer::open_zip(&path, tdir.path()).unwrap();
        assert!(c.exists("start.xhtml"));
        assert!(c.exists("toc.ncx"));
        assert_eq!(c.opf_version_parsed().unwrap(), (2, 0));
        let manifest_names: Vec<String> = c.manifest_id_map().unwrap().into_values().collect();
        assert!(manifest_names.contains(&"start.xhtml".to_string()));
        assert!(manifest_names.contains(&"toc.ncx".to_string()));
        let html = c.raw_data("start.xhtml", true).unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("Test Book"));
    }

    #[test]
    fn create_toc_start_entry_points_at_html_name() {
        let opf = Xml::parse(&sample_mi().to_xml()).unwrap();
        let ncx = create_toc(&sample_mi(), &opf, "start.xhtml", "en").unwrap();
        let bytes = ncx.serialize();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("navPoint"));
        assert!(text.contains("start.xhtml"));
    }
}
