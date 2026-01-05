use calibre_ebooks::input::comic_input::ComicInput;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_comic_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let cbz_path = tmp_dir.path().join("test.cbz");
    let file = File::create(&cbz_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Add dummy image files
    zip.start_file("page1.jpg", options).unwrap();
    zip.write_all(b"fake image data 1").unwrap();

    zip.start_file("page2.png", options).unwrap();
    zip.write_all(b"fake image data 2").unwrap();

    zip.start_file("ignore.txt", options).unwrap();
    zip.write_all(b"text file").unwrap();

    zip.finish().unwrap();

    // Test Conversion
    let output_dir = tmp_dir.path().join("output");
    let plugin = ComicInput::new();
    let book = plugin
        .convert(&cbz_path, &output_dir)
        .expect("Conversion failed");

    // Verify Manifest
    // Should have page1.jpg, page2.png, index.html
    assert!(book
        .manifest
        .items
        .values()
        .any(|item| item.href.ends_with("page1.jpg")));
    assert!(book
        .manifest
        .items
        .values()
        .any(|item| item.href.ends_with("page2.png")));
    let index_item = book
        .manifest
        .items
        .values()
        .find(|i| i.href == "index.html")
        .expect("index.html missing");

    // Verify Index Content
    let index_path = output_dir.join("index.html");
    let content = std::fs::read_to_string(index_path).unwrap();
    assert!(content.contains("page1.jpg"));
    assert!(content.contains("page2.png"));
}
