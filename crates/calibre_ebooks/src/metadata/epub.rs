use crate::html_cover_fallback::{extract_calibre_cover, extract_cover_from_embedded_svg};
use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use crate::xmltree::{Xml, XmlNodeId};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::Path;
use zip::ZipArchive;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(&mut stream).context("Failed to read zip")?;

    // 1. Read META-INF/container.xml
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

    // 2. Read OPF
    let opf_content = {
        let mut f = archive
            .by_name(&opf_path)
            .context(format!("OPF file {} not found in archive", opf_path))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    // 3. Parse Metadata
    let mut meta = parse_opf(&opf_content)?;

    // 4. Extract Cover if available
    if let Some(cover_id) = &meta.cover_id {
        // We need to find the manifest item with this ID to get the relative href
        let cover_href = find_href_by_id(&opf_content, cover_id);

        if let Some(href) = cover_href {
            // Resolve relative path. OPF path is `a/b/content.opf`, href is `images/cover.jpg` -> `a/b/images/cover.jpg`.
            let opf_dir = std::path::Path::new(&opf_path)
                .parent()
                .unwrap_or(std::path::Path::new(""));
            // This path joining in Zip is usually forward slashes.
            // Simplified join:
            let full_path = if opf_dir.as_os_str().is_empty() {
                href.clone()
            } else {
                // Hacky join for zip paths (always forward slash)
                let dir = opf_dir.to_string_lossy().replace("\\", "/");
                format!("{}/{}", dir, href)
            };

            // Normalize path (remove ./ etc)?
            // zip crate `by_name` might be strict.

            // Read the item's bytes first, in a scope of its own so the
            // borrow `file` holds on `archive` ends before any further
            // archive access (the HTML-cover fallback below needs to
            // read a second, different archive entry).
            let data = {
                if let Ok(mut file) = archive.by_name(&full_path) {
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)?;
                    Some(buf)
                } else {
                    None
                }
            };

            if let Some(data) = data {
                if calibre_utils::imghdr::what(&data).is_some() {
                    let ext = std::path::Path::new(&href)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("jpg")
                        .to_string();
                    meta.cover_data = (Some(ext), data);
                } else if let Some(cover) = extract_html_cover(&mut archive, &full_path, &data) {
                    // The manifest cover item isn't a recognized raster
                    // format -- port of `render_html_svg_workaround`'s
                    // pure-parsing pre-fallbacks for an HTML/XHTML
                    // titlepage cover. Real, disclosed narrowing: the
                    // final Qt-WebEngine rendering fallback isn't
                    // ported (see `html_cover_fallback`'s own module
                    // doc), so a titlepage needing actual CSS layout
                    // produces no cover here rather than crashing or
                    // (the bug this fixes) silently mislabeling the raw
                    // HTML source as image bytes.
                    meta.cover_data = (Some("jpg".to_string()), cover);
                }
            }
        }
    }

    Ok(meta)
}

/// Runs `html_cover_fallback`'s pure-parsing pre-fallbacks against a
/// manifest item that turned out not to be a raster image, reading any
/// referenced sibling resource (e.g. an embedded SVG's linked raster
/// image) directly from `archive` instead of extracting to disk --
/// see `html_cover_fallback`'s own module doc for why a resolver
/// callback is used here instead of a filesystem path.
fn extract_html_cover<R: Read + Seek>(archive: &mut ZipArchive<R>, cover_item_path: &str, cover_item_bytes: &[u8]) -> Option<Vec<u8>> {
    let (raw, _encoding) = crate::chardet::xml_to_unicode(cover_item_bytes, true, false);
    let dir = Path::new(cover_item_path).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();

    let mut resolve = |rel: &str| -> Option<Vec<u8>> {
        let resolved = if dir.is_empty() { rel.to_string() } else { format!("{dir}/{rel}") };
        let mut f = archive.by_name(&resolved).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        Some(buf)
    };

    let mut cover = None;
    if raw.contains(SVG_NS) {
        cover = extract_cover_from_embedded_svg(&raw, &mut resolve);
    }
    if cover.is_none() {
        cover = extract_calibre_cover(&raw, &mut resolve);
    }
    cover
}

fn extract_opf_path_from_container(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    root.descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("rootfile"))
        .and_then(|n| n.attribute("full-path").map(|s| s.to_string()))
}

