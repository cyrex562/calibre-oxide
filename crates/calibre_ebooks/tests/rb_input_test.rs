use calibre_ebooks::input::rb_input::RBInput;
use calibre_ebooks::rb::writer::RbWriter;
use std::fs::File;
use std::io::BufWriter;
use tempfile::tempdir;

#[test]
fn test_rb_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("book.rb");
    let output_dir = tmp_dir.path().join("output");

    // Create Mock RB File
    {
        let file = File::create(&input_path).unwrap();
        let mut writer = BufWriter::new(file);
        let mut rb_writer = RbWriter::new();

        let content = "<html><body><h1>RB Test</h1><p>Content</p></body></html>";
        rb_writer.add_entry("content.html", content.as_bytes().to_vec(), 0);
        rb_writer
            .write(&mut writer)
            .expect("Failed to write mock RB");
    }

    // Convert
    let input = RBInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("Conversion failed");

    // Verify
    let title = book.metadata.get("title").first().unwrap().value.clone();
    assert_eq!(title, "Unknown RB Book");
    assert!(book.manifest.items.contains_key("content"));

    // Check content file exists
    let content_path = output_dir.join("content.html");
    assert!(content_path.exists());
    let extracted = std::fs::read_to_string(&content_path).unwrap();
    assert!(extracted.contains("RB Test"));
}
