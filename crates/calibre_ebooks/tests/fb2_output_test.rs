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
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1><p>Text</p><img src="image.png"/></body></html>"#,
    )
    .unwrap();

    // A real (not fake-bytes) image -- the real Fb2Mlizer this is
    // now wired to validates/transcodes image data via the `image`
    // crate, unlike the old ad-hoc converter this test used to cover.
    let png = {
        let mut buf = Vec::new();
        image::DynamicImage::new_rgb8(2, 2).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageOutputFormat::Png).unwrap();
        buf
    };
    fs::write(source_path.join("image.png"), &png).unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("ch1", "ch1.html", "application/xhtml+xml");
    book.manifest.add("img1", "image.png", "image/png");
    book.spine.add("ch1", true);
    book.metadata.add("title", "FB2 Test");
    book.metadata.add("creator", "Author Name");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book.fb2");

    // Convert
    let output = FB2Output::new();
    output
        .convert(&book, &output_path)
        .expect("Conversion failed");

    // Verify -- real Fb2Mlizer structure (FictionBook wrapper,
    // document-info, renumbered `img_N` image ids, real JPEG-encoded
    // binaries) that the old ad-hoc converter never produced.
    let content = fs::read_to_string(output_path).unwrap();

    assert!(content.starts_with("<?xml"), "{content}");
    assert!(content.contains("<FictionBook"), "{content}");
    assert!(content.contains("<book-title>FB2 Test</book-title>"), "{content}");
    // Real Fb2Mlizer splits a "First Last" creator into separate
    // first-name/last-name elements, unlike the old ad-hoc converter's
    // single <first-name>Author Name</first-name>.
    assert!(content.contains("<first-name>Author</first-name><last-name>Name</last-name>"), "{content}");
    assert!(content.contains("<document-info>"), "{content}");
    assert!(content.contains("<section"), "{content}");
    assert!(content.contains("<p>Text</p>"), "{content}");
    assert!(content.contains("<image l:href=\"#img_0\"/>"), "{content}");
    // A native PNG passes through `DefaultImageConverter` unchanged
    // (only non-native formats are actually transcoded to JPEG).
    assert!(content.contains("<binary id=\"img_0\" content-type=\"image/png\">"), "{content}");
}
