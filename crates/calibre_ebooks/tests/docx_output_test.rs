use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::docx_output::DOCXOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_docx_output_basics() {
    let temp_dir = tempdir().unwrap();
    let output_path = temp_dir.path().join("test.docx");
    
    // Create Dummy Book
    let container_path = temp_dir.path().join("src");
    fs::create_dir_all(&container_path).unwrap();
    let container = Box::new(DirContainer::new(&container_path));
    let mut book = OEBBook::new(container);
    book.metadata.add("title", "My DOCX Book");

    let output_plugin = DOCXOutput::new();
    output_plugin.convert(&book, &output_path).unwrap();

    assert!(output_path.exists());

    // Verify ZIP content
    let file = fs::File::open(&output_path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();

    assert!(archive.by_name("[Content_Types].xml").is_ok());
    assert!(archive.by_name("_rels/.rels").is_ok());
    assert!(archive.by_name("word/document.xml").is_ok());
}
