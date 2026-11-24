use crate::metadata::MetaInformation;
use anyhow::Result;
use roxmltree::{Document, Node};

pub fn metadata_from_xmp_packet(packet: &[u8]) -> Result<MetaInformation> {
    let s = String::from_utf8_lossy(packet);

    // Attempt to locate the RDF block roughly
    let xml_str = if let Some(start) = s.find("<rdf:RDF") {
        let end = s[start..]
            .find("</rdf:RDF>")
            .map(|i| start + i + 10)
            .unwrap_or(s.len());
        &s[start..end]
    } else {
        &s
    };

    eprintln!("Extracted XML: '{}'", xml_str);

    let doc = match Document::parse(xml_str) {
        Ok(d) => d,
        Err(_) => {
            // eprintln!("XMP parse error: {}", e);
            return Ok(MetaInformation::default());
        }
    };

    // println!("Parsed XMP: {:?}", doc);

    let mut mi = MetaInformation::default();

    for node in doc.descendants() {
        if node.is_element() {
            let name = node.tag_name().name();
            // eprintln!("Node: {}", name);
            match name {
                "title" => {
                    if let Some(t) = extract_localized_text(&node) {
                        mi.title = t;
                    }
                }
                "creator" => {
                    extract_seq_list(&node, &mut mi.authors);
                }
                "subject" => {
                    extract_seq_list(&node, &mut mi.tags);
                }
                "publisher" => {
                    extract_simple_text(&node, |t| mi.publisher = Some(t));
                }
                "description" => {
                    if let Some(t) = extract_localized_text(&node) {
                        mi.comments = Some(t);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(mi)
}

fn extract_localized_text(node: &Node) -> Option<String> {
    // Greedy extraction: find first non-empty text in descendants
    for child in node.descendants() {
        if let Some(text) = child.text() {
            let t = text.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn extract_seq_list(node: &Node, target: &mut Vec<String>) {
    for child in node.descendants() {
        if child.is_element() && child.tag_name().name() == "li" {
            if let Some(text) = child.text() {
                let t = text.trim();
                if !t.is_empty() {
                    target.push(t.to_string());
                }
            }
        }
    }
}

fn extract_simple_text<F>(node: &Node, mut setter: F)
where
    F: FnMut(String),
{
    if let Some(text) = node.text() {
        let t = text.trim();
        if !t.is_empty() {
            setter(t.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_xmp_parse() {
        let xmp_data = r#"
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                     xmlns:dc="http://purl.org/dc/elements/1.1/">
              <rdf:Description rdf:about="">
                <dc:title>
                  <rdf:Alt>
                    <rdf:li xml:lang="x-default">Test Title</rdf:li>
                  </rdf:Alt>
                </dc:title>
                <dc:creator>
                  <rdf:Seq>
                    <rdf:li>Author One</rdf:li>
                    <rdf:li>Author Two</rdf:li>
                  </rdf:Seq>
                </dc:creator>
                <dc:publisher>Test Publisher</dc:publisher>
              </rdf:Description>
            </rdf:RDF>
        "#;

        let mi = metadata_from_xmp_packet(xmp_data.as_bytes()).unwrap();
        // eprintln!("Parsed MI: {:?}", mi);
        assert_eq!(mi.title, "Test Title");
        assert_eq!(mi.authors.len(), 2);
        assert_eq!(mi.authors[0], "Author One");
        assert_eq!(mi.publisher, Some("Test Publisher".to_string()));
    }
}
