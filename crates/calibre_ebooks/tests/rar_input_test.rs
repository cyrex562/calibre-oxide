use calibre_ebooks::input::rar_input::RARInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_rar_input_placeholder() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("test.rar");
    fs::write(&input_path, b"DUMMY RAR DATA").unwrap();

    let input = RARInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    // Verify it produced the placeholder
    assert!(output_dir.join("index.html").exists());
    let content = fs::read_to_string(output_dir.join("index.html")).unwrap();
    assert!(content.contains("RAR Content Not Supported Yet"));

    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "Converted RAR");
}
