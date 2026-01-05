use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::lit::header::LitHeader;
use calibre_ebooks::output::lit_output::LitOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_lit_output_generation() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("input");
    let output_file = tmp_dir.path().join("book.lit");
    fs::create_dir(&input_dir).unwrap();

    // Create Source
    let index_html =
        "<html><head><title>LIT Output Test</title></head><body><p>LIT Content</p></body></html>";
    fs::write(input_dir.join("index.html"), index_html).unwrap();

    // Ingest
    let input = HTMLInput::new();
    let book = input
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("Ingest failed");

    // Export
    let output = LitOutput::new();
    output.convert(&book, &output_file).expect("Export failed");

    // Verify
    assert!(output_file.exists());
    let mut file = fs::File::open(&output_file).unwrap();
    // Validate Header
    let header = LitHeader::parse(&mut file).expect("Invalid LIT header");
    assert_eq!(header.version, 1);
}
