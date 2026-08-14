//! Port of `old_src/src/calibre/ebooks/odt/input.py`'s `Extract.__call__`
//! orchestration: unzip an ODT file, resolve its metadata, convert
//! `content.xml` to XHTML, run the calibre-specific post-processing
//! fixups, extract embedded pictures, and assemble an [`OEBBook`].
//!
//! The real ODT-content -\> XHTML conversion lives in
//! [`crate::odt::convert`] (a from-scratch, intentionally scoped-down
//! implementation -- see that module's docs for exactly what ODF markup
//! is and isn't handled); this module ports the surrounding
//! calibre-specific logic `Extract` adds on top of the (separately
//! tracked, not-yet-ported) `odf2xhtml.ODF2XHTML` base class:
//! `extract_pictures`, `fix_markup` (via [`crate::odt::fixup`]),
//! `search_page_img`/`filter_cover` (via [`crate::odt::cover`]), and the
//! overall `__call__` sequencing.
//!
//! Unlike Python, which writes an on-disk `metadata.opf` intermediate via
//! `OPFCreator`, this returns an in-memory [`OEBBook`] directly, matching
//! every other input plugin in `crate::input`.

use crate::metadata::odt::get_metadata;
use crate::odt::{convert, cover, fixup};
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

pub struct ODTInput;

impl Default for ODTInput {
    fn default() -> Self {
        Self::new()
    }
}

impl ODTInput {
    pub fn new() -> Self {
        ODTInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        fs::create_dir_all(output_dir).context("Failed to create output directory")?;

        // `get_metadata` (already a real, tested port of
        // `calibre.ebooks.metadata.odt.get_metadata`) wants its own
        // stream, separate from the `ZipArchive` used below for
        // content/pictures.
        let meta_file =
            fs::File::open(input_path).context("Failed to open ODT file for metadata")?;
        let mut mi = get_metadata(meta_file).unwrap_or_default();
        // Port of `if not mi.title: mi.title = _('Unknown')` /
        // `if not mi.authors: mi.authors = [_('Unknown')]`.
        if mi.title.trim().is_empty() {
            mi.title = "Unknown".to_string();
        }
        if mi.authors.is_empty() {
            mi.authors = vec!["Unknown".to_string()];
        }

        let file = fs::File::open(input_path).context("Failed to open ODT file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ODT zip")?;

        let content_xml =
            read_zip_text(&mut archive, "content.xml").context("ODT file has no content.xml")?;
        let styles_xml = read_zip_text(&mut archive, "styles.xml").ok();

        // Port of `Extract.filter_load`'s pre-conversion filtering step
        // (`search_page_img`/`filter_cover`), run against the raw
        // `content.xml` before conversion.
        let content_xml = match roxmltree::Document::parse(&content_xml) {
            Ok(doc) => {
                if cover::has_page_anchored_frame(&doc) {
                    eprintln!(
                        "Document has Pictures anchored to Page, will all end up before first page!"
                    );
                }
                // `filter_cover` needs a detected cover frame name/href.
                // `crate::metadata::odt::get_metadata` does not currently
                // populate those (no `odf_cover_frame` equivalent on
                // `MetaInformation` -- see `crate::odt::cover`'s module
                // docs for why that's a gap in the already-ported/closed
                // metadata module, not this issue), so there is nothing
                // to pass here yet; `cover::filter_cover` itself is real
                // and unit-tested, just not reachable from this call site
                // until that metadata gap is closed.
                content_xml
            }
            Err(_) => content_xml,
        };

        let converted = convert::convert_content(&content_xml, styles_xml.as_deref(), &mi.title)
            .context("Failed to convert ODT content to XHTML")?;
        let fixed = fixup::fix_markup(&converted.xhtml, &converted.list_starts);

        let content_href = "index.html";
        fs::write(output_dir.join(content_href), &fixed.html)
            .context("Failed to write index.html")?;
        if !fixed.external_css.trim().is_empty() {
            fs::write(output_dir.join("odfpy.css"), &fixed.external_css)
                .context("Failed to write odfpy.css")?;
        }

        let picture_hrefs = extract_pictures(&mut archive, output_dir)?;

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        let content_id = "content".to_string();
        book.manifest.items.insert(
            content_id.clone(),
            ManifestItem::new(&content_id, content_href, "application/xhtml+xml"),
        );
        book.manifest
            .hrefs
            .insert(content_href.to_string(), content_id.clone());
        book.spine.add(&content_id, true);

        for (i, href) in picture_hrefs.iter().enumerate() {
            let item_id = format!("image_{i}");
            let media_type = mime_guess::from_path(href)
                .first_or_octet_stream()
                .to_string();
            book.manifest.items.insert(
                item_id.clone(),
                ManifestItem::new(&item_id, href, &media_type),
            );
            book.manifest.hrefs.insert(href.clone(), item_id);
        }

        book.metadata.add("title", &mi.title);
        for author in &mi.authors {
            book.metadata.add("creator", author);
        }
        if let Some(desc) = &mi.comments {
            book.metadata.add("description", desc);
        }
        for lang in &mi.languages {
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("und") {
                book.metadata.add("language", lang);
            }
        }
        for tag in &mi.tags {
            book.metadata.add("subject", tag);
        }
        if let Some(publisher) = &mi.publisher {
            book.metadata.add("publisher", publisher);
        }

        Ok(book)
    }
}

