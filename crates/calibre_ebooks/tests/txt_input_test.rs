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

    // Two blank lines apart so paragraph-type auto-detection (issue
    // #537) doesn't collapse them into a single block/single-line
    // paragraph the way "single spaced" prose would.
    let txt_content = "Just some plain text.\n\n\nNew line here.";
    fs::write(&input_path, txt_content).unwrap();

    let plugin = TXTInput::new();
    let book = plugin
        .convert(&input_path, &output_dir)
        .expect("Conversion failed");

    let href = &book.manifest.items["item_0"].href;
    let html_path = output_dir.join(href);
    let html = fs::read_to_string(html_path).unwrap();

    // Real upstream's own `convert_basic` (used here for plain,
    // non-Markdown/Textile text, issue #537) wraps each real
    // paragraph in `<p>`, not `<pre>`.
    assert!(html.contains("<p>Just some plain text.</p>"));
    assert!(html.contains("<p>New line here.</p>"));
}
