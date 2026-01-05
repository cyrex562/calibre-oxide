use calibre_ebooks::input::tcr_input::TCRInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_tcr_input_conversion_stub() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("test.tcr");
    let output_dir = tmp_dir.path().join("output");

    // Write a dummy TCR file
    fs::write(&input_path, b"DUMMY TCR CONTENT").unwrap();

    let input = TCRInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("TCR conversion failed");

    // Check Manifest
    assert!(!book.manifest.items.is_empty());

    // Check Content File
    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());
    let content = fs::read_to_string(content_path).unwrap();
    assert!(content.contains("TCR Content Not Supported Yet"));
}
