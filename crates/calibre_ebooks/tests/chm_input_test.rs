use calibre_ebooks::input::chm_input::CHMInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_chm_input_placeholder() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("test.chm");
    fs::write(&input_path, b"DUMMY CHM DATA").unwrap();

    let input = CHMInput::new();
    println!("Converting...");
    let book = input.convert(&input_path, &output_dir).unwrap();
    println!("Converted.");

    // Verify it produced the placeholder
    assert!(output_dir.join("index.html").exists());
    let content = fs::read_to_string(output_dir.join("index.html")).unwrap();
    assert!(content.contains("CHM Content Not Supported Yet"));
    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "Converted CHM");
}
