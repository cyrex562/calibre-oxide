use super::container::DOCX;
use super::error::DocxError;
use super::names::DOCXNamespaces;
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

pub struct DOCXToHTML;

impl DOCXToHTML {
    pub fn convert<R: Read + Seek>(
        docx: &mut DOCX<R>,
        dest_dir: &Path,
    ) -> Result<String, DocxError> {
        let doc_name = docx.document_name()?;

        // 1. Read Document Relationships
        // Construct path: word/_rels/document.xml.rels
        // Simple logic: assume doc_name has a parent dir
        let path_obj = Path::new(&doc_name);
        let file_name = path_obj.file_name().unwrap_or_default().to_string_lossy();
        let parent = path_obj.parent().unwrap_or(Path::new(""));
        let rels_path = parent.join("_rels").join(format!("{}.rels", file_name));
        let rels_path_str = rels_path.to_string_lossy().replace("\\", "/");

        let mut doc_rels = HashMap::new();
        if let Ok(content) = docx.read_file(&rels_path_str) {
            let text = String::from_utf8(content).unwrap_or_default();
            if let Ok(doc) = Document::parse(&text) {
                for node in doc.descendants() {
                    if node.has_tag_name("Relationship") {
                        let id = node.attribute("Id").unwrap_or_default().to_string();
                        let target = node.attribute("Target").unwrap_or_default().to_string();
                        doc_rels.insert(id, target);
                    }
                }
            }
        }

        // 2. Read Document Content
        let content = docx.read_file(&doc_name)?;
        let text = String::from_utf8(content).map_err(|e| DocxError::InvalidDocx(e.to_string()))?;
        let doc = Document::parse(&text)?;

        // 3. Generate HTML
        let mut html = String::from("<html><head><meta charset=\"utf-8\"/></head><body>");

        for node in doc.descendants() {
            if node.tag_name().name() == "p" {
                Self::process_paragraph(node, &mut html, &doc_rels, docx, dest_dir);
            }
        }

        html.push_str("</body></html>");
        Ok(html)
    }

    fn process_paragraph<R: Read + Seek>(
        node: Node,
        html: &mut String,
        rels: &HashMap<String, String>,
        docx: &mut DOCX<R>,
        dest_dir: &Path,
    ) {
        // Determine Tag (p, h1-h6) based on pPr/pStyle
        let mut tag = "p";

        for child in node.children() {
            if child.tag_name().name() == "pPr" {
                for p_prop in child.children() {
                    if p_prop.tag_name().name() == "pStyle" {
                        if let Some(val) = p_prop.attribute("val") {
                            match val {
                                "Heading1" => tag = "h1",
                                "Heading2" => tag = "h2",
                                "Heading3" => tag = "h3",
                                "Heading4" => tag = "h4",
                                "Heading5" => tag = "h5",
                                "Heading6" => tag = "h6",
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        html.push('<');
        html.push_str(tag);
        html.push('>');

        for child in node.children() {
            if child.tag_name().name() == "r" {
                Self::process_run(child, html, rels, docx, dest_dir);
            } else if child.tag_name().name() == "hyperlink" {
                // Handle hyperlink
                let rid = child.attribute((DOCXNamespaces::R, "id"));
                if let Some(rid) = rid {
                    if let Some(target) = rels.get(rid) {
                        html.push_str(&format!("<a href=\"{}\">", target));
                        for sub in child.children() {
                            if sub.tag_name().name() == "r" {
                                Self::process_run(sub, html, rels, docx, dest_dir);
                            }
                        }
                        html.push_str("</a>");
                    }
                }
            }
        }

        html.push_str(&format!("</{}>", tag));
    }

    fn process_run<R: Read + Seek>(
        node: Node,
        html: &mut String,
        rels: &HashMap<String, String>,
        docx: &mut DOCX<R>,
        dest_dir: &Path,
    ) {
        for child in node.children() {
            match child.tag_name().name() {
                "t" => {
                    if let Some(text) = child.text() {
                        html.push_str(&html_escape::encode_text(text));
                    }
                }
                "br" => html.push_str("<br/>"),
                "drawing" => {
                    // Extract image
                    // This is complex in OOXML. drawing -> inline -> graphic -> graphicData -> pic -> blipFill -> blip -> embed
                    // Or similar structure
                    for desc in child.descendants() {
                        if desc.tag_name().name() == "blip" {
                            if let Some(rid) = desc.attribute((DOCXNamespaces::R, "embed")) {
                                if let Some(target) = rels.get(rid) {
                                    // target is relative to document.xml usually, e.g. "media/image1.jpeg"
                                    // We need to resolve it relative to DOCX root (word/media/image1.jpeg)
                                    // Assuming document is at word/document.xml
                                    let image_path = Path::new("word")
                                        .join(target)
                                        .to_string_lossy()
                                        .replace("\\", "/");

                                    if let Ok(data) = docx.read_file(&image_path) {
                                        // Write to dest_dir
                                        let file_name =
                                            Path::new(target).file_name().unwrap_or_default();
                                        let dest_path = dest_dir.join(file_name);
                                        if std::fs::write(&dest_path, data).is_ok() {
                                            html.push_str(&format!(
                                                "<img src=\"{}\" />",
                                                file_name.to_string_lossy()
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub mod html_escape {
    pub fn encode_text(s: &str) -> String {
        s.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace("\"", "&quot;")
            .replace("'", "&#39;")
    }
}
