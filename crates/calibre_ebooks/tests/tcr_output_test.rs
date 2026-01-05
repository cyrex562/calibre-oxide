use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::manifest::ManifestItem;
use calibre_ebooks::output::tcr_output::TCROutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_tcr_output_conversion() {
    let temp_dir = tempdir().unwrap();
    let output_path = temp_dir.path().join("test_out.tcr");
    let input_dir = temp_dir.path().join("source");
    fs::create_dir_all(&input_dir).unwrap();

    // Setup OEBBook
    let content = "<html><body><p>Hello TCR</p></body></html>";
    fs::write(input_dir.join("index.html"), content).unwrap();

    let container = Box::new(DirContainer::new(&input_dir));
    let mut book = OEBBook::new(container);
    let id = "content".to_string();
    let href = "index.html".to_string();
    book.manifest.items.insert(
        id.clone(),
        ManifestItem::new(&id, &href, "application/xhtml+xml"),
    );
    book.manifest.hrefs.insert(href.clone(), id.clone());
    book.spine.add(&id, true);

    // Run conversion
    let output = TCROutput::new();
    output.convert(&book, &output_path).unwrap();

    // Verify Output
    assert!(output_path.exists());
    let text = fs::read_to_string(&output_path).unwrap();
    assert!(text.contains("Hello TCR"));
}
