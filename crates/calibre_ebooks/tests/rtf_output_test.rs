use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::manifest::ManifestItem;
use calibre_ebooks::output::rtf_output::RTFOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_rtf_output_conversion() {
    let tmp_dir = tempdir().unwrap();
    let book_dir = tmp_dir.path().join("book");
    fs::create_dir_all(&book_dir).unwrap();
    let output_path = tmp_dir.path().join("output.rtf");

    // Create a dummy OEB book
    let content_file = "content.html";
    let content_path = book_dir.join(content_file);
    fs::write(
        &content_path,
        "<html><body><p>Hello World</p><p>Second Line</p></body></html>",
    )
    .unwrap();

    let container = Box::new(DirContainer::new(&book_dir));
    let mut book = OEBBook::new(container);
    let id = "item1".to_string();
    book.manifest.items.insert(
        id.clone(),
        ManifestItem {
            id: id.clone(),
            href: content_file.to_string(),
            media_type: "application/xhtml+xml".to_string(),
            fallback: None,
            linear: true,
        },
    );
    book.spine.add(&id, true);

    let output = RTFOutput::new();
    output
        .convert(&book, &output_path)
        .expect("RTF output conversion failed");

    assert!(output_path.exists());
    let rtf_content = fs::read_to_string(output_path).unwrap();

    // Check basic RTF structure
    assert!(rtf_content.starts_with("{\\rtf1"));
    assert!(rtf_content.contains("Hello World"));
    assert!(rtf_content.contains("Second Line"));
    assert!(rtf_content.contains("\\par"));
}
