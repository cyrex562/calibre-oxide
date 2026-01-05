use crate::oeb::book::OEBBook;
use crate::pdb::writer::PdbWriter;
use anyhow::Result;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct PMLOutput;

impl PMLOutput {
    pub fn new() -> Self {
        PMLOutput
    }

    fn convert_html_to_pml(&self, html: &str) -> String {
        // Very basic PML converter
        // \p = paragraph
        // \b = bold
        // \i = italic

        // We use roxmltree for parsing if it were XML, but HTML can be messy.
        // For robustness, maybe just string replacement or regex for this batch?
        // Or better: use a proper HTML parser if we had one handy and robust.
        // `html2text` is available but produces text.

        // Let's use a simplified approach:
        // 1. naive tag replacement for known tags.
        // 2. strip unknown tags.

        let mut pml = String::new();

        // Normalize
        let content = html.replace("\r\n", "\n").replace("\r", "\n");

        // Simple state machine or just regex replacements?
        // Regex is fragile but efficient for "porting prototypes".
        // Let's iterate chars or use a crate if possible.
        // Actually, we can use `roxmltree` if we trust the input is XHTML (which OEB content should be).

        if let Ok(doc) = roxmltree::Document::parse(&content) {
            self.visit_node(doc.root(), &mut pml);
        } else {
            // Fallback: Just text? Or Regex?
            // If parsing fails (loose HTML), let's just dump text for now.
            pml.push_str(&html); // This is bad, implies raw HTML.
                                 // Maybe remove tags?
        }

        pml
    }

    fn visit_node(&self, node: roxmltree::Node, out: &mut String) {
        if node.is_text() {
            if let Some(text) = node.text() {
                // Determine if we need to escape anything?
                // PML doesn't need much escaping usually, except maybe backslashes.
                // But \ is the control char.
                let safe_text = text.replace("\\", "\\\\");
                out.push_str(&safe_text);
            }
            return;
        }

        let tag = node.tag_name().name();

        // Enter
        match tag {
            "p" | "div" => out.push_str("\\p"),
            "b" | "strong" | "h1" | "h2" | "h3" => out.push_str("\\b"),
            "i" | "em" => out.push_str("\\i"),
            "br" => out.push_str("\n"),
            _ => {}
        }

        for child in node.children() {
            self.visit_node(child, out);
        }

        // Exit
        match tag {
            "p" | "div" => out.push_str("\n"),
            "b" | "strong" | "h1" | "h2" | "h3" => out.push_str("\\b"),
            "i" | "em" => out.push_str("\\i"),
            _ => {}
        }
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let title = book
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or("Unknown".to_string());

        // Collect content
        let mut full_pml = String::new();

        // Header info
        // full_pml.push_str(&format!("\\v{}\n", title)); // \v = title in some PML versions? Or just text.
        // Actually standard PML doesn't have a specific title tag inside text, it's in PDB header.

        for item in &book.spine.items {
            if let Some(manifest_item) = book.manifest.get_by_id(&item.idref) {
                // Load content
                if let Ok(content_bytes) = book.container.read(&manifest_item.href) {
                    let content = String::from_utf8_lossy(&content_bytes);
                    let pml_chunk = self.convert_html_to_pml(&content);
                    full_pml.push_str(&pml_chunk);
                    full_pml.push_str("\\p"); // break between chapters
                }
            }
        }

        // Write PDB
        let file = File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        let pdb_writer = PdbWriter::new();

        // Encode as Latin1 (ISO-8859-1) or CP1252 usually for legacy Palm
        // For Rust, we might just write bytes?
        // `encoding_rs::WINDOWS_1252`
        let (cow, _encoding, _malformed) = encoding_rs::WINDOWS_1252.encode(&full_pml);
        let encoded_bytes = cow.to_vec();

        pdb_writer.write(&title, &encoded_bytes, &mut writer)?;

        Ok(())
    }
}
