use calibre_ebooks::input::pdf_input::PDFInput;
use lopdf::{Dictionary, Document, Object};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_pdf_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("test.pdf");
    let output_dir = tmp_dir.path().join("output");

    // Create a minimal PDF using lopdf
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let font_id = doc.new_object_id();
    let content_id = doc.new_object_id();
    let page_id = doc.new_object_id();

    // Content: "Hello World"
    let content_ops = "BT /F1 12 Tf 100 100 Td (Hello World) Tj ET";
    let content_stream = lopdf::Stream::new(Dictionary::new(), content_ops.as_bytes().to_vec());
    doc.objects
        .insert(content_id, Object::Stream(content_stream));

    // Font
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    doc.objects.insert(font_id, Object::Dictionary(font));

    // Page
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set(
        "MediaBox",
        Object::Array(vec![0.into(), 0.into(), 500.into(), 500.into()]),
    );
    page.set("Contents", Object::Reference(content_id));

    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set("F1", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    page.set("Resources", Object::Dictionary(resources));

    doc.objects.insert(page_id, Object::Dictionary(page));

    // Pages
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    // Catalog
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(&input_path).unwrap();

    let input = PDFInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("PDF conversion failed");

    // Check Manifest
    assert!(!book.manifest.items.is_empty());

    // Check Content
    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());
    let content = fs::read_to_string(content_path).unwrap();
    // Our naive extractor should find "Hello World"
    assert!(content.contains("Hello World"));
}
