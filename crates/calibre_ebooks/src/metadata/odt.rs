use crate::metadata::{string_to_authors, MetaInformation};
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(&mut stream)?;

    // Read meta.xml
    let mut meta_file = archive.by_name("meta.xml").context("No meta.xml in ODT")?;

    let mut xml = String::new();
    meta_file.read_to_string(&mut xml)?;

    parse_metadata(&xml)
}

fn parse_metadata(xml: &str) -> Result<MetaInformation> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut mi = MetaInformation::default();

    // Namespaces in roxmltree are handled via Uri.
    // DC = http://purl.org/dc/elements/1.1/
    // META = urn:oasis:names:tc:opendocument:xmlns:meta:1.0

    const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
    const META_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:meta:1.0";

    // Traverse
    for node in doc.descendants() {
        if node.is_element() {
            if node.tag_name().namespace() == Some(DC_NS) {
                match node.tag_name().name() {
                    "title" => {
                        if let Some(t) = node.text() {
                            mi.title = t.trim().to_string();
                        }
                    }
                    "creator" | "initial-creator" => {
                        if let Some(t) = node.text() {
                            mi.authors = string_to_authors(t);
                        }
                    }
                    "description" => {
                        if let Some(t) = node.text() {
                            mi.comments = Some(t.trim().to_string());
                        }
                    }
                    "language" => {
                        if let Some(t) = node.text() {
                            mi.languages = vec![t.trim().to_string()];
                        }
                    }
                    "subject" => {
                        if let Some(t) = node.text() {
                            mi.tags.push(t.trim().to_string());
                        }
                    }
                    _ => {}
                }
            } else if node.tag_name().namespace() == Some(META_NS) {
                match node.tag_name().name() {
                    "keyword" => {
                        if let Some(t) = node.text() {
                            mi.tags.push(t.trim().to_string());
                        }
                    }
                    "user-defined" => {
                        // Custom metadata
                        if let Some(name) = node.attribute((META_NS, "name")) {
                            if let Some(val) = node.text() {
                                mi.user_metadata
                                    .insert(name.to_lowercase(), val.to_string());

                                // Map back to Calibre fields if possible
                                match name.to_lowercase().as_str() {
                                    "opf.series" => mi.series = Some(val.to_string()),
                                    "opf.seriesindex" => {
                                        if let Ok(idx) = val.parse::<f64>() {
                                            mi.series_index = idx;
                                        }
                                    }
                                    "opf.publisher" => mi.publisher = Some(val.to_string()),
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    #[test]
    fn test_odt_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("meta.xml", options)?;

            let xml = r#"
            <office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                                  xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
                                  xmlns:dc="http://purl.org/dc/elements/1.1/">
                <office:meta>
                    <dc:title>My ODT Title</dc:title>
                    <dc:creator>Jane Doe</dc:creator>
                    <dc:description>Description</dc:description>
                    <meta:keyword>tag1</meta:keyword>
                    <meta:user-defined meta:name="opf.series">My Series</meta:user-defined>
                    <meta:user-defined meta:name="opf.seriesindex">2.5</meta:user-defined>
                </office:meta>
            </office:document-meta>
            "#;
            zip.write_all(xml.as_bytes())?;
            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "My ODT Title");
        assert_eq!(mi.authors, vec!["Jane Doe"]);
        assert_eq!(mi.comments, Some("Description".to_string()));
        assert_eq!(mi.tags, vec!["tag1"]);
        assert_eq!(mi.series, Some("My Series".to_string()));
        assert_eq!(mi.series_index, 2.5);

        Ok(())
    }
}
