use crate::html_cover_fallback::{extract_calibre_cover, extract_cover_from_embedded_svg};
use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use anyhow::{Context, Result};
use std::io::{Read, Seek};
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
