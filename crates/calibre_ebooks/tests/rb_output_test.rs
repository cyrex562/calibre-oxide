use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::output::rb_output::RBOutput;
use calibre_ebooks::rb::header::RbHeader;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_rb_output_generation() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("input");
    let output_file = tmp_dir.path().join("book.rb");
    fs::create_dir(&input_dir).unwrap();

    // Create Source
    let index_html =
        "<html><head><title>RB Output Test</title></head><body><p>RB Content</p></body></html>";
    fs::write(input_dir.join("index.html"), index_html).unwrap();

    // Ingest
    let input = HTMLInput::new();
    let book = input
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("Ingest failed");

    // Export
    let output = RBOutput::new();
    output.convert(&book, &output_file).expect("Export failed");

    // Verify
    assert!(output_file.exists());
    let mut file = fs::File::open(&output_file).unwrap();
    // Validate Header
    let header = RbHeader::parse(&mut file).expect("Invalid RB header");
    assert!(header.toc_count > 0);
}
