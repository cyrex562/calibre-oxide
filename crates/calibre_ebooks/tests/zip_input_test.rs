use calibre_ebooks::input::zip_input::ZIPInput;
use std::fs;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_zip_input_conversion_with_index() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("test.zip");

    {
        let file = fs::File::create(&input_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("index.html", options).unwrap();
        use std::io::Write;
        zip.write_all(b"<html><body><h1>Hello ZIP</h1></body></html>")
            .unwrap();
        zip.finish().unwrap();
    }

    let input = ZIPInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    assert!(output_dir.join("index.html").exists());
    let content = fs::read_to_string(output_dir.join("index.html")).unwrap();
    assert!(content.contains("Hello ZIP"));

    // Should detect index.html and title "ZIP Content"
    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "ZIP Content");
}

#[test]
fn test_zip_input_conversion_fallback() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out_fb");
    let input_path = temp_dir.path().join("test_fb.zip");

    {
        let file = fs::File::create(&input_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("image.jpg", options).unwrap();
        use std::io::Write;
        zip.write_all(b"fake image data").unwrap();
        zip.finish().unwrap();
    }

    let input = ZIPInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    // Should generate "generated_index.html" and title "ZIP Archive"
    assert!(output_dir.join("generated_index.html").exists());

    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "ZIP Archive");
}
