use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::mobi_output::MOBIOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_mobi_output_conversion() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Content
    fs::write(
        source_path.join("page.html"),
        "<h1>MOBI Page</h1><p>Content</p>",
    )
    .unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "MOBI Test Book");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book.mobi");

    // Convert
    let output = MOBIOutput::new();
    output
        .convert(&book, &output_path)
        .expect("Conversion failed");

    // Verify file exists
    assert!(output_path.exists());

    // Basic Size Check
    let meta = fs::metadata(&output_path).unwrap();
    assert!(meta.len() > 100); // Should have headers

    // Future: Use Reader to verify (Circular dependency if we use lib in test? No, lib is under test)
    // We can try to use MobiReader if it's public.
    // use calibre_ebooks::mobi::reader::MobiReader;
    // But MobiReader might expect complex structure.
    // For now, just existence and size check is good initial verification.
}