fn find_href_by_id(xml: &str, id: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    // <manifest><item id="..." href="..."/></manifest>
    root.descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("item") && n.attribute("id") == Some(id))
        .and_then(|n| n.attribute("href").map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    #[test]
    fn test_epub_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            // container.xml
            zip.start_file("META-INF/container.xml", options)?;
            Write::write_all(&mut zip, br#"<container version="1.0"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#)?;

            // content.opf
            zip.start_file("content.opf", options)?;
            let opf = r#"
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
                    <dc:title>EPUB Title</dc:title>
                    <dc:creator opf:role="aut">EPUB Author</dc:creator>
                    <meta name="cover" content="cover-image"/>
                </metadata>
                <manifest>
                    <item id="cover-image" href="cover.jpg" media-type="image/jpeg"/>
                </manifest>
            </package>
            "#;
            Write::write_all(&mut zip, opf.as_bytes())?;

            // cover.jpg -- real recognized-format bytes (a bare JPEG
            // SOI+EOI marker pair), not a placeholder string: a fake
            // "image data" string would pass even if `get_metadata`
            // stopped actually validating the format at all.
            zip.start_file("cover.jpg", options)?;
            Write::write_all(&mut zip, JPEG_MAGIC)?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "EPUB Title");
        assert_eq!(mi.authors, vec!["EPUB Author"]);
        assert_eq!(mi.cover_data.1, JPEG_MAGIC);

        Ok(())
    }

    const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF, 0xD9];

    /// The real bug this issue fixed: a manifest cover item that's
    /// actually an HTML/XHTML titlepage (a real, valid EPUB shape) used
    /// to have its raw HTML source text returned as `cover_data`,
    /// mislabeled as image bytes. Now it routes through
    /// `html_cover_fallback`'s pure-parsing pre-fallback chain instead.
    #[test]
    fn html_cover_item_falls_back_to_the_real_calibre_cover_extractor() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("META-INF/container.xml", options)?;
            Write::write_all(&mut zip, br#"<container version="1.0"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#)?;

            zip.start_file("content.opf", options)?;
            let opf = r#"
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
                    <dc:title>EPUB Title</dc:title>
                    <meta name="cover" content="titlepage"/>
                </metadata>
                <manifest>
                    <item id="titlepage" href="cover.html" media-type="application/xhtml+xml"/>
                </manifest>
            </package>
            "#;
            Write::write_all(&mut zip, opf.as_bytes())?;

            // A "simple cover": a body with no text and exactly one
            // <img> -- extract_calibre_cover's own real second branch.
            zip.start_file("cover.html", options)?;
            Write::write_all(&mut zip, br#"<html><body><img src="images/real-cover.jpg"/></body></html>"#)?;

            zip.start_file("images/real-cover.jpg", options)?;
            Write::write_all(&mut zip, JPEG_MAGIC)?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;
        assert_eq!(mi.cover_data.1, JPEG_MAGIC, "should resolve the real referenced image, not the HTML source text");
        assert_eq!(mi.cover_data.0.as_deref(), Some("jpg"));

        Ok(())
    }

    /// When the cover item is HTML but neither pure-parsing pre-
    /// fallback matches (a real titlepage needing actual CSS layout),
    /// no cover is produced -- the real, disclosed narrowing, not a
    /// crash or mislabeled data.
    #[test]
    fn html_cover_item_with_real_text_produces_no_cover() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("META-INF/container.xml", options)?;
            Write::write_all(&mut zip, br#"<container version="1.0"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#)?;

            zip.start_file("content.opf", options)?;
            let opf = r#"
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
                    <dc:title>EPUB Title</dc:title>
                    <meta name="cover" content="titlepage"/>
                </metadata>
                <manifest>
                    <item id="titlepage" href="cover.html" media-type="application/xhtml+xml"/>
                </manifest>
            </package>
            "#;
            Write::write_all(&mut zip, opf.as_bytes())?;

            zip.start_file("cover.html", options)?;
            Write::write_all(&mut zip, br#"<html><body><h1>My Book</h1><p>by an author</p></body></html>"#)?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;
        assert!(mi.cover_data.1.is_empty(), "no pre-fallback matches and the Qt fallback isn't ported, so no cover_data");

        Ok(())
    }
}

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const OPF_NS: &str = "http://www.idpf.org/2007/opf";

/// Writes `mi` into an existing EPUB's OPF, in place (#834).
///
/// The `metadata` element is **edited**, not regenerated. Regenerating it
/// is the obvious shortcut and it loses things that live there and are
/// not `dc:*`: the `<meta name="cover">` pointer at the cover image, and
/// calibre's own `calibre:series` / `calibre:title_sort` entries. A book
/// whose cover vanished on a metadata edit would be a bad trade for
/// simpler code.
///
/// Two invariants that a naive replace breaks, both tested:
///
/// - The `dc:identifier` named by `package/@unique-identifier` is left
///   alone. Removing every `dc:identifier` and adding an ISBN would strip
///   the book's unique id and make the EPUB invalid.
/// - Only fields `mi` actually carries are touched. A `MetaInformation`
///   with no publisher must not *delete* the book's publisher — the
///   caller is setting fields, not declaring the complete set.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let (opf_path, opf_content) = read_opf(path)?;
    let updated = rewrite_opf(&opf_content, mi)?;
    replace_zip_entry(path, &opf_path, &updated)
}

