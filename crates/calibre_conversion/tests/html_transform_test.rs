use anyhow::Result;
use calibre_conversion::oeb::{ManifestItem, OebBook};
use calibre_conversion::traits::{ConversionOptions, Transform};
use calibre_conversion::transform::html_roundtrip::HtmlRoundTrip;
use std::fs::File;
use std::io::{Read, Write};
use tempfile::Builder;

#[test]
fn test_html_roundtrip() -> Result<()> {
    // 1. Setup Temp File with HTML
    let temp_dir = Builder::new().prefix("html_test_").tempdir()?;
    let html_path = temp_dir.path().join("chapter1.html");

    let original_html = r#"<!DOCTYPE html>
<html><head><title>Test</title></head><body><h1>Hello World</h1><p>Some text.</p></body></html>"#;

    {
        let mut f = File::create(&html_path)?;
        f.write_all(original_html.as_bytes())?;
    }

    // 2. Setup OebBook
    let mut book = OebBook::new();
    book.manifest.insert(
        "item1".to_string(),
        ManifestItem {
            id: "item1".to_string(),
            href: "chapter1.html".to_string(),
            media_type: "application/xhtml+xml".to_string(),
            path: html_path.clone(),
        },
    );

    // 3. Run Transform
    let transform = HtmlRoundTrip;
    transform.process(&mut book, &ConversionOptions::default())?;

    // 4. Verify Output
    let mut output_content = String::new();
    File::open(&html_path)?.read_to_string(&mut output_content)?;

    println!("Output HTML: {}", output_content);

    // Check that it's still valid HTML and contains expected content
    assert!(output_content.contains("<html>"));
    assert!(output_content.contains("Hello World"));
    // Note: html5ever might reformat or add attributes, so exact match might be brittle.
    // But structurally it should be similar.

    Ok(())
}
