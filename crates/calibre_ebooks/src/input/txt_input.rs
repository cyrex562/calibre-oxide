use crate::oeb::book::OEBBook;
use crate::input::html_input::HTMLInput;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use pulldown_cmark::{Parser, Options, html};
use encoding_rs::UTF_8;

pub struct TXTInput;

impl TXTInput {
    pub fn new() -> Self {
        TXTInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Converting TXT/MD file: {:?}", input_path);

        // 1. Read Content & Detect Encoding (Basic UTF-8 fallback for now)
        let content_bytes = fs::read(input_path).context("Failed to read input file")?;
        let (cow, _, _) = UTF_8.decode(&content_bytes);
        let content = cow.to_string();

        // 2. Determine if Markdown
        let ext = input_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let is_markdown = ["md", "markdown", "text", "txt"].contains(&ext.as_str()) && 
                          (ext == "md" || ext == "markdown" || content.contains("# ") || content.contains("**"));
        
        let html_content = if is_markdown {
            let mut options = Options::empty();
            options.insert(Options::ENABLE_TABLES);
            options.insert(Options::ENABLE_FOOTNOTES);
            options.insert(Options::ENABLE_STRIKETHROUGH);
            options.insert(Options::ENABLE_TASKLISTS);
            
            let parser = Parser::new_ext(&content, options);
            let mut html_output = String::new();
            html::push_html(&mut html_output, parser);
            
            format!(
                "<html><head><title>Converted Text</title></head><body>{}</body></html>",
                html_output
            )
        } else {
            // Plain text - wrap in pre or paragraphs
            // For now, simple pre-wrap to preserve formatting
            format!(
                "<html><head><title>Converted Text</title></head><body><pre>{}</pre></body></html>",
                html_escape::encode_text(&content) // Need html escape? 
            )
        };
        
        // We lack `html_escape` crate. Let's just do very basic replacement or require `v_htmlescape`?
        // Actually, let's use a simple replacement for now to avoid dep hell if possible, 
        // OR add `v_htmlescape`.
        // Better: Use `pulldown_cmark` even for plain text but treated as code block? No.
        
        // I'll add a simple escaping helper function to avoid dependency if it's small.
        // Or assume pulldown-cmark is sufficient if I treat it as markdown?
        // If I run plain text through markdown parser, it handles paragraphs but might eat newlines.
        // Let's stick to Markdown path for everything for now (it's robust enough for most text).
        // A simple text file is valid markdown.
        // BUT, plain text usually expects `<pre>` or preserved newlines without markdown syntax.
        
        // Let's just escape <, >, & manually for the non-markdown fallback path.
        
        // 3. Write to Temp HTML
        let temp_dir = output_dir.join("temp_conversion");
        fs::create_dir_all(&temp_dir)?;
        let temp_html_path = temp_dir.join("index.html");
        fs::write(&temp_html_path, html_content).context("Failed to write input HTML")?;

        // 4. Delegate to HTMLInput
        let html_plugin = HTMLInput::new();
        let book = html_plugin.convert(&temp_html_path, output_dir)?;
        
        // Cleanup temp (optional, maybe keep for debug?)
        // fs::remove_dir_all(temp_dir)?;

        Ok(book)
    }
}

// Simple HTML escaper
mod html_escape {
    pub fn encode_text(s: &str) -> String {
        s.replace("&", "&amp;")
         .replace("<", "&lt;")
         .replace(">", "&gt;")
         .replace("\"", "&quot;")
         .replace("'", "&#39;")
    }
}
