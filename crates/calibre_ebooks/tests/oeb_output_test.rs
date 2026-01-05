use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::oeb_output::OEBOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_oeb_output_conversion() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Content
    fs::write(source_path.join("page.html"), "<h1>OEB Page</h1>").unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "OEB Test Book");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book_oeb");

    // Convert
    let output = OEBOutput::new();
    output
        .convert(&mut book, &output_path)
        .expect("Conversion failed");

    // Verify
    assert!(output_path.exists());
    assert!(output_path.is_dir());
    assert!(output_path.join("content.opf").exists());
    assert!(output_path.join("page.html").exists());

    let content = fs::read_to_string(output_path.join("page.html")).unwrap();
    assert_eq!(content, "<h1>OEB Page</h1>");

    let opf = fs::read_to_string(output_path.join("content.opf")).unwrap();
    assert!(opf.contains("OEB Test Book"));
}
