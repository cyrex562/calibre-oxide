use calibre_ebooks::input::rtf_input::RTFInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_rtf_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("test.rtf");
    let output_dir = tmp_dir.path().join("output");

    let rtf_content =
        r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0\pard This is some {\b bold} text.\par}";
    fs::write(&input_path, rtf_content).unwrap();

    let input = RTFInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("RTF conversion failed");

    // Check Manifest
    assert!(!book.manifest.items.is_empty());
    assert!(book.manifest.items.contains_key("content"));

    // Check Content File
    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());
    let content = fs::read_to_string(content_path).unwrap();
    assert!(content.contains("This is some {\\b bold} text.")); // Our simple parser currently keeps RTF tags mostly or escapes them
}

#[test]
fn test_rtf_input_plain_text_fallback() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("plain.rtf");
    let output_dir = tmp_dir.path().join("output_plain");

    let text_content = "Just plain text acting as RTF.";
    fs::write(&input_path, text_content).unwrap();

    let input = RTFInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("Plain text conversion failed");

    let content_path = output_dir.join("index.html");
    let content = fs::read_to_string(content_path).unwrap();
    assert!(content.contains("Just plain text acting as RTF."));
}
