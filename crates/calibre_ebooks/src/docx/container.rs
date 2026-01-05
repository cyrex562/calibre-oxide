use super::error::DocxError;
use super::names::DOCXNamespaces;
use crate::oeb::metadata::Metadata;
use roxmltree::Document;
use std::collections::HashMap;
use std::io::{Read, Seek};
use zip::ZipArchive; // Reusing OEB Metadata? Or need conversion?
                     // NOTE: container.py initializes a custom Metadata struct (book.base).
                     // Here we will extract info into a subset of Metadata or just return fields.
                     // For now, let's use OEBMetadata for convenience or create a struct.

pub struct DOCX<R: Read + Seek> {
    zip: ZipArchive<R>,
    pub content_types: HashMap<String, String>,
    pub default_content_types: HashMap<String, String>,
    pub relationships: HashMap<String, String>, // Type -> Target
}

impl<R: Read + Seek> DOCX<R> {
    pub fn new(reader: R) -> Result<Self, DocxError> {
        let mut zip = ZipArchive::new(reader)?;

        let mut docx = DOCX {
            zip,
            content_types: HashMap::new(),
            default_content_types: HashMap::new(),
            relationships: HashMap::new(),
        };

        docx.read_content_types()?;
        docx.read_package_relationships()?;

        Ok(docx)
    }

    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>, DocxError> {
        let mut file = self.zip.by_name(name)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    fn read_content_types(&mut self) -> Result<(), DocxError> {
        let content = self
            .read_file("[Content_Types].xml")
            .map_err(|_| DocxError::InvalidDocx("No [Content_Types].xml".into()))?;
        let text = String::from_utf8(content)
            .map_err(|_| DocxError::InvalidDocx("Invalid UTF-8 in Content_Types".into()))?;
        let doc = Document::parse(&text)?;

        for node in doc.descendants() {
            if node.has_tag_name("Default") {
                let ext = node.attribute("Extension").unwrap_or("").to_lowercase();
                let ct = node.attribute("ContentType").unwrap_or("").to_string();
                self.default_content_types.insert(ext, ct);
            } else if node.has_tag_name("Override") {
                let part_name = node
                    .attribute("PartName")
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .to_string();
                let ct = node.attribute("ContentType").unwrap_or("").to_string();
                self.content_types.insert(part_name, ct);
            }
        }
        Ok(())
    }

    fn read_package_relationships(&mut self) -> Result<(), DocxError> {
        // Read _rels/.rels
        let content = match self.read_file("_rels/.rels") {
            Ok(c) => c,
            Err(_) => return Ok(()), // Some simple docs might skip this? No, standard requires it.
        };
        let text = String::from_utf8(content)
            .map_err(|_| DocxError::InvalidDocx("Invalid UTF-8 in _rels/.rels".into()))?;
        let doc = Document::parse(&text)?;

        for node in doc.descendants() {
            if node.has_tag_name("Relationship") {
                let target = node
                    .attribute("Target")
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .to_string();
                let typ = node.attribute("Type").unwrap_or("").to_string();
                self.relationships.insert(typ, target);
            }
        }
        Ok(())
    }

    pub fn document_name(&self) -> Result<String, DocxError> {
        if let Some(target) = self.relationships.get(DOCXNamespaces::DOCUMENT) {
            Ok(target.clone())
        } else {
            // Fallback: search for word/document.xml
            let names: Vec<_> = self.zip.file_names().collect();
            if names.contains(&"word/document.xml") {
                Ok("word/document.xml".to_string())
            } else {
                Err(DocxError::InvalidDocx("No main document found".into()))
            }
        }
    }

    pub fn get_metadata(&mut self) -> Result<Metadata, DocxError> {
        let mut meta = Metadata::new();

        // 1. Core Props
        let core_props_path = self.relationships.get(DOCXNamespaces::CORE_PROPS).cloned();

        if let Some(core_props_path) = core_props_path {
            if let Ok(content) = self.read_file(&core_props_path) {
                let text = String::from_utf8_lossy(&content);
                // Parse simple DC elements
                if let Ok(doc) = Document::parse(&text) {
                    for node in doc.descendants() {
                        if node.tag_name().name() == "title" {
                            if let Some(t) = node.text() {
                                meta.add("title", t);
                            }
                        } else if node.tag_name().name() == "creator" {
                            if let Some(t) = node.text() {
                                meta.add("creator", t);
                            }
                        } else if node.tag_name().name() == "language" {
                            if let Some(t) = node.text() {
                                meta.add("language", t);
                            }
                        }
                    }
                }
            }
        }

        Ok(meta)
    }
}
