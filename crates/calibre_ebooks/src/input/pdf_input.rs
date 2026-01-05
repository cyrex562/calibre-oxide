use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use lopdf::{Document, Object};
use std::fs;
use std::path::Path;

pub struct PDFInput;

impl PDFInput {
    pub fn new() -> Self {
        PDFInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let doc = Document::load(input_path).context("Failed to load PDF document")?;

        // 1. Extract Text Content (Naive)
        let mut text_content = String::new();
        text_content.push_str("<html><body>");

        // Iterate pages
        for (page_num, page_id) in doc.get_pages() {
            let text = self
                .extract_text_from_page(&doc, page_id)
                .unwrap_or_default();
            if !text.trim().is_empty() {
                text_content.push_str(&format!("<div id=\"page_{}\">", page_num));
                // Basic escaping
                let escaped = html_escape::encode_text(&text);
                // Split by newlines to preserve some structure
                for line in escaped.lines() {
                    text_content.push_str(&format!("<p>{}</p>", line));
                }
                text_content.push_str("</div><hr/>");
            }
        }
        text_content.push_str("</body></html>");

        fs::create_dir_all(output_dir)?;
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);
        fs::write(&content_path, &text_content)?;

        // Build OEBBook
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        let id = "content".to_string();
        let href = content_filename.to_string();

        book.manifest.items.insert(
            id.clone(),
            ManifestItem::new(&id, &href, "application/xhtml+xml"),
        );
        book.manifest.hrefs.insert(href.clone(), id.clone());
        book.spine.add(&id, true);

        // Metadata
        if let Ok(info) = crate::metadata::pdf::get_metadata(&mut fs::File::open(input_path)?) {
            if !info.title.is_empty() && info.title != "Unknown" {
                book.metadata.add("title", &info.title);
            }
            for author in info.authors {
                if author != "Unknown" {
                    book.metadata.add("creator", &author);
                }
            }
        }

        if book.metadata.get("title").is_empty() {
            book.metadata.add("title", "Converted PDF");
        }

        Ok(book)
    }

    fn extract_text_from_page(&self, doc: &Document, page_id: lopdf::ObjectId) -> Result<String> {
        let content_data = doc.get_page_content(page_id)?;
        let content = lopdf::content::Content::decode(&content_data)?;

        let mut text = String::new();

        // Very naive operation iteration
        for operation in content.operations {
            match operation.operator.as_ref() {
                "Tj" | "TJ" => {
                    // Text show operators
                    for operand in operation.operands {
                        if let Ok(s) = self.decode_text_object(&operand) {
                            text.push_str(&s);
                        }
                    }
                    text.push(' '); // Space between chunks
                }
                "ET" | "Td" | "TD" | "T*" => {
                    // End text or new line
                    text.push('\n');
                }
                _ => {}
            }
        }

        Ok(text)
    }

    fn decode_text_object(&self, obj: &Object) -> Result<String> {
        match obj {
            Object::String(bytes, _) => Ok(String::from_utf8_lossy(bytes).to_string()),
            Object::Array(arr) => {
                let mut s = String::new();
                for item in arr {
                    if let Ok(sub) = self.decode_text_object(item) {
                        s.push_str(&sub);
                    }
                }
                Ok(s)
            }
            _ => Ok(String::new()),
        }
    }
}