fn read_zip_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<String> {
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("{name} not found in ODT zip"))?;
    let mut s = String::new();
    entry
        .read_to_string(&mut s)
        .with_context(|| format!("{name} is not valid UTF-8"))?;
    Ok(s)
}

/// Port of `Extract.extract_pictures`: every zip entry under `Pictures/`
/// gets written to `output_dir/Pictures/...`, unconditionally (matching
/// Python, which extracts the whole folder regardless of which images the
/// converted document actually references). Returns the hrefs (relative
/// to `output_dir`) of every file extracted, for manifest registration.
fn extract_pictures<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    output_dir: &Path,
) -> Result<Vec<String>> {
    let pictures_dir = output_dir.join("Pictures");
    let mut hrefs = Vec::new();
    for i in 0..archive.len() {
        let (name, data) = {
            let mut entry = archive
                .by_index(i)
                .context("Failed to read ODT zip entry")?;
            let name = entry.name().to_string();
            if !name.starts_with("Pictures/") || name == "Pictures/" || entry.is_dir() {
                continue;
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .with_context(|| format!("Failed to read {name} from ODT zip"))?;
            (name, data)
        };
        let rel = name.trim_start_matches("Pictures/");
        if rel.is_empty() {
            continue;
        }
        let dest = pictures_dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("Failed to create {parent:?}"))?;
        }
        fs::write(&dest, &data).with_context(|| format!("Failed to write {dest:?}"))?;
        hrefs.push(name);
    }
    Ok(hrefs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zip::write::FileOptions;
    use zip::ZipWriter;

    fn write_test_odt(
        path: &Path,
        content_xml: &str,
        meta_xml: Option<&str>,
        picture: Option<(&str, &[u8])>,
    ) {
        let file = fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("mimetype", options).unwrap();
        zip.write_all(b"application/vnd.oasis.opendocument.text")
            .unwrap();

        zip.start_file("content.xml", options).unwrap();
        zip.write_all(content_xml.as_bytes()).unwrap();

        if let Some(meta) = meta_xml {
            zip.start_file("meta.xml", options).unwrap();
            zip.write_all(meta.as_bytes()).unwrap();
        }

        if let Some((name, data)) = picture {
            zip.start_file(name, options).unwrap();
            zip.write_all(data).unwrap();
        }

        zip.finish().unwrap();
    }

    use std::io::Write as _;

    const SIMPLE_CONTENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2">
 <office:body>
  <office:text>
   <text:p>Hello, ODT!</text:p>
  </office:text>
 </office:body>
</office:document-content>"#;

    const SIMPLE_META: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                       xmlns:dc="http://purl.org/dc/elements/1.1/">
 <office:meta>
  <dc:title>My ODT Book</dc:title>
  <dc:creator>Jane Author</dc:creator>
 </office:meta>
</office:document-meta>"#;

    #[test]
    fn converts_simple_document_with_metadata() {
        let temp_dir = tempdir().unwrap();
        let input_path = temp_dir.path().join("test.odt");
        let output_dir = temp_dir.path().join("output");
        write_test_odt(&input_path, SIMPLE_CONTENT, Some(SIMPLE_META), None);

        let input = ODTInput::new();
        let book = input.convert(&input_path, &output_dir).unwrap();

        assert!(output_dir.join("index.html").exists());
        let index_html = fs::read_to_string(output_dir.join("index.html")).unwrap();
        assert!(index_html.contains("Hello, ODT!"));
        assert!(index_html.contains("<title>My ODT Book</title>"));

        assert!(book.manifest.hrefs.contains_key("index.html"));
        let title = book.metadata.get("title");
        assert_eq!(title.len(), 1);
        assert_eq!(title[0].value, "My ODT Book");
        let creators = book.metadata.get("creator");
        assert_eq!(creators[0].value, "Jane Author");
    }

    #[test]
    fn falls_back_to_unknown_title_and_author_when_metadata_missing() {
        let temp_dir = tempdir().unwrap();
        let input_path = temp_dir.path().join("test.odt");
        let output_dir = temp_dir.path().join("output");
        write_test_odt(&input_path, SIMPLE_CONTENT, None, None);

        let input = ODTInput::new();
        let book = input.convert(&input_path, &output_dir).unwrap();

        assert_eq!(book.metadata.get("title")[0].value, "Unknown");
        assert_eq!(book.metadata.get("creator")[0].value, "Unknown");
    }

    #[test]
    fn extracts_pictures_and_adds_to_manifest() {
        let temp_dir = tempdir().unwrap();
        let input_path = temp_dir.path().join("test.odt");
        let output_dir = temp_dir.path().join("output");
        let png_bytes: &[u8] = b"\x89PNG\r\n\x1a\nfake";
        write_test_odt(
            &input_path,
            SIMPLE_CONTENT,
            None,
            Some(("Pictures/100000000000000A.png", png_bytes)),
        );

        let input = ODTInput::new();
        let book = input.convert(&input_path, &output_dir).unwrap();

        let extracted = output_dir.join("Pictures/100000000000000A.png");
        assert!(extracted.exists());
        assert_eq!(fs::read(&extracted).unwrap(), png_bytes);
        assert!(book
            .manifest
            .hrefs
            .contains_key("Pictures/100000000000000A.png"));
    }

    #[test]
    fn converts_headings_lists_and_tables_end_to_end() {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
  office:version="1.2">
 <office:automatic-styles>
  <text:list-style style:name="L1">
   <text:list-level-style-number text:level="1" style:num-format="1" text:start-value="7"/>
  </text:list-style>
 </office:automatic-styles>
 <office:body>
  <office:text>
   <text:h text:outline-level="1">Title</text:h>
   <text:list text:style-name="L1">
    <text:list-item><text:p>First</text:p></text:list-item>
   </text:list>
   <table:table>
    <table:table-row><table:table-cell><text:p>A</text:p></table:table-cell></table:table-row>
   </table:table>
  </office:text>
 </office:body>
</office:document-content>"#;

        let temp_dir = tempdir().unwrap();
        let input_path = temp_dir.path().join("test.odt");
        let output_dir = temp_dir.path().join("output");
        write_test_odt(&input_path, content, None, None);

        let input = ODTInput::new();
        input.convert(&input_path, &output_dir).unwrap();

        let html = fs::read_to_string(output_dir.join("index.html")).unwrap();
        assert!(html.contains("<h1"));
        assert!(html.contains("<ol class=\"L1_1\" start=\"7\">"), "{html}");
        assert!(html.contains("<table"));
        assert!(html.contains("First"));
    }
}