/// The OPF's path inside the archive, and its text.
fn read_opf(path: &Path) -> Result<(String, String)> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = ZipArchive::new(file).context("not a zip archive")?;

    let container_xml = {
        let mut f = archive.by_name("META-INF/container.xml").context("META-INF/container.xml not found")?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };
    let opf_path = extract_opf_path_from_container(&container_xml).context("could not find the OPF path in container.xml")?;

    let mut f = archive.by_name(&opf_path).with_context(|| format!("{opf_path} not found in the archive"))?;
    let mut opf = String::new();
    f.read_to_string(&mut opf)?;
    Ok((opf_path, opf))
}

/// Replaces every `dc:<local>` child of `metadata` with one element per
/// value.
///
/// `values` being empty means "leave this field alone", not "clear it" --
/// see [`set_metadata`]'s own doc for why.
fn replace_dc(xml: &mut Xml, metadata: XmlNodeId, local: &str, values: &[String], attrs: &[(&str, &str)]) {
    if values.is_empty() {
        return;
    }
    for child in xml.element_children(metadata) {
        if xml.namespace(child) == Some(DC_NS) && xml.local_name(child) == Some(local) {
            xml.detach(child);
        }
    }
    for value in values {
        let element = xml.new_element(local, Some(DC_NS));
        xml.set_element_text(element, value.as_str());
        for (name, attr_value) in attrs {
            xml.set_attr(element, name, *attr_value);
        }
        xml.insert_element(metadata, element, None);
    }
}

fn rewrite_opf(opf: &str, mi: &MetaInformation) -> Result<Vec<u8>> {
    let mut xml = Xml::parse(opf).context("parsing the OPF")?;
    // Read only to fail early on something that is not an OPF at all.
    xml.root_element().context("the OPF has no root element")?;

    let ns: HashMap<&str, &str> = [("opf", OPF_NS), ("dc", DC_NS)].into_iter().collect();
    let metadata = *xml
        .opf_xpath("//opf:metadata", &ns)
        .first()
        .context("the OPF has no metadata element")?;

    // `dc:` has to be declared for the elements below to serialize with a
    // usable prefix. Many OPFs declare it on `metadata` rather than
    // `package`, and some not at all when they use no dc elements yet.
    xml.ensure_namespace_declared(Some("dc"), DC_NS);
    xml.ensure_namespace_declared(Some("opf"), OPF_NS);

    if !mi.title.trim().is_empty() {
        replace_dc(&mut xml, metadata, "title", std::slice::from_ref(&mi.title), &[]);
    }
    // `opf:role="aut"` is what distinguishes an author from an editor or
    // illustrator; a reader that ignores it still shows the name, but one
    // that honours it would file the book wrongly without it.
    replace_dc(&mut xml, metadata, "creator", &mi.authors, &[("opf:role", "aut")]);
    replace_dc(&mut xml, metadata, "language", &mi.languages, &[]);
    replace_dc(&mut xml, metadata, "subject", &mi.tags, &[]);

    if let Some(publisher) = mi.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
        replace_dc(&mut xml, metadata, "publisher", std::slice::from_ref(publisher), &[]);
    }
    if let Some(comments) = mi.comments.as_ref().filter(|c| !c.trim().is_empty()) {
        replace_dc(&mut xml, metadata, "description", std::slice::from_ref(comments), &[]);
    }
    if let Some(pubdate) = mi.pubdate {
        // ISO 8601, which is what `parse_opf` reads back.
        replace_dc(&mut xml, metadata, "date", &[pubdate.to_rfc3339()], &[]);
    }

    set_identifiers(&mut xml, metadata, mi);
    Ok(xml.serialize())
}

