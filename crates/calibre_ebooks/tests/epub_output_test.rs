use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::metadata::{Item, Metadata};
use calibre_ebooks::oeb::writer::OEBWriter; // We still might want OEBWriter directly for setup? No, just book.
use calibre_ebooks::output::epub_output::EPUBOutput;
use std::fs;
use tempfile::tempdir;
use zip::ZipArchive;

#[test]
fn test_epub_output_generation() {
    let tmp_dir = tempdir().unwrap();
    let output_epub = tmp_dir.path().join("output.epub");

    // Setup Mock Book
    let book_dir = tmp_dir.path().join("book_source");
    fs::create_dir(&book_dir).unwrap();

    // Create a simple file in the source container
    fs::write(
        book_dir.join("page.html"),
        "<html><body>Hello</body></html>",
    )
    .unwrap();

    let container = Box::new(DirContainer::new(&book_dir));
    let mut book = OEBBook::new(container);

    // Add Metadata
    book.metadata.items.push(Item {
        term: "dc:title".to_string(),
        value: "Output Test".to_string(),
        attrib: Default::default(),
    });

    // Add Manifest Item (manually, as we skipped parsing)
    use calibre_ebooks::oeb::manifest::ManifestItem;
    book.manifest.items.insert(
        "p1".to_string(),
        ManifestItem {
            id: "p1".to_string(),
            href: "page.html".to_string(),
            media_type: "application/xhtml+xml".to_string(),
            fallback: None,
            linear: true,
        },
    );

    use calibre_ebooks::oeb::spine::SpineItem;
    book.spine.items.push(SpineItem {
        idref: "p1".to_string(),
        linear: true,
    });

    // Run EPUBOutput
    let output = EPUBOutput::new();
    output
        .convert(&mut book, &output_epub)
        .expect("Failed to create EPUB");

    assert!(output_epub.exists());

    // Verify ZIP content
    let file = fs::File::open(&output_epub).unwrap();
    let mut zip = ZipArchive::new(file).unwrap();

    // Check mimetype
    {
        let mut f = zip.by_name("mimetype").expect("mimetype missing");
        let mut content = String::new();
        std::io::Read::read_to_string(&mut f, &mut content).unwrap();
        assert_eq!(content, "application/epub+zip");
        // Verify no compression stored (method 0)
        assert_eq!(f.compression(), zip::CompressionMethod::Stored);
    }

    // Check container.xml
    assert!(zip.by_name("META-INF/container.xml").is_ok());

    // Check content.opf
    assert!(zip.by_name("content.opf").is_ok());

    // Check page.html
    assert!(zip.by_name("page.html").is_ok());
}
