use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(stream).context("Failed to open DOCX archive")?;
    let mut mi = MetaInformation::default();

    // 1. docProps/core.xml (DC Metadata)
    if let Ok(mut file) = archive.by_name("docProps/core.xml") {
        let mut xml = String::new();
        file.read_to_string(&mut xml)?;
        // Simple XML parsing using roxmltree
        if let Ok(doc) = roxmltree::Document::parse(&xml) {
            for node in doc.descendants() {
                match node.tag_name().name() {
                    "title" => {
                        if let Some(t) = node.text() {
                            mi.title = t.to_string();
                        }
                    }
                    "creator" => {
                        if let Some(t) = node.text() {
                            mi.authors = vec![t.to_string()];
                        }
                    } // Split by comma?
                    "description" => {
                        if let Some(t) = node.text() {
                            mi.comments = Some(t.to_string());
                        }
                    }
                    "subject" => {
                        if let Some(t) = node.text() {
                            mi.tags = t
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                    }
                    "keywords" => {
                        if let Some(t) = node.text() {
                            let kw = t
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>();
                            mi.tags.extend(kw);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // 2. docProps/app.xml (Publisher)
    if let Ok(mut file) = archive.by_name("docProps/app.xml") {
        let mut xml = String::new();
        file.read_to_string(&mut xml)?;
        if let Ok(doc) = roxmltree::Document::parse(&xml) {
            if let Some(company) = doc.descendants().find(|n| n.tag_name().name() == "Company") {
                if let Some(t) = company.text() {
                    mi.publisher = Some(t.to_string());
                }
            }
        }
    }

    // 3. Cover (docProps/thumbnail.jpeg or similar)
    // Priority: docProps/thumbnail.jpeg, .jpg, .png
    let candidates = [
        "docProps/thumbnail.jpeg",
        "docProps/thumbnail.jpg",
        "docProps/thumbnail.png",
    ];
    for cand in candidates {
        if let Ok(mut file) = archive.by_name(cand) {
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            let ext = std::path::Path::new(cand)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("jpg")
                .to_string();
            mi.cover_data = (Some(ext), data);
            break;
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use zip::write::FileOptions;

    #[test]
    fn test_docx_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            // docProps/core.xml
            zip.start_file("docProps/core.xml", options)?;
            let core = r#"
            <cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>Test Document</dc:title>
                <dc:creator>John Doe</dc:creator>
                <dc:description>A test document.</dc:description>
                <cp:keywords>tag1, tag2</cp:keywords>
            </cp:coreProperties>
            "#;
            Write::write_all(&mut zip, core.as_bytes())?;

            // docProps/app.xml
            zip.start_file("docProps/app.xml", options)?;
            let app = r#"
            <Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
                <Company>Acme Corp</Company>
            </Properties>
            "#;
            Write::write_all(&mut zip, app.as_bytes())?;

            // docProps/thumbnail.jpeg
            zip.start_file("docProps/thumbnail.jpeg", options)?;
            Write::write_all(&mut zip, b"image data")?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Test Document");
        assert_eq!(mi.authors, vec!["John Doe"]);
        assert_eq!(mi.comments.as_deref(), Some("A test document."));
        assert_eq!(mi.publisher.as_deref(), Some("Acme Corp"));
        assert!(mi.tags.contains(&"tag1".to_string()));
        assert!(mi.cover_data.1.starts_with(b"image data"));

        Ok(())
    }
}