/// Writes `mi`'s identifiers, preserving the book's unique id.
///
/// The `dc:identifier` whose `id` matches `package/@unique-identifier` is
/// the book's identity. Replacing every identifier -- the obvious way to
/// write an ISBN -- removes it and leaves an EPUB that readers reject, so
/// this only ever updates an identifier matching the scheme being set, or
/// appends a new one. Nothing here detaches an identifier, which is what
/// keeps the unique one safe without having to name it.
fn set_identifiers(xml: &mut Xml, metadata: XmlNodeId, mi: &MetaInformation) {
    for (scheme, value) in &mi.identifiers {
        if value.trim().is_empty() {
            continue;
        }
        let wanted_scheme = scheme.to_uppercase();

        // Update an existing element for this scheme if there is one,
        // rather than adding a duplicate.
        let mut updated = false;
        for child in xml.element_children(metadata) {
            if xml.namespace(child) != Some(DC_NS) || xml.local_name(child) != Some("identifier") {
                continue;
            }
            if xml.get_attr(child, "opf:scheme").map(|s| s.to_uppercase()) == Some(wanted_scheme.clone()) {
                xml.set_element_text(child, value.as_str());
                updated = true;
                break;
            }
        }
        if updated {
            continue;
        }

        // The unique identifier is never repurposed, even when it has no
        // scheme and this one would otherwise be a natural fit for it.
        let element = xml.new_element("identifier", Some(DC_NS));
        xml.set_element_text(element, value.as_str());
        xml.set_attr(element, "opf:scheme", wanted_scheme);
        xml.insert_element(metadata, element, None);
    }
}

