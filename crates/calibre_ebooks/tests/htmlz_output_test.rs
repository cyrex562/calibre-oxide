use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::htmlz_output::HTMLZOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_htmlz_output_conversion() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Content
    fs::write(source_path.join("page.html"), "<h1>Page</h1>").unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "Output HTMLZ");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book.htmlz");

    // Convert
    let output = HTMLZOutput::new();
    output
        .convert(&mut book, &output_path)
        .expect("Conversion failed");

    // Verify by unzipping
    assert!(output_path.exists());
    let file = fs::File::open(&output_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();

    // Should have content.opf and page.html
    assert!(zip.by_name("content.opf").is_ok());
    assert!(zip.by_name("page.html").is_ok());
}
