use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::manifest::ManifestItem;
use calibre_ebooks::output::pdf_output::PDFOutput;
use lopdf::Document;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_pdf_output_conversion() {
    let tmp_dir = tempdir().unwrap();
    let book_dir = tmp_dir.path().join("book");
    fs::create_dir_all(&book_dir).unwrap();
    let output_path = tmp_dir.path().join("output.pdf");

    // Create a dummy OEB book
    let content_file = "content.html";
    let content_path = book_dir.join(content_file);
    fs::write(
        &content_path,
        "<html><body><p>PDF Content Line</p></body></html>",
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

    let output = PDFOutput::new();
    output
        .convert(&book, &output_path)
        .expect("PDF output conversion failed");

    assert!(output_path.exists());

    // Verify it is a valid PDF structure by loading it back
    let doc = Document::load(&output_path).expect("Failed to load generated PDF");
    assert!(doc.get_pages().len() >= 1);

    // basic sanity check
    assert_eq!(doc.version, "1.4");
}
