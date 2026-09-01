use base64::Engine;
use calibre_ebooks::input::fb2_input::FB2Input;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_fb2_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let fb2_path = tmp_dir.path().join("test.fb2");

    // A real, tiny (1x1) JPEG, generated at test time rather than
    // hand-typed -- the fixture used to embed a placeholder run of
    // literal `~~~~` characters where valid base64 image data
    // belonged, which FB2Input's decoder correctly rejected (#196).
    let mut jpeg_bytes = Vec::new();
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut std::io::Cursor::new(&mut jpeg_bytes), image::ImageOutputFormat::Jpeg(90))
        .unwrap();
    let jpeg_base64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

    // Minimal valid structure + binary image + body
    let fb2_content = format!(
        r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <book-title>Test Book</book-title>
      <lang>en</lang>
    </title-info>
  </description>
  <body>
    <title>
      <p>Chapter 1</p>
    </title>
    <section>
      <p>Hello FB2</p>
      <image l:href="#cover.jpg"/>
    </section>
  </body>
  <binary id="cover.jpg" content-type="image/jpeg">
    {jpeg_base64}
  </binary>
</FictionBook>
"##
    );

    fs::write(&fb2_path, fb2_content).unwrap();

    // Output Dir
    let output_dir = tmp_dir.path().join("output");

    // Convert
    let plugin = FB2Input::new();
    let book = plugin
        .convert(&fb2_path, &output_dir)
        .expect("Conversion failed");

    // Verify Metadata
    let title = book
        .metadata
        .items
        .iter()
        .find(|i| i.term == "title")
        .unwrap();
    assert_eq!(title.value, "Test Book");

    // Verify Binary extraction
    let img_path = output_dir.join("cover.jpg");
    assert!(img_path.exists());

    // Verify Index
    let index_path = output_dir.join("index.html");
    let content = fs::read_to_string(index_path).unwrap();
    assert!(content.contains("Hello FB2"));
    assert!(content.contains(r#"<img src="cover.jpg" />"#));
}

/// Port of upstream's own `except TypeError: ... ignoring` -- one
/// corrupted `<binary>` block shouldn't fail the whole conversion.
#[test]
fn a_corrupted_binary_is_skipped_not_fatal() {
    let tmp_dir = tempdir().unwrap();
    let fb2_path = tmp_dir.path().join("test.fb2");

    let fb2_content = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <book-title>Test Book</book-title>
      <lang>en</lang>
    </title-info>
  </description>
  <body>
    <section>
      <p>Hello FB2</p>
    </section>
  </body>
  <binary id="cover.jpg" content-type="image/jpeg">not-valid-base64~~~~</binary>
</FictionBook>
"##;
    fs::write(&fb2_path, fb2_content).unwrap();

    let output_dir = tmp_dir.path().join("output");
    let plugin = FB2Input::new();
    let book = plugin.convert(&fb2_path, &output_dir).expect("conversion should still succeed");

    assert!(!output_dir.join("cover.jpg").exists(), "the corrupted binary should not have been written");
    let title = book.metadata.items.iter().find(|i| i.term == "title").unwrap();
    assert_eq!(title.value, "Test Book");
}