/// Rewrites `archive_path` inside the zip at `path`, leaving every other
/// entry byte-identical.
///
/// Writes a whole new archive to a temp file and renames over the
/// original, so an interrupted write cannot truncate somebody's book.
fn replace_zip_entry(path: &Path, archive_path: &str, content: &[u8]) -> Result<()> {
    let staging = tempfile::Builder::new().prefix("set-metadata").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    {
        // The source archive is opened *inside* this block so its file
        // handle is closed before the rename below. Windows refuses to
        // replace a file that anything still has open ("Access is
        // denied", os error 5) where Unix allows it -- so on Linux this
        // scoping looks like style and on Windows it is the difference
        // between working and not.
        let mut archive = ZipArchive::new(std::fs::File::open(path)?)?;
        let mut out = zip::ZipWriter::new(std::fs::File::create(staging.path())?);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            // `mimetype` must stay first and stored, or the file stops
            // being a recognisable EPUB. Preserving each entry's own
            // compression method keeps that true without special-casing.
            let options = zip::write::FileOptions::default().compression_method(entry.compression());
            out.start_file(&name, options)?;
            if name == archive_path {
                out.write_all(content)?;
            } else {
                std::io::copy(&mut entry, &mut out)?;
            }
        }
        out.finish()?;
    }

    // `persist` renames, which is atomic within a filesystem -- and the
    // temp file is deliberately created beside the book so it is the same
    // one.
    staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;

    /// A real EPUB whose OPF has the things a naive metadata rewrite
    /// destroys: a unique identifier, a cover pointer, and a
    /// calibre-specific `meta`.
    fn write_epub(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        zip.start_file("mimetype", zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();

        let deflated = zip::write::FileOptions::default();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();

        zip.start_file("content.opf", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Old Title</dc:title>
    <dc:creator opf:role="aut">Old Author</dc:creator>
    <dc:language>fr</dc:language>
    <dc:publisher>Old Publisher</dc:publisher>
    <dc:identifier id="uid">urn:uuid:11111111-1111-1111-1111-111111111111</dc:identifier>
    <meta name="cover" content="cover-image"/>
    <meta name="calibre:series" content="Old Series"/>
  </metadata>
  <manifest>
    <item id="c1" href="c1.html" media-type="application/xhtml+xml"/>
    <item id="cover-image" href="cover.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();

        zip.start_file("c1.html", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Text</p></body></html>"#).unwrap();
        zip.start_file("cover.jpg", deflated).unwrap();
        zip.write_all(b"\xff\xd8\xff not really a jpeg").unwrap();

        zip.finish().unwrap();
    }

    fn opf_of(path: &Path) -> String {
        let mut archive = ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut opf = String::new();
        archive.by_name("content.opf").unwrap().read_to_string(&mut opf).unwrap();
        opf
    }

    fn a_book(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("book.epub");
        write_epub(&path);
        path
    }

    #[test]
    fn title_and_authors_round_trip_through_get_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "New Title");
        assert_eq!(read_back.authors, vec!["Ann Author".to_string(), "Bob Writer".to_string()]);
    }

    /// The trap a naive "replace all dc:identifier" implementation falls
    /// into: the book's unique id is a `dc:identifier`, and losing it
    /// makes the EPUB invalid.
    #[test]
    fn setting_an_isbn_keeps_the_books_unique_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.identifiers.insert("isbn".to_string(), "9780441013593".to_string());
        set_metadata(&path, &mi).unwrap();

        let opf = opf_of(&path);
        assert!(opf.contains("urn:uuid:11111111-1111-1111-1111-111111111111"), "the unique identifier was lost:\n{opf}");
        assert!(opf.contains("9780441013593"), "the ISBN was not written:\n{opf}");
    }

    /// The other trap: regenerating `metadata` wholesale drops the cover
    /// pointer, so the book silently loses its cover on a title edit.
    #[test]
    fn the_cover_pointer_and_calibre_meta_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let opf = opf_of(&path);
        assert!(opf.contains(r#"name="cover""#), "the cover pointer was lost:\n{opf}");
        assert!(opf.contains("calibre:series"), "calibre's own metadata was lost:\n{opf}");
    }

    /// Setting a title must not clear the publisher. The caller is
    /// setting fields, not declaring the complete set.
    #[test]
    fn a_field_the_caller_did_not_set_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.publisher.as_deref(), Some("Old Publisher"), "the publisher should have been left as it was");
    }

    /// Writing metadata must not disturb the rest of the book.
    #[test]
    fn the_other_zip_entries_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);
        let before: Vec<u8> = {
            let mut archive = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
            let mut bytes = Vec::new();
            archive.by_name("cover.jpg").unwrap().read_to_end(&mut bytes).unwrap();
            bytes
        };

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let mut archive = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut after = Vec::new();
        archive.by_name("cover.jpg").unwrap().read_to_end(&mut after).unwrap();
        assert_eq!(after, before, "the cover image should be byte-identical");
        assert!(archive.by_name("c1.html").is_ok(), "the content should still be there");
    }

    /// `mimetype` must remain the first entry and stored uncompressed, or
    /// the file stops being a recognisable EPUB.
    #[test]
    fn the_mimetype_entry_stays_first_and_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let mut archive = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let first = archive.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored, "mimetype must not be deflated");
    }

    /// Repeated edits must not accumulate duplicate elements.
    #[test]
    fn setting_the_title_twice_leaves_one_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "First".to_string();
        set_metadata(&path, &mi).unwrap();
        mi.title = "Second".to_string();
        set_metadata(&path, &mi).unwrap();

        let opf = opf_of(&path);
        assert_eq!(opf.matches("<dc:title").count(), 1, "titles accumulated:\n{opf}");
        assert!(opf.contains("Second"));
        assert!(!opf.contains("First"));
    }

    #[test]
    fn tags_and_languages_are_written_as_repeated_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_book(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New".to_string();
        mi.tags = vec!["Science Fiction".to_string(), "Classics".to_string()];
        mi.languages = vec!["en".to_string()];
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(read_back.tags.contains(&"Science Fiction".to_string()), "{:?}", read_back.tags);
        assert!(read_back.tags.contains(&"Classics".to_string()), "{:?}", read_back.tags);
        assert_eq!(read_back.languages, vec!["en".to_string()], "the old French language should have been replaced");
    }
}
