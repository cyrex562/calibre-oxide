use calibre_ebooks::conversion::archives::{ArchiveHandler, ArchiveType};
use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;
use zip::ZipWriter;

#[test]
fn test_archive_detection() {
    let p1 = std::path::Path::new("test.zip");
    assert!(matches!(ArchiveHandler::detect_type(p1), ArchiveType::Zip));

    let p2 = std::path::Path::new("test.rar");
    assert!(matches!(ArchiveHandler::detect_type(p2), ArchiveType::Rar));

    let p3 = std::path::Path::new("test.unknown");
    assert!(matches!(
        ArchiveHandler::detect_type(p3),
        ArchiveType::Unknown
    ));
}

#[test]
fn test_zip_extraction() {
    let temp_dir = tempdir().unwrap();
    let zip_path = temp_dir.path().join("test.zip");
    let output_dir = temp_dir.path().join("out");

    // Create Test ZIP
    let file = File::create(&zip_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    zip.start_file("hello.txt", options).unwrap();
    zip.write_all(b"Hello World").unwrap();
    zip.finish().unwrap();

    // Extract
    let handler = ArchiveHandler::new();
    handler.extract(&zip_path, &output_dir).unwrap();

    // Verify
    let hello_path = output_dir.join("hello.txt");
    assert!(hello_path.exists());
    let content = fs::read_to_string(hello_path).unwrap();
    assert_eq!(content, "Hello World");
}
