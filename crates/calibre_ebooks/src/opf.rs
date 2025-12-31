use roxmltree::{Document, Node};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpfError {
    #[error("Failed to parse XML: {0}")]
    XmlParseError(#[from] roxmltree::Error),
    #[error("Missing metadata section in OPF")]
    MissingMetadata,
}

#[derive(Debug, Default)]
pub struct OpfMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub uuid: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub cover_id: Option<String>,
}

pub fn parse_opf(xml_content: &str) -> Result<OpfMetadata, OpfError> {
    let doc = Document::parse(xml_content)?;
    let root = doc.root_element();

    // Namespaces in XML can be tricky with roxmltree as it doesn't automatically handle prefixes via a map like lxml
    // We look for local names mostly.

    let metadata_node = root
        .children()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("metadata"))
        .ok_or(OpfError::MissingMetadata)?;

    let mut meta = OpfMetadata::default();

    for node in metadata_node.children() {
        if !node.is_element() {
            continue;
        }

        let tag = node.tag_name().name().to_lowercase();
        match tag.as_str() {
            "title" => {
                if let Some(text) = node.text() {
                    meta.title = text.to_string();
                }
            }
            "creator" => {
                // Check if it's an author (role aut or no role)
                let is_aut = node
                    .attribute("role")
                    .or_else(|| node.attribute((ns_opf(node), "role")))
                    .map(|r| r == "aut")
                    .unwrap_or(true);

                if is_aut {
                    if let Some(text) = node.text() {
                        meta.authors.push(text.to_string());
                    }
                }
            }
            "description" => {
                if let Some(text) = node.text() {
                    meta.description = Some(text.to_string());
                }
            }
            "language" => {
                if let Some(text) = node.text() {
                    meta.language = Some(text.to_string());
                }
            }
            "identifier" => {
                // Simple heuristic for UUID, in reality we check scheme
                // Often: <dc:identifier id="uuid_id" opf:scheme="uuid">
                let scheme = node
                    .attribute("scheme")
                    .or_else(|| node.attribute((ns_opf(node), "scheme")));
                if let Some(s) = scheme {
                    if s.eq_ignore_ascii_case("uuid") {
                        if let Some(text) = node.text() {
                            meta.uuid = Some(text.to_string());
                        }
                    }
                }
            }
            "meta" => {
                // Cover extraction <meta name="cover" content="cover_id"/>
                let name = node.attribute("name");
                if let Some(n) = name {
                    if n.eq_ignore_ascii_case("cover") {
                        if let Some(content) = node.attribute("content") {
                            meta.cover_id = Some(content.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(meta)
}

impl OpfMetadata {
    pub fn to_xml(&self) -> String {
        let mut xml = String::from(
            r#"<?xml version='1.0' encoding='utf-8'?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="uuid_id" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
"#,
        );

        // Title
        xml.push_str(&format!(
            "    <dc:title>{}</dc:title>\n",
            escape_xml(&self.title)
        ));

        // Authors (assuming role aut)
        for author in &self.authors {
            xml.push_str(&format!(
                "    <dc:creator opf:file-as=\"{}\" opf:role=\"aut\">{}</dc:creator>\n",
                escape_xml(author),
                escape_xml(author)
            ));
        }

        // UUID
        if let Some(uuid) = &self.uuid {
            xml.push_str(&format!(
                "    <dc:identifier opf:scheme=\"uuid\" id=\"uuid_id\">{}</dc:identifier>\n",
                escape_xml(uuid)
            ));
        }

        // Description
        if let Some(desc) = &self.description {
            xml.push_str(&format!(
                "    <dc:description>{}</dc:description>\n",
                escape_xml(desc)
            ));
        }

        // Language
        if let Some(lang) = &self.language {
            xml.push_str(&format!(
                "    <dc:language>{}</dc:language>\n",
                escape_xml(lang)
            ));
        }

        // Cover
        if let Some(cover_id) = &self.cover_id {
            xml.push_str(&format!(
                "    <meta name=\"cover\" content=\"{}\"/>\n",
                escape_xml(cover_id)
            ));
        }

        xml.push_str("  </metadata>\n  <guide/>\n</package>");
        xml
    }
}

fn ns_opf<'a, 'input>(node: Node<'a, 'input>) -> &'a str {
    node.lookup_prefix("http://www.idpf.org/2007/opf")
        .unwrap_or("opf")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_opf() {
        let xml = r#"
            <package xmlns="http://www.idpf.org/2007/opf" unique-identifier="uuid_id" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
                    <dc:title>Test Book</dc:title>
                    <dc:creator opf:file-as="Doe, John" opf:role="aut">John Doe</dc:creator>
                    <dc:identifier opf:scheme="uuid" id="uuid_id">urn:uuid:12345</dc:identifier>
                    <dc:language>en</dc:language>
                    <meta name="cover" content="cover_img"/>
                </metadata>
            </package>
        "#;

        let meta = parse_opf(xml).expect("Failed to parse");
        assert_eq!(meta.title, "Test Book");
        assert_eq!(meta.authors, vec!["John Doe"]);
        assert_eq!(meta.uuid, Some("urn:uuid:12345".to_string()));
        assert_eq!(meta.language, Some("en".to_string()));
        assert_eq!(meta.cover_id, Some("cover_img".to_string()));
    }
}
