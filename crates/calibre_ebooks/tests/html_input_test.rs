use calibre_ebooks::input::html_input::HTMLInput;
use std::fs;
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

    // Issue #536: the book's real title comes from the root file's
    // own `<title>` tag, not a hardcoded placeholder.
    let titles = book.metadata.get("title");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].value, "Root File");
}

#[test]
fn a_root_file_with_no_title_tag_falls_back_to_its_filename() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("source");
    let output_dir = tmp_dir.path().join("output");
    fs::create_dir(&input_dir).unwrap();

    let root_path = input_dir.join("untitled_chapter.html");
    fs::write(&root_path, "<html><body><p>No title tag here.</p></body></html>").unwrap();

    let book = HTMLInput::new().convert(&root_path, &output_dir).expect("Conversion failed");
    let titles = book.metadata.get("title");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].value, "untitled_chapter");
}
