use calibre_ebooks::input::txt_input::TXTInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_markdown_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("test.md");
    let output_dir = tmp_dir.path().join("output");

    // Create Markdown File
    let md_content = r#"
# Heading 1

**Bold Text**

* Item 1
* Item 2
    "#;
    fs::write(&input_path, md_content).unwrap();

    // Run Conversion
    let plugin = TXTInput::new();
    let book = plugin
        .convert(&input_path, &output_dir)
        .expect("Conversion failed");

    // Verify
    assert_eq!(book.manifest.items.len(), 1, "Should have 1 HTML file");

    // Read extracted file content
    let href = &book.manifest.items["item_0"].href;
    let html_path = output_dir.join(href);
    let html = fs::read_to_string(html_path).unwrap();

    // Check Markup
    assert!(html.contains("<h1>Heading 1</h1>"));
    assert!(html.contains("<strong>Bold Text</strong>") || html.contains("<b>Bold Text</b>"));
    assert!(html.contains("<ul>"));
    assert!(html.contains("<li>Item 1</li>"));
}

#[test]
fn test_plain_text_conversion() {
    let tmp_dir = tempdir().unwrap();
    // Use .txt extension so it falls back to plain text logic?
    // Wait, my logic checks content for # or ** too.
    // Let's make it mundane.
    let input_path = tmp_dir.path().join("test.txt");
    let output_dir = tmp_dir.path().join("output_txt");

    let txt_content = "Just some plain text.\nNew line here.";
    fs::write(&input_path, txt_content).unwrap();

    let plugin = TXTInput::new();
    let book = plugin
        .convert(&input_path, &output_dir)
        .expect("Conversion failed");

    let href = &book.manifest.items["item_0"].href;
    let html_path = output_dir.join(href);
    let html = fs::read_to_string(html_path).unwrap();

    // Expect pre tag for plain text fallback (if logic works)
    // Actually my logic defaults to markdown if ext is txt AND content has # or **.
    // Here logic might default to markdown parser ANYWAY if I implemented it that way?
    // "else { Plain text }" logic depends on `is_markdown`.
    // is_markdown = extension check AND content check.
    // "md" -> true. "txt" -> true IF content check.
    // Here content is simple. So is_markdown should be false.

    assert!(html.contains("<pre>"));
    assert!(html.contains("New line here."));
}
