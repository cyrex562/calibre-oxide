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
                    let pml_chunk = html_to_pml(&content);
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

impl Default for PMLOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Basic (X)HTML -> PML markup converter.
///
/// This is a *narrow* stand-in for `calibre.ebooks.pml.pmlml`'s
/// `PMLMLizer` (a full OEB-walking converter that also tracks image
/// hrefs, page/chapter markers, and CSS-driven styling). Porting that
/// converter is tracked separately under `#### pml` in
/// `docs/modules_to_port.md` (`pmlml.py`, still unchecked as of this
/// writing) -- it isn't part of this module's own file list, so this
/// function only maps the handful of tags (`p`, `div`, `b`/`strong`,
/// headings, `i`/`em`, `br`) needed for a readable round trip.
///
/// Extracted to a free function (was a private `PMLOutput` method) so
/// `crate::pdb::ereader::writer` can reuse the same conversion instead
/// of duplicating it, per this crate's established "don't duplicate a
/// codec that already exists once" pattern.
pub fn html_to_pml(html: &str) -> String {
    let mut pml = String::new();

    // Normalize
    let content = html.replace("\r\n", "\n").replace('\r', "\n");

    // We can use `roxmltree` if we trust the input is XHTML (which OEB
    // content should be).
    if let Ok(doc) = roxmltree::Document::parse(&content) {
        visit_node(doc.root(), &mut pml);
    } else {
        // Fallback: parsing failed (loose HTML) -- just dump the raw
        // text rather than losing the content entirely.
        pml.push_str(html);
    }

    pml
}

fn visit_node(node: roxmltree::Node, out: &mut String) {
    if node.is_text() {
        if let Some(text) = node.text() {
            // Determine if we need to escape anything?
            // PML doesn't need much escaping usually, except maybe backslashes.
            // But \ is the control char.
            let safe_text = text.replace('\\', "\\\\");
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
        "br" => out.push('\n'),
        _ => {}
    }

    for child in node.children() {
        visit_node(child, out);
    }

    // Exit
    match tag {
        "p" | "div" => out.push('\n'),
        "b" | "strong" | "h1" | "h2" | "h3" => out.push_str("\\b"),
        "i" | "em" => out.push_str("\\i"),
        _ => {}
    }
}
