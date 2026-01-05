use crate::compression::palmdoc::decompress;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::pdb::reader::PdbReader;
use anyhow::{Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use html_escape::encode_text;
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

pub struct PMLInput;

impl PMLInput {
    pub fn new() -> Self {
        PMLInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let mut reader = PdbReader::new(input_path).context("Failed to open PDB file")?;

        // 1. Read Payload
        // Rec 0 likely contains Compression header if "TEXt" type.
        // Standard PalmDoc: Rec 0 is header?
        // Or Rec 0 is TEXT?
        // PalmDoc spec: Rec 0 is header. Rec 1..N is text.

        let mut text_content = Vec::new();

        // Check compression
        let mut compression = 1; // 1 = None, 2 = PalmDoc

        if reader.num_records() > 0 {
            let rec0 = reader.read_record(0)?;
            if rec0.len() >= 2 {
                let mut curs = Cursor::new(&rec0);
                compression = curs.read_u16::<BigEndian>()?;
            }
        }

        // Records 1..N-1 usually (Last record might be bookmarks/metadata?)
        // Standard PalmDoc: All records after 0 are text until ...?
        // We'll iterate 1..N.

        for i in 1..reader.num_records() {
            let data = reader.read_record(i)?;

            // Check if it's a valid text record or auxiliary (bookmarks, etc)?
            // Usually text records are roughly 4096 bytes.
            // Let's try to decompress/read.

            let chunk = if compression == 2 {
                decompress(&data)?
            } else {
                data
            };

            text_content.extend_from_slice(&chunk);
        }

        let pml_text = String::from_utf8_lossy(&text_content).to_string();

        // 2. Parse PML
        let html_body = self.pml_to_html(&pml_text);

        // 3. Create OEBBook
        let container = Box::new(DirContainer::new(output_dir.to_path_buf()));
        let mut book = OEBBook::new(container);

        // Metadata
        let title = reader.header.name.clone();
        book.metadata.add("title", &title);
        book.metadata.add("language", "en"); // Default

        // Content
        let page_filename = "index.html";
        let full_html = format!(
            "<html><head><title>{}</title></head><body>{}</body></html>",
            encode_text(&title),
            html_body
        );

        // Write to output dir
        fs::write(output_dir.join(page_filename), full_html)?;

        book.manifest
            .add("content", page_filename, "application/xhtml+xml");
        book.spine.add("content", true);

        Ok(book)
    }

    fn pml_to_html(&self, pml: &str) -> String {
        // Basic PML parser
        // Codes: \p (paragraph), \b (bold), \i (italic), \U (underline), \o (strikethrough)
        // \v (invisible), \t (indent), \n (newline - implied by \p?)
        // \x (image - complex), \a (anchor)
        // \w, \k (keywords/links)

        // We will do a simple pass.
        // Note: PML codes are often toggles or singletons?
        // \b is specific: "Toggle bold".
        // \i is "Toggle italic".

        let mut html = String::new();
        let mut chars = pml.chars().peekable();

        let mut in_bold = false;
        let mut in_italic = false;
        let mut in_underline = false;
        let mut in_strike = false;

        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(code) = chars.next() {
                    match code {
                        'p' => html.push_str("<p>"),
                        'b' => {
                            if in_bold {
                                html.push_str("</strong>");
                            } else {
                                html.push_str("<strong>");
                            }
                            in_bold = !in_bold;
                        }
                        'i' => {
                            if in_italic {
                                html.push_str("</em>");
                            } else {
                                html.push_str("<em>");
                            }
                            in_italic = !in_italic;
                        }
                        'U' => {
                            if in_underline {
                                html.push_str("</u>");
                            } else {
                                html.push_str("<u>");
                            }
                            in_underline = !in_underline;
                        }
                        'o' => {
                            if in_strike {
                                html.push_str("</s>");
                            } else {
                                html.push_str("<s>");
                            }
                            in_strike = !in_strike;
                        }
                        'n' => html.push_str("<br/>"),
                        '\\' => html.push('\\'), // Escaped backslash
                        _ => {
                            // Ignore unknown or handle specially
                            // E.g. \x1234 image?
                            // For now, ignore unknown codes
                        }
                    }
                }
            } else {
                // Escape HTML chars
                match c {
                    '<' => html.push_str("&lt;"),
                    '>' => html.push_str("&gt;"),
                    '&' => html.push_str("&amp;"),
                    _ => html.push(c),
                }
            }
        }

        // Close open tags
        if in_strike {
            html.push_str("</s>");
        }
        if in_underline {
            html.push_str("</u>");
        }
        if in_italic {
            html.push_str("</em>");
        }
        if in_bold {
            html.push_str("</strong>");
        }

        html
    }
}
