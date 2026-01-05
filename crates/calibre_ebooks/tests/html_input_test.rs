use calibre_ebooks::input::html_input::HTMLInput;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_html_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("source");
    let output_dir = tmp_dir.path().join("output");
    
    fs::create_dir(&input_dir).unwrap();

    // Create Root File
    let root_html = r#"
    <html>
        <head><title>Root File</title></head>
        <body>
            <h1>Welcome</h1>
            <a href="chapter1.html">Chapter 1</a>
        </body>
    </html>
    "#;
    let root_path = input_dir.join("index.html");
    fs::write(&root_path, root_html).unwrap();

    // Create Child File
    let chap1_html = r#"
    <html>
        <head><title>Chapter 1</title></head>
        <body>
            <h1>Chapter 1</h1>
            <p>Content.</p>
        </body>
    </html>
    "#;
    let chap1_path = input_dir.join("chapter1.html");
    fs::write(&chap1_path, chap1_html).unwrap();

    // Run Conversion
    let plugin = HTMLInput::new();
    let book = plugin.convert(&root_path, &output_dir).expect("Conversion failed");

    // Verify
    assert_eq!(book.manifest.items.len(), 2, "Should find 2 HTML files");
    assert_eq!(book.spine.items.len(), 2, "Should have 2 spine items");
    
    // Check output files exist
    assert!(output_dir.join(book.manifest.items["item_0"].href.clone()).exists());
    assert!(output_dir.join(book.manifest.items["item_1"].href.clone()).exists());
}
