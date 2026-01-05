use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::output::txt_output::TXTOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_txt_output_generation() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("input");
    let output_file = tmp_dir.path().join("book.txt");
    fs::create_dir(&input_dir).unwrap();

    // Create Source
    // Use some simple HTML with formatting that html2text should handle (headers, paragraphs)
    let index_html = "<html><head><title>TXT Output Test</title></head><body><h1>Chapter 1</h1><p>This is a paragraph of text.</p><ul><li>Item 1</li><li>Item 2</li></ul></body></html>";
    fs::write(input_dir.join("index.html"), index_html).unwrap();

    // Ingest
    let input = HTMLInput::new();
    let mut book = input
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("Ingest failed");

    // Export
    let output = TXTOutput::new();
    output
        .convert(&mut book, &output_file)
        .expect("Export failed");

    // Verify
    assert!(output_file.exists());
    let content = fs::read_to_string(&output_file).unwrap();

    // Check for converted text content
    assert!(content.contains("Chapter 1"));
    assert!(content.contains("This is a paragraph of text."));
    // html2text usually converts lists with * or -
    assert!(content.contains("* Item 1") || content.contains("- Item 1"));
}
