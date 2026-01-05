use calibre_ebooks::input::fb2_input::FB2Input;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_fb2_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let fb2_path = tmp_dir.path().join("test.fb2");
    
    // Create sample FB2
    // Minimal valid structure + binary image + body
    let fb2_content = r#"<?xml version="1.0" encoding="utf-8"?>
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
    /9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBAB~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~//2Q==
  </binary>
</FictionBook>
"#;
    
    fs::write(&fb2_path, fb2_content).unwrap();
    
    // Output Dir
    let output_dir = tmp_dir.path().join("output");
    
    // Convert
    let plugin = FB2Input::new();
    let book = plugin.convert(&fb2_path, &output_dir).expect("Conversion failed");
    
    // Verify Metadata
    let title = book.metadata.items.iter().find(|i| i.term == "title").unwrap();
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
