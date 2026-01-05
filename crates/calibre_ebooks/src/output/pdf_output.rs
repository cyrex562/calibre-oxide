use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use calibre_utils::html2text::html2text;
use lopdf::{Dictionary, Document, Object, Stream};
use std::collections::BTreeMap;
use std::path::Path;

pub struct PDFOutput;

impl PDFOutput {
    pub fn new() -> Self {
        PDFOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let mut doc = Document::with_version("1.4");

        let pages_id = doc.new_object_id();
        let font_id = doc.new_object_id();
        let content_id = doc.new_object_id();
        let page_id = doc.new_object_id(); // Single page for now? Or split?

        // 1. Gather Content logic
        // For simplicity, we concatenate all text content into one big page stream.
        // A real implementation needs pagination logic (complex).
        let mut combined_text = String::new();
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let html = String::from_utf8_lossy(&data);
                    let text = html2text(&html);
                    combined_text.push_str(&text);
                    combined_text.push_str("\n\n");
                }
            }
        }

        // Sanitize for PDF text (escape parens and backslashes)
        let pdf_text = combined_text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");

        // Create Content Stream
        // Basic BT /F1 12 Tf ... ET
        // We need to split lines explicitly for PDF not to run off page
        let mut stream_content = String::new();
        stream_content.push_str("BT\n/F1 12 Tf\n100 700 Td\n14 TL\n"); // Start text, Font, Position (Top-Left ish), Leading

        for line in pdf_text.lines() {
            // Very basic line wrapping? No, just clip.
            stream_content.push_str(&format!("({}) Tj\nT*\n", line));
        }
        stream_content.push_str("ET");

        let content_stream = Stream::new(Dictionary::new(), stream_content.as_bytes().to_vec());
        doc.objects
            .insert(content_id, Object::Stream(content_stream));

        // Define Font
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert(font_id, Object::Dictionary(font));

        // Define Page
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set(
            "MediaBox",
            Object::Array(vec![0.into(), 0.into(), 595.into(), 842.into()]),
        ); // A4
        page.set("Contents", Object::Reference(content_id));

        let mut resources = Dictionary::new();
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Reference(font_id));
        resources.set("Font", Object::Dictionary(fonts));
        page.set("Resources", Object::Dictionary(resources));

        doc.objects.insert(page_id, Object::Dictionary(page));

        // Define Pages Root
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        // Define Catalog (Root)
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference(catalog_id));

        // Save
        doc.save(output_path).context("Failed to save PDF")?;

        Ok(())
    }
}
