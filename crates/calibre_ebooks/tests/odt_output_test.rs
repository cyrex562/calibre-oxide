use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::manifest::ManifestItem;
use calibre_ebooks::output::odt_output::ODTOutput;
use std::fs;
use std::io::Read;
use tempfile::tempdir;
use zip::ZipArchive;

#[test]
fn test_odt_output_conversion() {
    let temp_dir = tempdir().unwrap();
    let output_path = temp_dir.path().join("test_out.odt");
    let input_dir = temp_dir.path().join("source");
    fs::create_dir_all(&input_dir).unwrap();

    // Setup OEBBook
    let content = "<html><body><p>Test Paragraph</p></body></html>";
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
    let output = ODTOutput::new();
    output.convert(&book, &output_path).unwrap();

    // Verify Output
    assert!(output_path.exists());

    let file = fs::File::open(&output_path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();

    // Check mimetype
    let mut mimetype = String::new();
    archive
        .by_name("mimetype")
        .unwrap()
        .read_to_string(&mut mimetype)
        .unwrap();
    assert_eq!(mimetype, "application/vnd.oasis.opendocument.text");

    // Check content.xml
    let mut content_xml = String::new();
    archive
        .by_name("content.xml")
        .unwrap()
        .read_to_string(&mut content_xml)
        .unwrap();
    assert!(content_xml.contains("<text:p>Test Paragraph</text:p>"));
}
