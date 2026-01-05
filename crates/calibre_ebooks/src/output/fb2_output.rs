use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use base64::Engine;
use html_escape::encode_text;
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct FB2Output;

impl FB2Output {
    pub fn new() -> Self {
        FB2Output
    }

    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        let mut file = File::create(output_path).context("Failed to create output FB2 file")?;

        writeln!(file, "<?xml version=\"1.0\" encoding=\"utf-8\"?>")?;
        writeln!(file, "<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" xmlns:l=\"http://www.w3.org/1999/xlink\">")?;

        // 1. Description (Metadata)
        writeln!(file, "  <description>")?;
        writeln!(file, "    <title-info>")?;

        // Title
        let title = book
            .metadata
            .items
            .iter()
            .find(|i| i.term == "title")
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        writeln!(
            file,
            "      <book-title>{}</book-title>",
            encode_text(&title)
        )?;

        // Language
        let lang = book
            .metadata
            .items
            .iter()
            .find(|i| i.term == "language")
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "en".to_string());
        writeln!(file, "      <lang>{}</lang>", encode_text(&lang))?;

        // Author (first one found)
        if let Some(creator) = book.metadata.items.iter().find(|i| i.term == "creator") {
            writeln!(
                file,
                "      <author><first-name>{}</first-name></author>",
                encode_text(&creator.value)
            )?;
        }

        writeln!(file, "    </title-info>")?;
        writeln!(file, "  </description>")?;

        // 2. Body
        writeln!(file, "  <body>")?;

        // Iterate spine
        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    let html_content = String::from_utf8_lossy(&data);

                    // Simple HTML to FB2 conversion
                    // Ideally we parse the HTML structure.
                    // For this iteration, we'll wrap paragraphs in <p> assuming simple text,
                    // or strip tags for safety if complex.
                    // A robust solution needs an HTML parser.
                    // Here we will use a quick hack: Remove XML header if present, and try to extract body content.

                    let content = self.extract_body_content(&html_content);
                    // Just wrap raw content in a section for safety?
                    // FB2 requires Valid XML inside. Inserting HTML tags might break it if they aren't FB2 tags.
                    // FB2 tags: section, title, p, image, empty-line.
                    // If we dump HTML <p> it works. <div> might not.
                    // Let's assume the helper cleans it or we sanitize.
                    // For now: Wrap in section. Replace <img> with <image>.

                    writeln!(file, "    <section>")?;
                    let fb2_markup = self.convert_html_to_fb2(&content);
                    writeln!(file, "{}", fb2_markup)?;
                    writeln!(file, "    </section>")?;
                }
            }
        }

        writeln!(file, "  </body>")?;

        // 3. Binaries
        for item in book.manifest.items.values() {
            // Check if image
            if item.media_type.starts_with("image/") {
                if let Ok(data) = book.container.read(&item.href) {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    // ID should match what's used in <image l:href="#...">
                    // In convert_html_to_fb2, we need to ensure we use the same ID.
                    // Usually item.id or item.href (cleaned).
                    // Let's use item.href as ID (basename).
                    let id = Path::new(&item.href).file_name().unwrap().to_string_lossy();

                    writeln!(
                        file,
                        "  <binary id=\"{}\" content-type=\"{}\">{}</binary>",
                        encode_text(&id),
                        item.media_type,
                        b64
                    )?;
                }
            }
        }

        writeln!(file, "</FictionBook>")?;
        Ok(())
    }

    fn extract_body_content(&self, html: &str) -> String {
        // Very naive extraction: find <body>...</body>
        if let Some(start) = html.find("<body") {
            if let Some(open_end) = html[start..].find('>') {
                let body_start = start + open_end + 1;
                if let Some(end) = html[body_start..].find("</body>") {
                    return html[body_start..body_start + end].to_string();
                }
            }
        }
        html.to_string() // Fallback: return all
    }

    fn convert_html_to_fb2(&self, html: &str) -> String {
        // Basic replacements
        // 1. <img> to <image>
        // <img src="foo.jpg" /> -> <image l:href="#foo.jpg"/>
        // Valid for simple cases.
        // Also remove unsupported tags?
        // FB2 parser is strict.
        // For this batch, implementing basic <img> support.

        // Regex for img
        // regex crate is available
        use regex::Regex;
        // lazy_static?
        // Just compile for now.

        let img_re = Regex::new(r#"<img\s+[^>]*src="([^"]+)"[^>]*>"#).unwrap();
        // Replace with <image l:href="#$1"/>
        // Note: $1 must be just filename if we used filenames in binaries.
        // If src="images/foo.jpg", binary id should handle that.
        // In process_binary loop above, I used file_name().

        let result = img_re.replace_all(html, |caps: &regex::Captures| {
            let src = &caps[1];
            let name = Path::new(src)
                .file_name()
                .unwrap_or(std::ffi::OsStr::new("unknown"))
                .to_string_lossy();
            format!("<image l:href=\"#{}\"/>", name)
        });

        // Replace <br> with <empty-line/>
        let br_re = Regex::new(r"<br\s*/?>").unwrap();
        let result = br_re.replace_all(&result, "<empty-line/>");

        // Strip <div>, <span>? FB2 doesn't like them.
        // Ideally we strip tags but keep content.
        // Or blindly map div -> p?
        // Let's replace div with p.
        let result = result.replace("<div>", "<p>").replace("</div>", "</p>");

        result.to_string()
    }
}
