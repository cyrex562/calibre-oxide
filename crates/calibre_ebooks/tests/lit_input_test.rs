use calibre_ebooks::input::lit_input::LitInput;
use calibre_ebooks::lit::writer::LitWriter;
use std::fs::File;
use std::io::BufWriter;
use tempfile::tempdir;

#[test]
fn test_lit_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("book.lit");
    let output_dir = tmp_dir.path().join("output");

    // Create Mock LIT File
    {
        let file = File::create(&input_path).unwrap();
        let mut writer = BufWriter::new(file);
        let lit_writer = LitWriter::new();
        lit_writer
            .write_dummy(&mut writer)
            .expect("Failed to write mock LIT");
    }

    // Convert
    let input = LitInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("Conversion failed");

    // Verify
    // The dummy conversion currently creates a generic title and content placeholder
    assert!(book
        .metadata
        .get("title")
        .first()
        .unwrap()
        .value
        .contains("Converted LIT Book"));

    // Check content file exists
    let content_path = output_dir.join("content.html");
    assert!(content_path.exists());
    let extracted = std::fs::read_to_string(&content_path).unwrap();
    assert!(extracted.contains("LIT Conversion")); // or whatever the placeholder says
}
