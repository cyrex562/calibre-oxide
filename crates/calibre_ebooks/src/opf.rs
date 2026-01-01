use crate::metadata::MetaInformation;
use chrono::{DateTime, Utc};
use roxmltree::{Document, Node};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpfError {
    #[error("Failed to parse XML: {0}")]
    XmlParseError(#[from] roxmltree::Error),
    #[error("Missing metadata section in OPF")]
    MissingMetadata,
}

pub fn parse_opf(xml_content: &str) -> Result<MetaInformation, OpfError> {
    let doc = Document::parse(xml_content)?;
    let root = doc.root_element();

    let metadata_node = root
        .children()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("metadata"))
        .ok_or(OpfError::MissingMetadata)?;

    let mut meta = MetaInformation::default();

    // Clear default "Unknown" author if we find creators
    let mut found_authors = false;

    for node in metadata_node.children() {
        if !node.is_element() {
            continue;
        }

        let tag = node.tag_name().name().to_lowercase();
        let text = node.text().unwrap_or("").trim().to_string();
        if text.is_empty() && tag != "meta" { // meta can have content in attributes
            continue;
        }

        match tag.as_str() {
            "title" => {
                meta.title = text;
            }
            "creator" => {
                let is_aut = node
                    .attribute("role")
                    .or_else(|| node.attribute((ns_opf(node), "role")))
                    .map(|r| r == "aut")
                    .unwrap_or(true); // Default role is author if unspecified? Verify standard.

                if is_aut {
                    if !found_authors {
                         meta.authors.clear();
                         found_authors = true;
                    }
                    meta.authors.push(text.clone());
                    
                    if let Some(file_as) = node.attribute((ns_opf(node), "file-as")) {
                        meta.author_sort_map.insert(text, file_as.to_string());
                    }
                }
            }
            "description" => {
                meta.comments = Some(text);
            }
            "language" => {
                 if meta.languages.len() == 1 && meta.languages[0] == "und" {
                     meta.languages.clear();
                 }
                 meta.languages.push(text);
            }
            "identifier" => {
                let scheme = node
                    .attribute("scheme")
                    .or_else(|| node.attribute((ns_opf(node), "scheme")));
                if let Some(s) = scheme {
                    if s.eq_ignore_ascii_case("uuid") {
                         meta.uuid = Some(text.clone());
                    } else if s.eq_ignore_ascii_case("isbn") {
                        meta.identifiers.insert("isbn".to_string(), text);
                    } else {
                        meta.identifiers.insert(s.to_lowercase(), text);
                    }
                }
            }
            "publisher" => {
                meta.publisher = Some(text);
            }
            "subject" => {
                meta.tags.push(text);
            }
            "date" => {
                // Handling publication date vs timestamp (creation date)
                // Often <dc:date opf:event="publication">...
                let event = node.attribute((ns_opf(node), "event"));
                if let Ok(dt) = DateTime::parse_from_rfc3339(&text) {
                     let utc: DateTime<Utc> = dt.with_timezone(&Utc);
                     if event == Some("creation") || event == Some("modification") {
                         meta.timestamp = Some(utc);
                     } else {
                         meta.pubdate = Some(utc); 
                     }
                } else {
                    // Try parsing naive date
                    if let Ok(ndt) = chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
                         let dt = DateTime::<Utc>::from_naive_utc_and_offset(ndt.and_hms_opt(0,0,0).unwrap(), Utc);
                         meta.pubdate = Some(dt);
                    }
                    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S") {
                        let dt = DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc);
                        meta.pubdate = Some(dt);
                    }
                }
            }
            "meta" => {
                let name = node.attribute("name");
                let content = node.attribute("content");
                
                if let (Some(n), Some(c)) = (name, content) {
                     if n.eq_ignore_ascii_case("cover") {
                         meta.cover_id = Some(c.to_string());
                     } else if n.eq_ignore_ascii_case("calibre:series") {
                         meta.series = Some(c.to_string());
                     } else if n.eq_ignore_ascii_case("calibre:series_index") {
                         if let Ok(idx) = c.parse::<f64>() {
                             meta.series_index = idx;
                         }
                     } else if n.eq_ignore_ascii_case("calibre:rating") {
                         if let Ok(r) = c.parse::<f64>() {
                             meta.rating = Some(r);
                         }
                     } else if n.eq_ignore_ascii_case("calibre:timestamp") {
                          if let Ok(dt) = DateTime::parse_from_rfc3339(c) {
                             meta.timestamp = Some(dt.with_timezone(&Utc));
                         }
                     } else if n.eq_ignore_ascii_case("calibre:title_sort") {
                         meta.title_sort = Some(c.to_string());
                     } else if n.eq_ignore_ascii_case("calibre:author_link_map") {
                        // skip complex json for now
                     } else if n.starts_with("calibre:user_metadata:") {
                         let key = n.strip_prefix("calibre:user_metadata:").unwrap();
                         meta.user_metadata.insert(key.to_string(), c.to_string());
                     }
                }
            }
            _ => {}
        }
    }

    Ok(meta)
}

fn ns_opf<'a, 'input>(node: Node<'a, 'input>) -> &'a str {
    node.lookup_prefix("http://www.idpf.org/2007/opf")
        .unwrap_or("opf")
}

// Basic XML generation (incomplete compared to parsing)
// ... Keeping to_xml in OpfMetadata? 
// We moved OpfMetadata to MetaInformation. 
// Ideally we implement to_xml on MetaInformation or a separate serializer.
// For now, removing the old OpfMetadata struct and methods.
