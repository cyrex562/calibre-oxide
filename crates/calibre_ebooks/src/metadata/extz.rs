use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(&mut stream).context("Failed to read zip archive")?;

    // Find the first .opf file in the archive
    let mut opf_name = String::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let name = file.name();
        if name.ends_with(".opf") && !name.contains('/') {
            opf_name = name.to_string();
            break;
        }
    }

    if opf_name.is_empty() {
        bail!("No OPF found in archive");
    }

    // Read OPF content
    let opf_content = {
        let mut f = archive.by_name(&opf_name)?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    // Parse Metadata
    let mut mi = parse_opf(&opf_content)?;

    // Extract cover if available
    let mut cover_href = None;

    // Check raster cover / guide cover logic (simplified from Python)
    // 1. Check if cover_id is set
    if let Some(cover_id) = &mi.cover_id {
        // Find href for this ID
        // Simplified search in OPF content for href associated with ID
        // Better: parse_opf should probably return this map, but currently returns MetaInformation.
        // We'll use a helper helper similar to epub if needed, or parse_opf ensures it sets something?
        // Actually, parse_opf sets `cover_id`. Use simple XML scan for ID -> Href match if not exposed.
        cover_href = find_href_by_id(&opf_content, cover_id);
    }

    // Python fallback logic: check for meta name="cover", guide items, etc.
    // parse_opf likely handles the 'meta name="cover"' -> sets cover_id.

    // If we have an href, verify it exists and is an image
    if let Some(href) = cover_href {
        if let Ok(mut file) = archive.by_name(&href) {
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            let ext = std::path::Path::new(&href)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg")
                .to_string();
            mi.cover_data = (Some(ext), data);
        }
    }

    Ok(mi)
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
    fn test_extz_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            // metadata.opf
            zip.start_file("metadata.opf", options)?;
            let opf = r#"
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                    <dc:title>EXTZ Title</dc:title>
                    <dc:creator>EXTZ Author</dc:creator>
                    <meta name="cover" content="cover-id"/>
                </metadata>
                <manifest>
                    <item id="cover-id" href="cover.jpg" media-type="image/jpeg"/>
                </manifest>
            </package>
            "#;
            Write::write_all(&mut zip, opf.as_bytes())?;

            // cover.jpg
            zip.start_file("cover.jpg", options)?;
            Write::write_all(&mut zip, b"extz cover")?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "EXTZ Title");
        assert_eq!(mi.authors, vec!["EXTZ Author"]);
        assert!(mi.cover_data.1.starts_with(b"extz cover"));

        Ok(())
    }
}
