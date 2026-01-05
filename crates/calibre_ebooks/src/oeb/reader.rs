use crate::oeb::book::OEBBook;
use crate::oeb::constants::*;
use anyhow::{bail, Result};
use roxmltree::Document;

pub struct OEBReader;

impl OEBReader {
    pub fn new() -> Self {
        OEBReader
    }

    pub fn read_opf(&self, book: &mut OEBBook, opf_path: &str) -> Result<()> {
        let data = book.container.read(opf_path)?;
        let text = String::from_utf8_lossy(&data); // Basic UTF-8 handling for now

        // roxmltree parses namespaces automatically.
        let doc = Document::parse(&text).map_err(|e| anyhow::anyhow!("XML Parse Error: {}", e))?;
        let root = doc.root_element();

        // Check root element local name
        if root.tag_name().name() != "package" {
            bail!("Root element is not package");
        }

        self.metadata_from_opf(book, &root)?;
        self.manifest_from_opf(book, &root)?;
        self.spine_from_opf(book, &root)?;
        self.guide_from_opf(book, &root)?;

        Ok(())
    }

    fn metadata_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        let metadata_nodes: Vec<_> = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "metadata")
            .collect();

        if let Some(metadata_node) = metadata_nodes.first() {
            for child in metadata_node.children().filter(|n| n.is_element()) {
                let tag_name = child.tag_name().name();
                let ns = child.tag_name().namespace().unwrap_or("");
                let text = child.text().unwrap_or("").trim();

                // Dublin Core check: Namespace is DC11_NS or tag starts with dc: (if ns parsing failed or different)
                // roxmltree handles standard namespaces well.
                if ns == DC11_NS {
                    book.metadata.add(tag_name, text);
                } else if tag_name == "meta" {
                    // Handle <meta name="..." content="...">
                    if let Some(name) = child.attribute("name") {
                        if let Some(content) = child.attribute("content") {
                            book.metadata.add(name, content);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn manifest_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        if let Some(manifest_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "manifest")
        {
            for child in manifest_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "item")
            {
                let id = child.attribute("id");
                let href = child.attribute("href");
                let media_type = child.attribute("media-type");

                if let (Some(id), Some(href), Some(media_type)) = (id, href, media_type) {
                    book.manifest.add(id, href, media_type);
                }
            }
        }
        Ok(())
    }

    fn spine_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        if let Some(spine_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "spine")
        {
            for child in spine_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "itemref")
            {
                if let Some(idref) = child.attribute("idref") {
                    let linear = child.attribute("linear").unwrap_or("yes") != "no";
                    book.spine.add(idref, linear);
                }
            }
        }
        Ok(())
    }

    fn guide_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        if let Some(guide_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "guide")
        {
            for child in guide_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "reference")
            {
                let type_ = child.attribute("type");
                let href = child.attribute("href");
                let title = child.attribute("title").map(|s| s.to_string());

                if let (Some(type_), Some(href)) = (type_, href) {
                    book.guide.add(type_, title, href);
                }
            }
        }
        Ok(())
    }
}
