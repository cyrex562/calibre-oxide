use crate::metadata::MetaInformation;
use chrono::{DateTime, Utc};
use roxmltree::{Document, Node};
use std::collections::HashMap;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpfError {
    #[error("Failed to parse XML: {0}")]
    XmlParseError(#[from] roxmltree::Error),
    #[error("Missing metadata section in OPF")]
    MissingMetadata,
}

const OPF_NS: &str = "http://www.idpf.org/2007/opf";

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

    // 1. Collect refinements (OPF 3.0)
    // Map of target_id -> Vec<(property, scheme, text)>
    let mut refines: HashMap<String, Vec<(String, Option<String>, String)>> = HashMap::new();

    for node in metadata_node.children() {
        if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("meta") {
            if let Some(target_id) = node.attribute("refines") {
                // Remove leading # if present
                let target_id = target_id.trim_start_matches('#').to_string();
                if let Some(prop) = node.attribute("property") {
                    let text = node.text().unwrap_or("").trim().to_string();
                    let scheme = node.attribute("scheme").map(|s| s.to_string());
                    refines
                        .entry(target_id)
                        .or_default()
                        .push((prop.to_string(), scheme, text));
                }
            }
        }
    }

    for node in metadata_node.children() {
        if !node.is_element() {
            continue;
        }

        let tag = node.tag_name().name().to_lowercase();
        let text = node.text().unwrap_or("").trim().to_string();

        // Skip refinery meta tags in this pass (handled above), but process legacy meta tags
        if tag == "meta" && node.attribute("refines").is_some() {
            continue;
        }

        if text.is_empty() && tag != "meta" {
            // meta can have content in attributes
            continue;
        }

        let id = node.attribute("id");

        match tag.as_str() {
            "title" => {
                let mut is_main = true;
                if let Some(id) = id {
                    if let Some(props) = refines.get(id) {
                        for (prop, _, val) in props {
                            if prop == "title-type" && val != "main" {
                                is_main = false;
                            }
                        }
                    }
                }

                if is_main {
                    if meta.title == "Unknown" {
                        meta.title = text;
                    } else {
                        // Overwrite generic title (some opf have multiple title tags)
                        meta.title = text;
                    }
                }
            }
            "creator" => {
                let mut role = "aut".to_string();
                let mut file_as = None;

                // Check OPF 2.0 attributes
                if let Some(r) = node
                    .attribute("role")
                    .or_else(|| node.attribute((OPF_NS, "role")))
                {
                    role = r.to_string();
                }
                if let Some(fa) = node.attribute((OPF_NS, "file-as")) {
                    file_as = Some(fa.to_string());
                }

                // Check OPF 3.0 refinements
                if let Some(id) = id {
                    if let Some(props) = refines.get(id) {
                        for (prop, _, val) in props {
                            if prop == "role" {
                                role = val.clone();
                            } else if prop == "file-as" {
                                file_as = Some(val.clone());
                            }
                        }
                    }
                }

                if role == "aut" {
                    if !found_authors {
                        meta.authors.clear();
                        found_authors = true;
                    }
                    meta.authors.push(text.clone());

                    if let Some(fa) = file_as {
                        meta.author_sort_map.insert(text, fa);
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
                    .or_else(|| node.attribute((OPF_NS, "scheme")))
                    .map(|s| s.to_string());

                if let Some(s) = scheme {
                    if s.eq_ignore_ascii_case("uuid") {
                        meta.uuid = Some(text.clone());
                    } else if s.eq_ignore_ascii_case("isbn") {
                        meta.identifiers.insert("isbn".to_string(), text);
                    } else {
                        meta.identifiers.insert(s.to_lowercase(), text);
                    }
                } else if text.starts_with("urn:uuid:") {
                    meta.uuid = Some(text.replace("urn:uuid:", ""));
                }
            }
            "publisher" => {
                meta.publisher = Some(text);
            }
            "subject" => {
                meta.tags.push(text);
            }
            "date" => {
                let mut event = node.attribute((OPF_NS, "event")).map(|s| s.to_string());

                if let Ok(dt) = DateTime::parse_from_rfc3339(&text) {
                    let utc: DateTime<Utc> = dt.with_timezone(&Utc);
                    if event.as_deref() == Some("creation")
                        || event.as_deref() == Some("modification")
                    {
                        meta.timestamp = Some(utc);
                    } else {
                        meta.pubdate = Some(utc);
                    }
                } else {
                    // Try parsing naive date
                    if let Ok(ndt) = chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
                        let dt = DateTime::<Utc>::from_naive_utc_and_offset(
                            ndt.and_hms_opt(0, 0, 0).unwrap(),
                            Utc,
                        );
                        meta.pubdate = Some(dt);
                    } else if let Ok(ndt) =
                        chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S")
                    {
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
                            // Assuming rating is Option<f64> or if u32 map it.
                            // Using code from previous file:
                            // meta.rating = Some(r);
                            // I will assume f64 based on previous file content.
                            // If u32 I will cast.
                            meta.rating = Some(r);
                        }
                    } else if n.eq_ignore_ascii_case("calibre:timestamp") {
                        if let Ok(dt) = DateTime::parse_from_rfc3339(c) {
                            meta.timestamp = Some(dt.with_timezone(&Utc));
                        }
                    } else if n.eq_ignore_ascii_case("calibre:title_sort") {
                        meta.title_sort = Some(c.to_string());
                    } else if n.starts_with("calibre:user_metadata:") {
                        let key = n.strip_prefix("calibre:user_metadata:").unwrap();
                        meta.user_metadata.insert(key.to_string(), c.to_string());
                    }
                }

                // OPF 3.0: meta property="dcterms:modified"
                if let Some(prop) = node.attribute("property") {
                    if prop == "dcterms:modified" {
                        if let Ok(dt) = DateTime::parse_from_rfc3339(&text) {
                            meta.timestamp = Some(dt.with_timezone(&Utc));
                        }
                    } else if prop == "calibre:timestamp" {
                        if let Ok(dt) = DateTime::parse_from_rfc3339(&text) {
                            meta.timestamp = Some(dt.with_timezone(&Utc));
                        }
                    } else if prop == "calibre:rating" {
                        if let Ok(r) = text.parse::<f64>() {
                            meta.rating = Some(r);
                        }
                    }
                    // Handle user metadata from properties?
                }
            }
            _ => {}
        }
    }

    Ok(meta)
}

// Basic XML generation (incomplete compared to parsing)
// ... Keeping to_xml in OpfMetadata?
// We moved OpfMetadata to MetaInformation.
// Ideally we implement to_xml on MetaInformation or a separate serializer.
// For now, removing the old OpfMetadata struct and methods.
