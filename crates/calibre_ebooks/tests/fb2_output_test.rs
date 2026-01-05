use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::fb2_output::FB2Output;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_fb2_output_conversion() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Content
    fs::write(
        source_path.join("ch1.html"),
        "<html><body><h1>Title</h1><p>Text</p><img src=\"image.jpg\"/></body></html>",
    )
    .unwrap();

    // Image
    fs::write(source_path.join("image.jpg"), "fake image data").unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("ch1", "ch1.html", "application/xhtml+xml");
    book.manifest.add("img1", "image.jpg", "image/jpeg");
    book.spine.add("ch1", true);
    book.metadata.add("title", "FB2 Test");
    book.metadata.add("creator", "Author Name");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book.fb2");

    // Convert
    let output = FB2Output::new();
    output
        .convert(&mut book, &output_path)
        .expect("Conversion failed");

    // Verify
    let content = fs::read_to_string(output_path).unwrap();

    assert!(content.contains("<book-title>FB2 Test</book-title>"));
    assert!(content.contains("<author><first-name>Author Name</first-name></author>"));
    assert!(content.contains("<section>"));
    assert!(content.contains("<p>Text</p>")); // p inside p? My logic wrapped content in section.
    assert!(content.contains("<image l:href=\"#image.jpg\"/>"));
    assert!(content.contains("<binary id=\"image.jpg\" content-type=\"image/jpeg\">"));

    // Check base64 of "fake image data"
    // "fake image data" -> "ZmFrZSBpbWFnZSBkYXRh"
    assert!(content.contains("ZmFrZSBpbWFnZSBkYXRh"));
}
