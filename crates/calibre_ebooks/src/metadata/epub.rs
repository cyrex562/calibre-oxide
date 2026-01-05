use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

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

            if let Ok(mut file) = archive.by_name(&full_path) {
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                // Extension?
                let ext = std::path::Path::new(&href)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("jpg")
                    .to_string();
                meta.cover_data = (Some(ext), data);
            }
        }
    }

    Ok(meta)
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

            // cover.jpg
            zip.start_file("cover.jpg", options)?;
            Write::write_all(&mut zip, b"image data")?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "EPUB Title");
        assert_eq!(mi.authors, vec!["EPUB Author"]);
        assert!(mi.cover_data.1.starts_with(b"image data"));

        Ok(())
    }
}
