use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::html_output::HTMLOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_html_output() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Create source content
    fs::write(source_path.join("chapter1.html"), "<h1>Chapter 1</h1>").unwrap();

    // Create Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("ch1", "chapter1.html", "application/xhtml+xml");
    book.spine.add("ch1", true);
    book.metadata.add("title", "Test HTML Output");

    // Output Dir
    let tmp_out = tempdir().unwrap();
    let output_dir = tmp_out.path().join("book_html");

    // Convert
    let output = HTMLOutput::new();
    output
        .convert(&mut book, &output_dir)
        .expect("Conversion failed");

    // Verify
    assert!(output_dir.join("chapter1.html").exists());
    let opf_path = output_dir.join("content.opf");
    assert!(opf_path.exists());

    let content = fs::read_to_string(output_dir.join("chapter1.html")).unwrap();
    assert_eq!(content, "<h1>Chapter 1</h1>");

    let opf_content = fs::read_to_string(opf_path).unwrap();
    assert!(opf_content.contains("Test HTML Output"));
}
