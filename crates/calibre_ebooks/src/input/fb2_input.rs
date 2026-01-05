use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::oeb::spine::SpineItem;
use anyhow::{Context, Result};
use base64::Engine;
use roxmltree::{Document, Node};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct FB2Input;

impl FB2Input {
    pub fn new() -> Self {
        FB2Input
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Converting FB2: {:?}", input_path);

        fs::create_dir_all(output_dir)?;

        let mut content = String::new();
        File::open(input_path)?.read_to_string(&mut content)?;

        let doc = Document::parse(&content)?;
        let root = doc.root_element();

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);
        let mut html = String::from("<html><head><meta charset=\"utf-8\"/></head><body>");

        // 1. Binaries (Images)
        for node in root.children() {
            if node.tag_name().name() == "binary" {
                self.process_binary(node, output_dir, &mut book)?;
            }
        }

        // 2. Metadata (description)
        if let Some(desc) = root
            .children()
            .find(|n| n.tag_name().name() == "description")
        {
            if let Some(title_info) = desc
                .children()
                .find(|n| n.tag_name().name() == "title-info")
            {
                for child in title_info.children() {
                    let tag = child.tag_name().name();
                    if tag == "book-title" {
                        if let Some(text) = child.text() {
                            book.metadata.add("title", text);
                        }
                    } else if tag == "lang" {
                        if let Some(text) = child.text() {
                            book.metadata.add("language", text);
                        }
                    }
                    // Author logic in FB2 is nested (first-name, last-name), skipping for brevity
                }
            }
        }

        // 3. Body (Content)
        // FB2 can have multiple bodies (one main, others notes).
        // For simplicity, convert all bodies sequentially.
        for node in root.children() {
            if node.tag_name().name() == "body" {
                self.process_body(node, &mut html)?;
            }
        }

        html.push_str("</body></html>");

        // Write index.html
        let index_path = output_dir.join("index.html");
        fs::write(&index_path, &html)?;

        book.manifest.items.insert(
            "index".to_string(),
            ManifestItem {
                id: "index".to_string(),
                href: "index.html".to_string(),
                media_type: "application/xhtml+xml".to_string(),
                fallback: None,
                linear: true,
            },
        );

        book.spine.items.push(SpineItem {
            idref: "index".to_string(),
            linear: true,
        });

        Ok(book)
    }

    fn process_binary(&self, node: Node, output_dir: &Path, book: &mut OEBBook) -> Result<()> {
        if let Some(id) = node.attribute("id") {
            let content_type = node
                .attribute("content-type")
                .unwrap_or("application/octet-stream");
            if let Some(text) = node.text() {
                // Remove whitespace/newlines from base64
                let clean_text: String = text.chars().filter(|c| !c.is_whitespace()).collect();

                let data = base64::engine::general_purpose::STANDARD
                    .decode(clean_text)
                    .context("Failed to decode base64 binary")?;

                let file_path = output_dir.join(id);
                fs::write(&file_path, data)?;

                book.manifest.items.insert(
                    id.to_string(),
                    ManifestItem {
                        id: id.to_string(),
                        href: id.to_string(),
                        media_type: content_type.to_string(),
                        fallback: None,
                        linear: false,
                    },
                );
            }
        }
        Ok(())
    }

    fn process_body(&self, node: Node, html: &mut String) -> Result<()> {
        html.push_str("<div class=\"body\">");
        for child in node.children() {
            self.process_element(child, html)?;
        }
        html.push_str("</div>");
        Ok(())
    }

    fn process_element(&self, node: Node, html: &mut String) -> Result<()> {
        if !node.is_element() {
            if let Some(text) = node.text() {
                html.push_str(&html_escape::encode_text(text));
            }
            return Ok(());
        }

        let tag = node.tag_name().name();
        match tag {
            "section" => {
                html.push_str("<div class=\"section\">");
                for child in node.children() {
                    self.process_element(child, html)?;
                }
                html.push_str("</div>");
            }
            "title" => {
                html.push_str("<h2>"); // Generic Heading
                for child in node.children() {
                    self.process_element(child, html)?;
                }
                html.push_str("</h2>");
            }
            "p" => {
                html.push_str("<p>");
                for child in node.children() {
                    self.process_element(child, html)?;
                }
                html.push_str("</p>");
            }
            "strong" | "b" => {
                html.push_str("<strong>");
                for child in node.children() {
                    self.process_element(child, html)?;
                }
                html.push_str("</strong>");
            }
            "emphasis" | "i" => {
                html.push_str("<em>");
                for child in node.children() {
                    self.process_element(child, html)?;
                }
                html.push_str("</em>");
            }
            "image" => {
                // href is usually #id
                // l:href or xlink:href
                // We check all attributes for something ending in href or just check typical namespaces
                // roxmltree handles namespaces.
                // The attribute is typically `{http://www.w3.org/1999/xlink}href`

                // Search for any attribute locally named "href"
                let href = node
                    .attributes()
                    .find(|a| a.name() == "href")
                    .map(|a| a.value());

                if let Some(val) = href {
                    let src = val.trim_start_matches('#');
                    html.push_str(&format!("<img src=\"{}\" />", src));
                }
            }
            "empty-line" => {
                html.push_str("<br/>");
            }
            _ => {
                // Fallback for unknown tags: process children
                for child in node.children() {
                    self.process_element(child, html)?;
                }
            }
        }
        Ok(())
    }
}
